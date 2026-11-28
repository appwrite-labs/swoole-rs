//! A server task's payload: `Server::task_pack` puts it inline, or in a temporary
//! file when it is too large for the pipe; `Server::task_unpack` reads it back
//! (`File::read_all`: pread from offset 0, retrying EINTR/EAGAIN, stopping at EOF or
//! another error) and unlinks the file unless the task is only peeked;
//! `Server::task_dump` prints it.
use crate::{CCast, ServerEvent};
use std::ffi::{CStr, c_int, c_void};
use std::mem::{MaybeUninit, offset_of};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::FileExt;

use crate::constants::TaskFlag;
use crate::{DataHead, datahead_dump, log, microtime, task_tmpfile};

use crate::buffer::string::StringBuffer;
use crate::ossupport::fd_write_all;

use crate::constants::SW_IPC_BUFFER_SIZE as IPC_PAYLOAD;
use crate::process::methods::{swoole_get_worker_id, swoole_get_worker_pid};

/// Why reading a task's temporary file failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskFileError {
    /// Opening it failed with this errno.
    Open(i32),
    /// It ended after `read` bytes.
    Short { read: usize },
    /// pread on `fd` failed with `error` after `read` bytes.
    Pread { error: i32, fd: i32, read: usize },
}

pub(crate) use crate::sys::errno;

/// Fills `buffer` from the start of the file at `path`, and unlinks the file once
/// it is read whole, unless `peek`.
fn read_file(path: &CStr, buffer: &mut [u8], peek: bool) -> Result<(), TaskFileError> {
    let path = std::ffi::OsStr::from_bytes(path.to_bytes());
    let file = std::fs::File::open(path)
        .map_err(|error| TaskFileError::Open(error.raw_os_error().unwrap_or(0)))?;
    let mut read = 0;
    while let Some(rest) = buffer.get_mut(read..).filter(|rest| !rest.is_empty()) {
        match file.read_at(rest, read as u64) {
            Ok(0) => return Err(TaskFileError::Short { read }),
            Ok(n) => read += n,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock
                ) => {}
            Err(error) => {
                return Err(TaskFileError::Pread {
                    error: error.raw_os_error().unwrap_or(0),
                    fd: file.as_raw_fd(),
                    read,
                });
            }
        }
    }
    drop(file);
    if !peek {
        std::fs::remove_file(path).unwrap_or_default();
    }
    Ok(())
}

/// `SW_TASK_TMP_PATH_SIZE`.
pub(crate) const TASK_TMP_PATH_SIZE: usize = crate::constants::SW_TASK_TMP_PATH_SIZE as usize;

/// The payload descriptor written into `EventData::data` when a task spills to a
/// temp file (upstream's `swoole::PacketTask`).
pub(super) struct PacketTask {
    length: usize,
    tmpfile: [u8; TASK_TMP_PATH_SIZE],
}

impl PacketTask {
    /// Writes its bytes to the start of `to`, as upstream copies the struct.
    fn write(&self, to: &mut [MaybeUninit<u8>]) {
        let (length, name) = to.split_at_mut(size_of::<usize>());
        length.write_copy_of_slice(&self.length.to_ne_bytes());
        if let Some(name) = name.get_mut(..TASK_TMP_PATH_SIZE) {
            name.write_copy_of_slice(&self.tmpfile);
        }
    }

    /// Reads the payload length and borrows the path from one complete descriptor.
    fn read(from: &[u8]) -> Result<(usize, &CStr), &'static str> {
        let (length, name) = from
            .split_first_chunk()
            .ok_or("invalid task packet length")?;
        let name: &[u8; TASK_TMP_PATH_SIZE] =
            name.try_into().map_err(|_| "invalid task packet length")?;
        let path =
            CStr::from_bytes_until_nul(name).map_err(|_| "unterminated task tmpfile path")?;
        Ok((usize::from_ne_bytes(*length), path))
    }
}

/// What packing a task payload did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TaskPack {
    /// false when the temp file could not be created or written (already warned).
    pub ok: bool,
    /// true when the payload spilled to a temp file and `dest` holds a `PacketTask`.
    pub tmpfile: bool,
    /// The byte count for `info.len`: the payload length, or `sizeof(PacketTask)`.
    pub info_len: usize,
}

/// `make_tmpfile`: `template` cut to `SW_TASK_TMP_PATH_SIZE - 1` bytes into `path`
/// and created there by `swoole_tmpfile`, which warns on failure: the open file, or
/// `None`.
pub(crate) fn make_tmpfile(
    template: &CStr,
    path: &mut [u8; TASK_TMP_PATH_SIZE],
) -> Option<crate::ossupport::TempFile> {
    *path = [0; TASK_TMP_PATH_SIZE];
    for (byte, &pattern) in path
        .iter_mut()
        .take(TASK_TMP_PATH_SIZE - 1)
        .zip(template.to_bytes())
    {
        *byte = pattern;
    }
    crate::ossupport::TempFile::create(path)
}

/// Writes the payload to the temp file as `File::write_all` does (warning on an
/// IO error), then warns `write to tmpfile failed` on a short write.
fn spill(file: &std::fs::File, data: &[u8]) -> bool {
    let wrote = fd_write_all(file.as_raw_fd(), false, data) == data.len();
    if !wrote {
        let error = errno();
        log::warning(
            c"Server::task_pack",
            format_args!("write to tmpfile failed"),
        );
        crate::sys::set_errno(error);
    }
    wrote
}

/// `Server::task_pack`'s payload placement: inline into `dest` when it fits,
/// otherwise a temp file created from `template()` (an mkstemp pattern), whose
/// path and length are written into `dest` (at least `sizeof(PacketTask)` bytes)
/// as a `PacketTask`. Only a spill asks for the template, which a lock guards.
fn pack_payload<T: AsRef<CStr>>(
    dest: &mut [MaybeUninit<u8>],
    data: &[u8],
    template: impl FnOnce() -> T,
) -> TaskPack {
    let fits = data.len() < dest.len();
    if let Some(inline) = dest.get_mut(..data.len()).filter(|_| fits) {
        inline.write_copy_of_slice(data);
        return TaskPack {
            ok: true,
            tmpfile: false,
            info_len: data.len(),
        };
    }
    let mut pkg = PacketTask {
        length: data.len(),
        tmpfile: [0; TASK_TMP_PATH_SIZE],
    };
    let Some(file) = make_tmpfile(template().as_ref(), &mut pkg.tmpfile) else {
        return TaskPack::default();
    };
    if !spill(&file.file, data) {
        return TaskPack::default();
    }
    pkg.write(dest);
    drop(file.persist());
    TaskPack {
        ok: true,
        tmpfile: true,
        info_len: size_of::<PacketTask>(),
    }
}

/// `swoole::EventData`'s header (`DataHead`), which native asserts is 40 bytes.
pub use crate::DataHead as EventHead;

/// An `EventData` to pack a task into. Its payload starts uninitialized, as a
/// stack `swoole::EventData` does, and only [`Self::pack`] sets the head's `len`,
/// so the bytes it counts are always written.
// `repr(C)`: sent and received as its head followed by its payload bytes.
#[repr(C)]
pub struct TaskBuffer {
    head: DataHead,
    data: [MaybeUninit<u8>; IPC_PAYLOAD],
}

const _: () = assert!(
    size_of::<DataHead>() == 40,
    "`Packed::bytes` reads the head as bytes, so it has no padding"
);

impl Default for TaskBuffer {
    fn default() -> Self {
        Self {
            head: DataHead::default(),
            data: [MaybeUninit::uninit(); IPC_PAYLOAD],
        }
    }
}

impl TaskBuffer {
    /// The head, as `pack` and the flag setters left it.
    #[must_use]
    pub fn head(&self) -> &DataHead {
        &self.head
    }

    /// `info.ext_flags |= flags`.
    pub fn add_flags(&mut self, flags: u16) {
        self.head.ext_flags |= flags;
    }

    /// Identify the synchronous batch sharing a result slot.
    pub(super) fn set_batch_id(&mut self, id: u64) {
        self.head.msg_id = id;
    }

    /// Set the event and the ID its receiver uses to match it.
    pub fn set_event(&mut self, event: ServerEvent, id: std::ffi::c_long) {
        self.head.kind = event.code();
        self.head.fd = id;
    }

    /// `info.server_fd = server_fd`.
    pub fn set_server_fd(&mut self, server_fd: u16) {
        self.head.server_fd = server_fd;
    }

    /// The buffer as a packed task: its head, then the `len` bytes `pack` wrote.
    #[must_use]
    pub fn packed(&self) -> Packed<'_> {
        let length = size_of::<DataHead>() + (self.head.len as usize).min(IPC_PAYLOAD);
        // SAFETY: the head (no padding, as asserted above) and then the payload's
        // first `len` bytes, all within `self` and initialized: only `pack` sets
        // `len`, after writing the bytes it counts.
        let bytes = unsafe { std::slice::from_raw_parts(std::ptr::from_ref(self).cast(), length) };
        Packed { bytes }
    }

    /// `Server::task_pack`: a task header from this worker, with `data` inline or
    /// in a temporary file.
    pub fn pack(&mut self, data: &[u8]) -> bool {
        let Some(id) = crate::global::next_task_id() else {
            self.head = DataHead {
                fd: -1,
                ..DataHead::default()
            };
            log::error_log(
                log::LogLevel::SW_LOG_WARNING,
                libc::EOVERFLOW,
                c"Server::task_pack",
                format_args!("task ID range exhausted"),
            );
            return false;
        };
        self.pack_event(data, ServerEvent::Task, id)
    }

    /// Release a locally packed spill when its waiter has already left.
    pub fn discard(&self) {
        self.packed().discard();
    }

    /// Finish a task, retaining the originating worker's PID on asynchronous replies.
    pub fn pack_result(&mut self, data: &[u8], task: &DataHead) -> bool {
        let packed = self.pack_event(data, ServerEvent::Finish, task.fd);
        if TaskFlag::from_bits(task.ext_flags).contains(TaskFlag::SW_TASK_NONBLOCK) {
            self.head.reserved = task.reserved;
        }
        packed
    }

    /// Pack an event with the ID that its receiver will match.
    pub fn pack_event(&mut self, data: &[u8], event: ServerEvent, id: std::ffi::c_long) -> bool {
        let reactor_id = swoole_get_worker_id().c_cast::<i16>();
        self.head = DataHead {
            fd: id,
            msg_id: 0,
            len: 0,
            reactor_id,
            kind: event.code(),
            flags: 0,
            server_fd: 0,
            ext_flags: 0,
            reserved: swoole_get_worker_pid().cast_unsigned(),
            time: microtime(),
        };
        let packed = pack_payload(&mut self.data, data, task_tmpfile);
        if !packed.ok {
            return false;
        }
        let len = packed.info_len.c_cast::<u32>();
        self.head.len = len;
        if packed.tmpfile {
            self.head.ext_flags |= TaskFlag::SW_TASK_TMPFILE.bits();
        }
        true
    }
}

/// A packed task lent for `'a`: its `DataHead`, at any alignment, then the `len`
/// bytes it counts (the payload, or a temporary file's `PacketTask`).
#[derive(Clone, Copy)]
pub struct Packed<'a> {
    /// The head and exactly the `len` bytes it counts.
    bytes: &'a [u8],
}

impl<'a> Packed<'a> {
    /// Remove a spilled result that its waiter abandoned.
    pub(super) fn discard(self) {
        if TaskFlag::from_bits(self.head().ext_flags).contains(TaskFlag::SW_TASK_TMPFILE)
            && let Ok((_, path)) = PacketTask::read(self.payload())
        {
            _ = crate::sys::unlink(path);
        }
    }

    /// `Server::task_dump`: the header, and the temporary file's name if it has one.
    pub fn dump(self) {
        let mut buf = [0_u8; 1024];
        let head = self.head();
        datahead_dump(&head, &mut buf);
        let text = CStr::from_bytes_until_nul(&buf).unwrap_or_default();
        crate::sys::print(text.to_bytes());
        if TaskFlag::from_bits(head.ext_flags).contains(TaskFlag::SW_TASK_TMPFILE)
            && let Ok((length, path)) = PacketTask::read(self.payload())
        {
            // Upstream prints the payload's length in bytes of the file's name; a
            // negative `(int)` precision prints all of it.
            let name = path.to_bytes();
            let precision = length.c_cast::<c_int>();
            let shown = usize::try_from(precision).map_or(name.len(), |n| n.min(name.len()));
            let name = name.get(..shown).unwrap_or(name);
            let line = [b"Task[tmpfile]=", name, b"\n"].concat();
            crate::sys::print(&line);
        }
    }

    /// The task at the start of `bytes`; `None` when they are too short for its
    /// head and the `len` bytes it counts.
    #[must_use]
    pub fn from_bytes(bytes: &'a [u8]) -> Option<Self> {
        Self::split(bytes).map(|(record, _)| record)
    }

    pub(super) fn split(bytes: &'a [u8]) -> Option<(Self, &'a [u8])> {
        let len = bytes
            .get(offset_of!(DataHead, len)..)
            .and_then(<[u8]>::first_chunk)
            .map(|len| u32::from_ne_bytes(*len) as usize)?;
        let (bytes, rest) = bytes.split_at_checked(size_of::<DataHead>().checked_add(len)?)?;
        Some((Self { bytes }, rest))
    }

    /// The task's address, for the native-shaped calls that still take it.
    #[must_use]
    pub fn as_ptr(self) -> *const c_void {
        self.bytes.as_ptr().cast()
    }

    /// The task's head.
    ///
    /// # Panics
    /// Only if an internal constructor violates the complete-header invariant.
    #[must_use]
    pub fn head(self) -> DataHead {
        #[expect(
            clippy::expect_used,
            reason = "both Packed constructors include the entire DataHead"
        )]
        DataHead::from_bytes(self.bytes.first_chunk().expect("validated task header"))
    }

    /// The task's head and the `len` bytes after it.
    #[must_use]
    pub fn bytes(self) -> &'a [u8] {
        self.bytes
    }

    fn payload(self) -> &'a [u8] {
        self.bytes.split_at(size_of::<DataHead>()).1
    }

    /// `Server::task_unpack(task, buffer, &packet)`: the payload's bytes, read
    /// into `buffer` from its temporary file if it has one, and the task's
    /// `ext_flags`; `None` (warned) if that file can't be read.
    pub fn unpack<'b>(self, buffer: &'b mut StringBuffer) -> Option<(&'b [u8], u16)>
    where
        'a: 'b,
    {
        let (payload, flags) = (self.payload(), self.head().ext_flags);
        let spilled = TaskFlag::from_bits(flags).contains(TaskFlag::SW_TASK_TMPFILE);
        let malformed = |message| {
            log::error_log(
                log::LogLevel::SW_LOG_WARNING,
                swoole_rs_error::ErrorCode::SW_ERROR_PACKAGE_MALFORMED_DATA as c_int,
                c"Server::task_unpack",
                format_args!("{message}"),
            );
            None
        };
        if payload.len() > IPC_PAYLOAD {
            return malformed("invalid task packet length");
        }
        if !spilled {
            return Some((payload, flags));
        }
        let (length, path) = match PacketTask::read(payload) {
            Ok(fields) => fields,
            Err(message) => return malformed(message),
        };
        if buffer.size() < length {
            buffer.extend(length);
        }
        let str = buffer.as_mut_ptr();
        // Zeroed first: past the String's content, its allocation is uninitialized. It
        // holds `length` bytes by now, which become the content once read; a
        // failed read keeps the content's length.
        let kept = buffer.len();
        let mut read = Ok(());
        buffer.read_zeroed(length, |target| {
            read = read_file(
                path,
                target,
                TaskFlag::from_bits(flags).contains(TaskFlag::SW_TASK_PEEK),
            );
            if read.is_ok() { target.len() } else { kept }
        });
        match read {
            Ok(()) => {}
            Err(TaskFileError::Short { .. }) => return None,
            Err(TaskFileError::Open(error)) => {
                crate::sys::set_errno(error);
                let name = path.to_string_lossy();
                log::sys_warning(c"Server::task_unpack", format_args!("open({name}) failed"));
                return None;
            }
            Err(TaskFileError::Pread { error, fd, read }) => {
                crate::sys::set_errno(error);
                log::sys_warning(
                    c"Server::task_unpack",
                    format_args!("pread({fd}, {str:p}, {}, {read}) failed", length - read),
                );
                return None;
            }
        }
        Some((buffer.view(), flags))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DataHead, PacketTask, ServerEvent, TaskBuffer, TaskFileError, TaskPack, pack_payload,
        read_file, spill,
    };
    use crate::process::methods::tests::{install, logged};
    use std::ffi::CString;

    fn read(path: &CString, length: usize, peek: bool) -> (Result<(), TaskFileError>, Vec<u8>) {
        let mut buffer = vec![0; length];
        let outcome = read_file(path, &mut buffer, peek);
        (outcome, buffer)
    }

    /// Upstream's `file.tmp_file`, second half: `make_tmpfile` with `task_tmpfile`
    /// in a missing directory (core-tests sets the template directly) makes no file.
    #[test]
    #[cfg_attr(miri, ignore = "Miri has no mkostemp")]
    fn make_tmpfile_in_a_missing_directory() {
        let mut path = [0; super::TASK_TMP_PATH_SIZE];
        assert!(super::make_tmpfile(c"/tmp/not-exists-dir/test.XXXXXX", &mut path).is_none());
    }

    #[test]
    fn packed_bytes_hold_their_head_and_the_payload_it_counts() {
        let head = size_of::<crate::DataHead>();
        // One byte ahead, so the head is unaligned.
        let mut bytes = vec![0u8; 1 + head + 3];
        bytes[1..9].copy_from_slice(&7i64.to_ne_bytes());
        let len = 1 + std::mem::offset_of!(crate::DataHead, len);
        bytes[len..len + 4].copy_from_slice(&3u32.to_ne_bytes());
        bytes[1 + head..].copy_from_slice(b"abc");
        let task = super::Packed::from_bytes(&bytes[1..]).unwrap();
        assert_eq!((task.head().fd, task.head().len), (7, 3));
        let mut string = crate::buffer::string::StringBuffer::EMPTY;
        let (payload, flags) = task.unpack(&mut string).unwrap();
        assert_eq!((payload, flags), (&b"abc"[..], 0));
        assert!(super::Packed::from_bytes(&bytes[1..bytes.len() - 1]).is_none());
        assert!(super::Packed::from_bytes(&bytes[1..17]).is_none());
        // A truncated head counting no payload is still too short.
        assert!(super::Packed::from_bytes(&[0; 24]).is_none());
    }

    #[test]
    fn reads_then_unlinks_unless_peeking() {
        let file = std::env::temp_dir().join(format!("swr-task-{}", std::process::id()));
        std::fs::write(&file, b"payload").unwrap();
        let path = CString::new(file.to_str().unwrap()).unwrap();

        let (outcome, data) = read(&path, 7, true);
        assert_eq!((outcome, data.as_slice()), (Ok(()), &b"payload"[..]));
        assert!(file.exists());

        let (outcome, _) = read(&path, 9, false);
        assert_eq!(outcome, Err(TaskFileError::Short { read: 7 }));
        assert!(file.exists());

        let (outcome, _) = read(&path, 7, false);
        assert_eq!(outcome, Ok(()));
        assert!(!file.exists());

        let (outcome, _) = read(&path, 7, false);
        assert_eq!(outcome, Err(TaskFileError::Open(libc::ENOENT)));
    }

    #[test]
    fn a_task_buffer_is_upstreams_event_data() {
        // Upstream's `sizeof(EventData)`: SW_IPC_MAX_SIZE (8192, 2048 on macOS).
        assert_eq!(
            size_of::<TaskBuffer>() as u64,
            crate::process::process_pool::SW_IPC_MAX_SIZE
        );
        assert_eq!(align_of::<TaskBuffer>(), 8);
    }

    #[test]
    fn a_task_buffer_counts_the_payload_it_packs() {
        let _turn = methods::TASK_ID.lock();
        let mut buf = TaskBuffer::default();
        assert_eq!(
            buf.packed().bytes().len(),
            size_of::<DataHead>(),
            "an empty buffer is its head"
        );
        assert!(buf.pack(b"abc"));
        buf.add_flags(1 << 2);
        let bytes = buf.packed().bytes();
        assert_eq!(bytes.len(), size_of::<DataHead>() + 3);
        assert_eq!(&bytes[size_of::<DataHead>()..], b"abc");
        assert_eq!(
            (buf.head().len, buf.head().event(), buf.head().ext_flags),
            (3, Some(ServerEvent::Task), 1 << 2)
        );
    }

    #[test]
    fn packs_a_small_payload_inline() {
        let data = b"a small task payload";
        let mut dest = vec![std::mem::MaybeUninit::new(0u8); 300];
        let template = CString::new("/unused/XXXXXX").unwrap();
        let out = pack_payload(&mut dest, data, || &template);
        assert_eq!(
            out,
            TaskPack {
                ok: true,
                tmpfile: false,
                info_len: data.len()
            }
        );
        let dest: Vec<u8> = dest.iter().map(|b| unsafe { b.assume_init() }).collect();
        assert_eq!(&dest[..data.len()], data);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no mkostemp")]
    fn packs_a_large_payload_to_a_temp_file() {
        let template = CString::new(
            std::env::temp_dir()
                .join(format!("swr-pack-{}-XXXXXX", std::process::id()))
                .to_str()
                .unwrap(),
        )
        .unwrap();
        let payload = vec![0xAB_u8; 500];
        let mut dest = vec![std::mem::MaybeUninit::new(0u8); 300]; // smaller than the payload, so it spills to a file
        let out = pack_payload(&mut dest, &payload, || &template);
        assert_eq!(
            out,
            TaskPack {
                ok: true,
                tmpfile: true,
                info_len: size_of::<PacketTask>()
            }
        );
        // Decode the PacketTask { length, tmpfile } written into dest by byte offset.
        let dest: Vec<u8> = dest.iter().map(|b| unsafe { b.assume_init() }).collect();
        let length = usize::from_ne_bytes(dest[..size_of::<usize>()].try_into().unwrap());
        assert_eq!(length, payload.len());
        let path_bytes = &dest[size_of::<usize>()..];
        let nul = path_bytes.iter().position(|&b| b == 0).unwrap();
        let path = std::str::from_utf8(&path_bytes[..nul]).unwrap();
        assert_eq!(std::fs::read(path).unwrap(), payload);
        std::fs::remove_file(path).unwrap_or_default();
    }

    fn pack_large(template: &CString) -> TaskPack {
        let payload = vec![0xAB_u8; 500];
        let mut dest = vec![std::mem::MaybeUninit::new(0u8); 300];
        pack_payload(&mut dest, &payload, || template)
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no mkostemp")]
    fn a_failed_mkstemp_warns_as_swoole_tmpfile_and_nothing_more() {
        install();
        let template = CString::new("/swr-no-such-dir/task-XXXXXX").unwrap();
        assert_eq!(pack_large(&template), TaskPack::default());
        // mkstemp may have filled in the Xs before failing, as upstream logs it.
        let warned = logged("swoole_tmpfile");
        assert_eq!(warned.len(), 1);
        assert_eq!((warned[0].0, warned[0].2), (0, libc::ENOENT));
        assert!(warned[0].3.starts_with("mkstemp('/swr-no-such-dir/task-"));
        assert!(warned[0].3.ends_with("') failed"));
        assert_eq!(
            warned[0].3.len(),
            "mkstemp('/swr-no-such-dir/task-XXXXXX') failed".len()
        );
        assert_eq!(logged("Server::task_pack"), []);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no mkostemp")]
    fn an_over_long_template_is_cut_and_mkstemp_warns() {
        install();
        let long = format!("/swr-no-such-dir/{}XXXXXX", "a".repeat(300));
        let template = CString::new(long.clone()).unwrap();
        assert_eq!(pack_large(&template), TaskPack::default());
        // The cut pattern has no Xs left: EINVAL on Linux, ENOENT where the
        // directory is checked first.
        let warned = logged("swoole_tmpfile");
        assert_eq!(warned.len(), 1);
        assert!([libc::EINVAL, libc::ENOENT].contains(&warned[0].2));
        assert_eq!(warned[0].3, format!("mkstemp('{}') failed", &long[..255]));
    }

    #[test]
    fn a_failed_write_warns_pwrite_then_the_task() {
        use std::os::fd::AsRawFd;
        install();
        let path = std::env::temp_dir().join(format!("swr-spill-{}", std::process::id()));
        let created = std::fs::File::create_new(&path).unwrap();
        let name = CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        let guard = crate::ossupport::TempFile::from_parts(name, created)
            .map_file(|_| std::fs::File::open(&path).unwrap());
        let fd = guard.file.as_raw_fd();
        let data = [1u8; 4];
        assert!(!spill(&guard.file, &data));
        drop(guard);
        assert_eq!(crate::sys::errno(), libc::EBADF);
        let warned = logged("File::write_all");
        assert_eq!(warned.len(), 1);
        assert_eq!((warned[0].0, warned[0].2), (0, libc::EBADF));
        assert_eq!(
            warned[0].3,
            format!("pwrite({fd}, {:p}, 4, 0) failed", data.as_ptr())
        );
        assert_eq!(
            logged("Server::task_pack"),
            [(1, 0, 0, "write to tmpfile failed".into())]
        );
        assert!(!path.try_exists().unwrap());
    }

    mod methods {
        use super::super::*;
        use crate::{swr_v1_core_current_task_id, swr_v1_core_set_current_task_id};

        /// `task_pack` draws from the process-wide task id, so its tests take turns.
        pub(super) static TASK_ID: std::sync::Mutex<()> = std::sync::Mutex::new(());

        #[test]
        fn a_small_task_packs_inline_and_unpacks_in_place() {
            let _turn = TASK_ID.lock();
            let mut task = TaskBuffer::default();
            swr_v1_core_set_current_task_id(41);
            assert!(task.pack(b"hello"));
            let head = task.head();
            assert_eq!(
                (head.fd, head.len, head.kind, head.ext_flags),
                (41, 5, 8, 0)
            );
            assert!(head.time > 0.0);
            assert_eq!(swr_v1_core_current_task_id(), 42);
            let mut string = crate::buffer::string::OwnedString::default();
            let (data, _) = task.packed().unpack(&mut string).unwrap();
            assert_eq!(data.len(), 5);
            assert_eq!(data.as_ptr(), task.data.as_ptr().cast());
            assert!(string.as_ptr().is_null());
            let mut request = *task.head();
            request.ext_flags = TaskFlag::SW_TASK_NONBLOCK.bits();
            request.reserved = 12345;
            assert!(task.pack_result(b"done", &request));
            assert_eq!(
                DataHead::from_bytes(&task.head().to_bytes()).reserved,
                12345
            );
            assert_eq!(task.head().fd, 41);
            assert_eq!(task.head().event(), Some(ServerEvent::Finish));
            assert_eq!(swr_v1_core_current_task_id(), 42);
        }

        #[test]
        #[cfg_attr(miri, ignore = "Miri has no access")]
        fn a_large_task_spills_to_a_file_that_unpacking_reads_and_removes() {
            let _turn = TASK_ID.lock();
            assert!(crate::set_task_tmpdir(c"/tmp"));
            let data = vec![7u8; 9000];
            let mut task = TaskBuffer::default();
            assert!(task.pack(&data));
            let head = task.head();
            assert_eq!(
                (head.len as usize, head.ext_flags),
                (size_of::<PacketTask>(), TaskFlag::SW_TASK_TMPFILE.bits())
            );
            let (_, path) = PacketTask::read(task.packed().payload()).unwrap();
            let path = path.to_str().unwrap().to_owned();
            assert!(path.starts_with("/tmp/swoole.task."));
            let mut string = crate::buffer::string::OwnedString::default();
            let (read, _) = task.packed().unpack(&mut string).unwrap();
            assert_eq!(read, data.as_slice());
            let read = read.as_ptr();
            assert_eq!(read, string.as_ptr());
            assert_eq!((string.len(), string.size()), (9000, 9000));
            assert!(!std::path::Path::new(&path).exists());
            // A second unpack finds no file.
            assert!(task.packed().unpack(&mut string).is_none());
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/modules/process/task_packets.rs"]
mod originals;
