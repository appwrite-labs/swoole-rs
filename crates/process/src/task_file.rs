//! A server task's payload: `Server::task_pack` puts it inline, or in a temporary
//! file when it is too large for the pipe; `Server::task_unpack` reads it back
//! (`File::read_all`: pread from offset 0, retrying EINTR/EAGAIN, stopping at EOF or
//! another error) and unlinks the file unless the task is only peeked;
//! `Server::task_dump` prints it.
use std::ffi::{CStr, c_int, c_void};
use std::marker::PhantomData;
use std::mem::{MaybeUninit, offset_of};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::FileExt;

use swoole_rs_corebase::constants::TaskFlag;
use swoole_rs_corebase::{
    DataHead, datahead_dump, log, microtime, swr_v1_core_current_task_id,
    swr_v1_core_set_current_task_id, task_tmpfile,
};

use swoole_rs_buffer::string::CppString;
use swoole_rs_ossupport::fd_write_all;

use crate::process_pool::SW_IPC_MAX_SIZE;
use crate::worker_globals::swr_v1_worker_global_get;

/// What reading a task's temporary file did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TaskFileRead {
    /// 0 read and (unless peeking) unlinked; 1 open failed; 2 short read at EOF;
    /// 3 pread failed after `read` bytes.
    pub status: u32,
    /// The errno of status 1 and 3.
    pub error: i32,
    /// The descriptor that status 3's pread used.
    pub fd: i32,
    /// Bytes read.
    pub read: usize,
}

pub(crate) fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// Fills `buffer` from the start of the file at `path`, and unlinks the file once
/// it is read whole, unless `peek`.
fn read_file(path: &CStr, buffer: &mut [u8], peek: bool) -> TaskFileRead {
    let path = std::ffi::OsStr::from_bytes(path.to_bytes());
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) => {
            return TaskFileRead {
                status: 1,
                error: error.raw_os_error().unwrap_or(0),
                fd: -1,
                read: 0,
            };
        }
    };
    let mut outcome = TaskFileRead {
        fd: file.as_raw_fd(),
        ..TaskFileRead::default()
    };
    while let Some(rest) = buffer
        .get_mut(outcome.read..)
        .filter(|rest| !rest.is_empty())
    {
        match file.read_at(rest, outcome.read as u64) {
            Ok(0) => {
                outcome.status = 2;
                break;
            }
            Ok(n) => outcome.read += n,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock
                ) => {}
            Err(error) => {
                (outcome.status, outcome.error) = (3, error.raw_os_error().unwrap_or(0));
                break;
            }
        }
    }
    drop(file);
    if outcome.status == 0 && !peek {
        std::fs::remove_file(path).ok();
    }
    outcome
}

/// `SW_TASK_TMP_PATH_SIZE`.
pub(crate) const TASK_TMP_PATH_SIZE: usize = 256;

/// The payload descriptor written into `EventData::data` when a task spills to a
/// temp file (upstream's `swoole::PacketTask`).
#[repr(C)]
struct PacketTask {
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

    /// A tmpfile task's `PacketTask`, copied as upstream does: all but its last
    /// byte, so the name stays NUL-terminated.
    fn read(from: &[u8]) -> Self {
        let mut pkg = Self {
            length: 0,
            tmpfile: [0; TASK_TMP_PATH_SIZE],
        };
        let (length, name) = from
            .split_at_checked(size_of::<usize>())
            .unwrap_or((from, &[]));
        if let Ok(length) = length.try_into() {
            pkg.length = usize::from_ne_bytes(length);
        }
        for (byte, &from) in pkg
            .tmpfile
            .iter_mut()
            .take(TASK_TMP_PATH_SIZE - 1)
            .zip(name)
        {
            *byte = from;
        }
        pkg
    }

    /// The temporary file's path.
    fn path(&self) -> &CStr {
        CStr::from_bytes_until_nul(&self.tmpfile).unwrap_or_default()
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
/// and created there by `swoole_tmpfile`, which warns on failure. Returns the fd,
/// or negative.
pub(crate) fn make_tmpfile(template: &CStr, path: &mut [u8; TASK_TMP_PATH_SIZE]) -> c_int {
    *path = [0; TASK_TMP_PATH_SIZE];
    for (byte, &pattern) in path
        .iter_mut()
        .take(TASK_TMP_PATH_SIZE - 1)
        .zip(template.to_bytes())
    {
        *byte = pattern;
    }
    swoole_rs_ossupport::create_tmpfile(path)
}

/// Writes the payload to the temp file as `File::write_all` does (warning on an
/// IO error), then warns `write to tmpfile failed` on a short write.
fn spill(fd: c_int, data: &[u8]) -> bool {
    let wrote = fd_write_all(fd, false, data) == data.len();
    if !wrote {
        log::warning(
            c"Server::task_pack",
            format_args!("write to tmpfile failed"),
        );
    }
    wrote
}

/// `Server::task_pack`'s payload placement: inline into `dest` when it fits,
/// otherwise a temp file created from `template` (an mkstemp pattern), whose
/// path and length are written into `dest` (at least `sizeof(PacketTask)` bytes)
/// as a `PacketTask`.
fn pack_payload(dest: &mut [MaybeUninit<u8>], data: &[u8], template: &CStr) -> TaskPack {
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
    let fd = make_tmpfile(template, &mut pkg.tmpfile);
    if fd < 0 {
        return TaskPack::default();
    }
    let wrote = spill(fd, data);
    // `fd` is open and owned here.
    swoole_rs_corebase::sys::close(fd);
    if !wrote {
        return TaskPack::default();
    }
    pkg.write(dest);
    TaskPack {
        ok: true,
        tmpfile: true,
        info_len: size_of::<PacketTask>(),
    }
}

/// `swoole::EventData`'s header (`DataHead`), which native asserts is 40 bytes.
pub use swoole_rs_corebase::DataHead as EventHead;

#[expect(
    clippy::cast_possible_truncation,
    reason = "SW_IPC_MAX_SIZE is a few KiB"
)]
const IPC_PAYLOAD: usize = SW_IPC_MAX_SIZE as usize - size_of::<DataHead>();

/// `swoole::EventData`: the header, then the payload one pipe message holds.
#[repr(C)]
pub struct EventData {
    head: DataHead,
    data: [u8; IPC_PAYLOAD],
}

/// An `EventData` to pack a task into. Its payload starts uninitialized, as a
/// stack `swoole::EventData` does, and only [`Self::pack`] sets the head's `len`,
/// so the bytes it counts are always written.
#[repr(C)]
pub struct TaskBuffer {
    head: DataHead,
    data: [MaybeUninit<u8>; IPC_PAYLOAD],
}

const _: () = assert!(
    size_of::<DataHead>() == 40,
    "`TaskBuffer::bytes` reads the head as bytes, so it has no padding"
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

    /// `info.type = kind`.
    pub fn set_kind(&mut self, kind: u8) {
        self.head.kind = kind;
    }

    /// The buffer, for an operation that reads `EventData::size()` bytes of it.
    #[must_use]
    pub fn as_mut_ptr(&mut self) -> *mut c_void {
        std::ptr::from_mut(self).cast()
    }

    /// `EventData::size()` bytes: the head, then the payload it counts.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        let size = size_of::<DataHead>() + self.head.len as usize;
        // SAFETY: the head has no padding, and `pack`, the one writer of `len`,
        // wrote the `len` payload bytes after it, within the buffer.
        unsafe { std::slice::from_raw_parts(std::ptr::from_ref(self).cast::<u8>(), size) }
    }

    /// `Server::task_pack`: a task header from this worker, with `data` inline or
    /// in a temporary file.
    pub fn pack(&mut self, data: &[u8]) -> bool {
        let id = swr_v1_core_current_task_id();
        swr_v1_core_set_current_task_id(id.wrapping_add(1));
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the worker id, narrowed to DataHead's int16_t reactor_id as upstream does"
        )]
        let reactor_id = swr_v1_worker_global_get(0) as i16;
        self.head = DataHead {
            fd: id,
            msg_id: 0,
            len: 0,
            reactor_id,
            kind: EVENT_TASK,
            flags: 0,
            server_fd: 0,
            ext_flags: 0,
            reserved: 0,
            time: microtime(),
        };
        let packed = pack_payload(&mut self.data, data, &task_tmpfile());
        if !packed.ok {
            return false;
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "at most sizeof(PacketTask) or the pipe's capacity, in DataHead's uint32_t len"
        )]
        let len = packed.info_len as u32;
        self.head.len = len;
        if packed.tmpfile {
            self.head.ext_flags |= TMPFILE;
        }
        true
    }
}

/// `SW_SERVER_EVENT_TASK`.
const EVENT_TASK: u8 = 8;
const TMPFILE: u16 = TaskFlag::SW_TASK_TMPFILE as u16;
const PEEK: u16 = TaskFlag::SW_TASK_PEEK as u16;

impl EventData {
    /// `Server::task_dump`: the header, and the temporary file's name if it has one.
    pub fn dump(&self) {
        let mut buf = [0_u8; 1024];
        datahead_dump(&self.head, &mut buf);
        let text = CStr::from_bytes_until_nul(&buf).unwrap_or_default();
        swoole_rs_corebase::sys::print(text.to_bytes());
        if self.head.ext_flags & TMPFILE != 0 {
            let pkg = PacketTask::read(&self.data);
            // Upstream prints the payload's length in bytes of the file's name; a
            // negative `(int)` precision prints all of it.
            let name = pkg.path().to_bytes();
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_possible_wrap,
                reason = "printf's int precision, as upstream's (int) cast"
            )]
            let precision = pkg.length as c_int;
            let shown = usize::try_from(precision).map_or(name.len(), |n| n.min(name.len()));
            let name = name.get(..shown).unwrap_or(name);
            let line = [b"Task[tmpfile]=", name, b"\n"].concat();
            swoole_rs_corebase::sys::print(&line);
        }
    }
}

/// [`TaskBuffer::pack`] into the task at `task`.
///
/// # Safety
/// `task` is a writable `EventData`; `data` lends `length` readable bytes.
pub unsafe fn task_pack(task: *mut EventHead, data: *const c_void, length: usize) -> bool {
    let data = if length == 0 {
        &[]
    } else {
        // SAFETY: as the caller promises.
        unsafe { std::slice::from_raw_parts(data.cast::<u8>(), length) }
    };
    // SAFETY: as the caller promises; a `TaskBuffer` is an `EventData` whose
    // payload may be uninitialized.
    unsafe { &mut *task.cast::<TaskBuffer>() }.pack(data)
}

/// A packed task lent for `'a`: its `DataHead`, at any alignment, then the `len`
/// bytes it counts (the payload, or a temporary file's `PacketTask`).
/// [`Packed::from_raw`] is the one unsafe step. It is laid out as the pointer, so a
/// server slot fn takes one where native code passes its `EventData *`.
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct Packed<'a> {
    raw: *const u8,
    life: PhantomData<&'a [u8]>,
}

impl<'a> Packed<'a> {
    /// The task at `raw`.
    ///
    /// # Safety
    /// `raw` holds a `DataHead` and then its `len` bytes, readable for `'a`; nothing
    /// writes them while a method of this handle reads them.
    #[must_use]
    pub unsafe fn from_raw(raw: *const c_void) -> Self {
        Self {
            raw: raw.cast(),
            life: PhantomData,
        }
    }

    /// The task at the start of `bytes`; `None` when they are too short for its
    /// head and the `len` bytes it counts.
    #[must_use]
    pub fn from_bytes(bytes: &'a [u8]) -> Option<Self> {
        let len = bytes
            .get(offset_of!(DataHead, len)..)
            .and_then(<[u8]>::first_chunk)
            .map(|len| u32::from_ne_bytes(*len) as usize)?;
        let payload = bytes.len().checked_sub(size_of::<DataHead>())?;
        (payload >= len).then_some(Self {
            raw: bytes.as_ptr(),
            life: PhantomData,
        })
    }

    /// The task's address, for the native-shaped calls that still take it.
    #[must_use]
    pub fn as_ptr(self) -> *const c_void {
        self.raw.cast()
    }

    /// The task's head.
    #[must_use]
    pub fn head(self) -> DataHead {
        // SAFETY: a head, per `from_raw`'s contract or `from_bytes`'s check; a
        // record in a result file's bytes may be unaligned.
        unsafe { self.raw.cast::<DataHead>().read_unaligned() }
    }

    /// The task's head and the `len` bytes after it.
    #[must_use]
    pub fn bytes(self) -> &'a [u8] {
        let length = size_of::<DataHead>() + self.head().len as usize;
        // SAFETY: as above, with the head's `len` bytes after it.
        unsafe { std::slice::from_raw_parts(self.raw, length) }
    }

    fn payload(self) -> &'a [u8] {
        self.bytes()
            .get(size_of::<DataHead>()..)
            .unwrap_or_default()
    }

    /// `Server::task_unpack(task, buffer, &packet)`: the payload's bytes, read
    /// into `buffer` from its temporary file if it has one, and the task's
    /// `ext_flags`; `None` (warned) if that file can't be read.
    pub fn unpack<'b>(self, buffer: &'b mut CppString) -> Option<(&'b [u8], u16)>
    where
        'a: 'b,
    {
        let (payload, flags) = (self.payload(), self.head().ext_flags);
        if flags & TMPFILE == 0 {
            return Some((payload, flags));
        }
        let pkg = PacketTask::read(payload);
        if buffer.size() < pkg.length {
            buffer.extend(pkg.length);
        }
        let str = buffer.as_mut_ptr();
        // Zeroed first: past the String's content, its allocation is uninitialized. It
        // holds `pkg.length` bytes by now.
        let target = buffer.zeroed(pkg.length).unwrap_or_default();
        let read = read_file(pkg.path(), target, flags & PEEK != 0);
        if read.status != 0 {
            if read.error != 0 {
                swoole_rs_corebase::sys::set_errno(read.error);
            }
            let name = pkg.path().to_string_lossy();
            if read.status == 1 {
                log::sys_warning(c"Server::task_unpack", format_args!("open({name}) failed"));
            } else if read.status == 3 {
                log::sys_warning(
                    c"Server::task_unpack",
                    format_args!(
                        "pread({}, {str:p}, {}, {}) failed",
                        read.fd,
                        pkg.length - read.read,
                        read.read
                    ),
                );
            }
            return None;
        }
        // SAFETY: the read filled the buffer's first `pkg.length` bytes.
        unsafe { buffer.set_len(pkg.length) };
        Some((buffer.view(), flags))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DataHead, EVENT_TASK, PacketTask, TaskBuffer, TaskFileRead, TaskPack, pack_payload,
        read_file, spill,
    };
    use crate::methods::tests::{install, logged};
    use std::ffi::CString;

    fn read(path: &CString, length: usize, peek: bool) -> (TaskFileRead, Vec<u8>) {
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
        assert!(super::make_tmpfile(c"/tmp/not-exists-dir/test.XXXXXX", &mut path) < 0);
    }

    #[test]
    fn packed_bytes_hold_their_head_and_the_payload_it_counts() {
        let head = size_of::<swoole_rs_corebase::DataHead>();
        // One byte ahead, so the head is unaligned.
        let mut bytes = vec![0u8; 1 + head + 3];
        bytes[1..9].copy_from_slice(&7i64.to_ne_bytes());
        let len = 1 + std::mem::offset_of!(swoole_rs_corebase::DataHead, len);
        bytes[len..len + 4].copy_from_slice(&3u32.to_ne_bytes());
        bytes[1 + head..].copy_from_slice(b"abc");
        let task = super::Packed::from_bytes(&bytes[1..]).unwrap();
        assert_eq!((task.head().fd, task.head().len), (7, 3));
        let mut string = swoole_rs_buffer::string::CppString::EMPTY;
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
        assert_eq!(
            (outcome.status, outcome.read, data.as_slice()),
            (0, 7, &b"payload"[..])
        );
        assert!(file.exists());

        let (outcome, _) = read(&path, 9, false);
        assert_eq!((outcome.status, outcome.read), (2, 7));
        assert!(file.exists());

        let (outcome, _) = read(&path, 7, false);
        assert_eq!(outcome.status, 0);
        assert!(!file.exists());

        let (outcome, _) = read(&path, 7, false);
        assert_eq!((outcome.status, outcome.error), (1, libc::ENOENT));
    }

    #[test]
    fn a_task_buffer_is_upstreams_event_data() {
        // Upstream's `sizeof(EventData)`: SW_IPC_MAX_SIZE (8192, 2048 on macOS).
        assert_eq!(
            size_of::<TaskBuffer>() as u64,
            crate::process_pool::SW_IPC_MAX_SIZE
        );
        assert_eq!(align_of::<TaskBuffer>(), 8);
    }

    #[test]
    fn a_task_buffer_counts_the_payload_it_packs() {
        let mut buf = TaskBuffer::default();
        assert_eq!(
            buf.bytes().len(),
            size_of::<DataHead>(),
            "an empty buffer is its head"
        );
        assert!(buf.pack(b"abc"));
        buf.add_flags(1 << 2);
        let bytes = buf.bytes();
        assert_eq!(bytes.len(), size_of::<DataHead>() + 3);
        assert_eq!(&bytes[size_of::<DataHead>()..], b"abc");
        assert_eq!(
            (buf.head().len, buf.head().kind, buf.head().ext_flags),
            (3, EVENT_TASK, 1 << 2)
        );
    }

    #[test]
    fn packs_a_small_payload_inline() {
        let data = b"a small task payload";
        let mut dest = vec![std::mem::MaybeUninit::new(0u8); 300];
        let template = CString::new("/unused/XXXXXX").unwrap();
        let out = pack_payload(&mut dest, data, &template);
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
        let out = pack_payload(&mut dest, &payload, &template);
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
        std::fs::remove_file(path).ok();
    }

    fn pack_large(template: &CString) -> TaskPack {
        let payload = vec![0xAB_u8; 500];
        let mut dest = vec![std::mem::MaybeUninit::new(0u8); 300];
        pack_payload(&mut dest, &payload, template)
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
        install();
        let fd = unsafe { libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY) };
        let data = [1u8; 4];
        assert!(!spill(fd, &data));
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
        unsafe { libc::close(fd) };
    }

    mod methods {
        use super::super::*;

        /// `task_pack` draws from the process-wide task id, so its tests take turns.
        static TASK_ID: std::sync::Mutex<()> = std::sync::Mutex::new(());

        /// An `EventData`-sized, 8-aligned buffer.
        fn event() -> Vec<u64> {
            vec![0; 8192 / 8 + 8]
        }

        fn empty_string() -> CppString {
            CppString::EMPTY
        }

        #[test]
        fn a_small_task_packs_inline_and_unpacks_in_place() {
            let _turn = TASK_ID.lock();
            let mut buf = event();
            let task = buf.as_mut_ptr().cast::<EventHead>();
            let event = task.cast::<EventData>();
            swr_v1_core_set_current_task_id(41);
            assert!(unsafe { task_pack(task, c"hello".as_ptr().cast(), 5) });
            let head = unsafe { &*task };
            assert_eq!(
                (head.fd, head.len, head.kind, head.ext_flags),
                (41, 5, 8, 0)
            );
            assert!(head.time > 0.0);
            assert_eq!(swr_v1_core_current_task_id(), 42);
            let mut string = empty_string();
            let (data, _) = unsafe { Packed::from_raw(event.cast()) }
                .unpack(&mut string)
                .unwrap();
            assert_eq!(data.len(), 5);
            assert_eq!(data.as_ptr(), unsafe { (*event).data.as_ptr() }.cast());
            assert!(string.as_ptr().is_null());
        }

        #[test]
        #[cfg_attr(miri, ignore = "Miri has no access")]
        fn a_large_task_spills_to_a_file_that_unpacking_reads_and_removes() {
            let _turn = TASK_ID.lock();
            assert!(unsafe { swoole_rs_corebase::swr_v1_core_set_task_tmpdir(c"/tmp".as_ptr()) });
            let data = vec![7u8; 9000];
            let mut buf = event();
            let task = buf.as_mut_ptr().cast::<EventHead>();
            let event = task.cast::<EventData>();
            assert!(unsafe { task_pack(task, data.as_ptr().cast(), data.len()) });
            let head = unsafe { &*task };
            assert_eq!(
                (head.len as usize, head.ext_flags),
                (size_of::<PacketTask>(), TMPFILE)
            );
            let pkg = PacketTask::read(unsafe { &(*event).data });
            let path = pkg.path().to_str().unwrap().to_owned();
            assert!(path.starts_with("/tmp/swoole.task."));
            let mut string = empty_string();
            let (read, _) = unsafe { Packed::from_raw(event.cast()) }
                .unpack(&mut string)
                .unwrap();
            assert_eq!(read, data.as_slice());
            let read = read.as_ptr();
            assert_eq!(read, string.as_ptr());
            assert_eq!((string.len(), string.size()), (9000, 9000));
            assert!(!std::path::Path::new(&path).exists());
            // A second unpack finds no file.
            assert!(
                unsafe { Packed::from_raw(event.cast()) }
                    .unpack(&mut string)
                    .is_none()
            );
        }
    }
}
