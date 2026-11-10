//! A server task's payload: `Server::task_pack` puts it inline, or in a temporary
//! file when it is too large for the pipe; `Server::task_unpack` reads it back
//! (`File::read_all`: pread from offset 0, retrying EINTR/EAGAIN, stopping at EOF or
//! another error) and unlinks the file unless the task is only peeked;
//! `Server::task_dump` prints it.
use std::ffi::{CStr, c_char, c_int, c_void};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::FileExt;

use swoole_rs_corebase::constants::TaskFlag;
use swoole_rs_corebase::{
    DataHead, datahead_dump, log, microtime, swr_v1_core_current_task_id,
    swr_v1_core_set_current_task_id,
};

use swoole_rs_buffer::string::CppString;
use swoole_rs_ossupport::{fd_write_all, swoole_tmpfile};

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
    fn write(&self, to: &mut [u8]) {
        let (length, name) = to.split_at_mut(size_of::<usize>());
        length.copy_from_slice(&self.length.to_ne_bytes());
        if let Some(name) = name.get_mut(..TASK_TMP_PATH_SIZE) {
            name.copy_from_slice(&self.tmpfile);
        }
    }

    /// A tmpfile task's `PacketTask`, copied as upstream does: all but its last
    /// byte, so the name stays NUL-terminated.
    fn read(from: &[u8]) -> Self {
        let mut pkg = Self {
            length: 0,
            tmpfile: [0; TASK_TMP_PATH_SIZE],
        };
        let (length, name) = from.split_at(size_of::<usize>());
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
    // SAFETY: `path` is a writable NUL-terminated template.
    unsafe { swoole_tmpfile(path.as_mut_ptr().cast()) }
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
fn pack_payload(dest: &mut [u8], data: &[u8], template: &CStr) -> TaskPack {
    let fits = data.len() < dest.len();
    if let Some(inline) = dest.get_mut(..data.len()).filter(|_| fits) {
        inline.copy_from_slice(data);
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

/// `swoole::PacketPtr`.
#[repr(C)]
pub struct PacketPtr {
    length: usize,
    data: *mut c_char,
}

/// `SwooleG.task_tmpfile`, the template of a task's temporary file, which each
/// caller copies at once.
pub(crate) fn task_tmpfile() -> &'static CStr {
    // SAFETY: the NUL-terminated path, changed only by `swoole_init` and the
    // `task_tmpdir` setting.
    unsafe { CStr::from_ptr(swoole_rs_corebase::swr_v1_core_task_tmpfile()) }
}

/// `SW_SERVER_EVENT_TASK`.
const EVENT_TASK: u8 = 8;
const TMPFILE: u16 = TaskFlag::SW_TASK_TMPFILE as u16;
const PEEK: u16 = TaskFlag::SW_TASK_PEEK as u16;

impl EventData {
    /// `Server::task_dump`: the header, and the temporary file's name if it has one.
    pub fn dump(&self) {
        let mut buf = [0 as c_char; 1024];
        // SAFETY: a live header; `buf` is writable for its length.
        unsafe { datahead_dump(&raw const self.head, buf.as_mut_ptr(), buf.len()) };
        // SAFETY: `dump` NUL-terminated `buf`.
        unsafe { libc::printf(c"%s".as_ptr(), buf.as_ptr()) };
        if self.head.ext_flags & TMPFILE != 0 {
            let pkg = PacketTask::read(&self.data);
            // Upstream prints the payload's length in bytes of the file's name.
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_possible_wrap,
                reason = "printf's int precision, as upstream's (int) cast"
            )]
            let precision = pkg.length as c_int;
            // SAFETY: the name is NUL-terminated.
            unsafe {
                libc::printf(
                    c"Task[tmpfile]=%.*s\n".as_ptr(),
                    precision,
                    pkg.tmpfile.as_ptr().cast::<c_char>(),
                )
            };
        }
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
        let packed = pack_payload(&mut self.data, data, task_tmpfile());
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

    /// `Server::task_unpack`: the task's payload, read into `buffer` from its
    /// temporary file if it has one; `None` (warned) if that can't be read.
    pub fn unpack(&mut self, buffer: &mut CppString) -> Option<PacketPtr> {
        let flags = self.head.ext_flags;
        if flags & TMPFILE == 0 {
            return Some(PacketPtr {
                length: self.head.len as usize,
                data: self.data.as_mut_ptr().cast(),
            });
        }
        let pkg = PacketTask::read(&self.data);
        if buffer.size < pkg.length {
            buffer.extend(pkg.length);
        }
        let str = buffer.str;
        let target: &mut [u8] = if pkg.length == 0 {
            &mut []
        } else {
            // SAFETY: a live String of `size` (at least `pkg.length > 0`) bytes at `str`.
            unsafe { std::slice::from_raw_parts_mut(str, pkg.length) }
        };
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
        buffer.length = pkg.length;
        Some(PacketPtr {
            length: pkg.length,
            data: str.cast(),
        })
    }
}

/// [`EventData::pack`] into the task at `task`.
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
    // SAFETY: as the caller promises.
    unsafe { &mut *task.cast::<EventData>() }.pack(data)
}

/// `Server::task_unpack(task, sw_tg_buffer(), &packet)`: the payload's bytes
/// and length, and the task's `ext_flags`; `None` if its temporary file can't be read.
///
/// # Safety
/// `task` is a live `EventData` and `buffer` this thread's live `String`.
pub unsafe fn unpack(
    task: *mut c_void,
    buffer: *mut c_void,
) -> Option<(*const c_char, usize, u16)> {
    // SAFETY: per the caller.
    let task = unsafe { &mut *task.cast::<EventData>() };
    // SAFETY: as above.
    let packet = task.unpack(unsafe { &mut *buffer.cast::<CppString>() })?;
    Some((packet.data, packet.length, task.head.ext_flags))
}

#[cfg(test)]
mod tests {
    use super::{PacketTask, TaskFileRead, TaskPack, pack_payload, read_file, spill};
    use crate::methods::tests::{install, logged};
    use std::ffi::CString;

    fn read(path: &CString, length: usize, peek: bool) -> (TaskFileRead, Vec<u8>) {
        let mut buffer = vec![0; length];
        let outcome = read_file(path, &mut buffer, peek);
        (outcome, buffer)
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
    fn packs_a_small_payload_inline() {
        let data = b"a small task payload";
        let mut dest = vec![0u8; 300];
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
        assert_eq!(&dest[..data.len()], data);
    }

    #[test]
    fn packs_a_large_payload_to_a_temp_file() {
        let template = CString::new(
            std::env::temp_dir()
                .join(format!("swr-pack-{}-XXXXXX", std::process::id()))
                .to_str()
                .unwrap(),
        )
        .unwrap();
        let payload = vec![0xAB_u8; 500];
        let mut dest = vec![0u8; 300]; // smaller than the payload, so it spills to a file
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
        let mut dest = vec![0u8; 300];
        pack_payload(&mut dest, &payload, template)
    }

    #[test]
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
            let packet = unsafe { &mut *event }.unpack(&mut string).unwrap();
            assert_eq!(packet.length, 5);
            assert_eq!(packet.data, unsafe { (*event).data.as_mut_ptr() }.cast());
            assert!(string.str.is_null());
        }

        #[test]
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
            let packet = unsafe { &mut *event }.unpack(&mut string).unwrap();
            assert_eq!(
                (string.length, string.size, packet.length),
                (9000, 9000, 9000)
            );
            assert_eq!(packet.data, string.str.cast());
            let read = unsafe { std::slice::from_raw_parts(packet.data.cast::<u8>(), 9000) };
            assert_eq!(read, data.as_slice());
            assert!(!std::path::Path::new(&path).exists());
            // A second unpack finds no file.
            assert!(unsafe { &mut *event }.unpack(&mut string).is_none());
        }
    }
}
