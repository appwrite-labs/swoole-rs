//! A server task's payload: `Server::task_pack` puts it inline, or in a temporary
//! file when it is too large for the pipe; `Server::task_unpack` reads it back
//! (`File::read_all`: pread from offset 0, retrying EINTR/EAGAIN, stopping at EOF or
//! another error) and unlinks the file unless the task is only peeked;
//! `Server::task_dump` prints it.
use std::ffi::{CStr, c_char, c_int, c_long, c_void};
use std::ptr;

use swoole_rs_corebase::constants::TaskFlag;
use swoole_rs_corebase::{
    datahead_dump, log, microtime, swr_v1_core_current_task_id, swr_v1_core_set_current_task_id,
};

use swoole_rs_buffer::string::CppString;
use swoole_rs_ossupport::{fd_write_all, swoole_tmpfile};

use crate::process_pool::SW_IPC_MAX_SIZE;
use crate::worker_globals::swr_v1_worker_global_get;

/// What reading a task's temporary file did.
#[repr(C)]
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

/// Reads `length` bytes of the file at `path` into `buffer`.
///
/// # Safety
/// `path` is a NUL-terminated string and `buffer` is writable for `length` bytes.
pub unsafe fn swr_v1_process_task_file_read(
    path: *const c_char,
    buffer: *mut u8,
    length: usize,
    peek: bool,
) -> TaskFileRead {
    // SAFETY: the caller passes a NUL-terminated path.
    let path = unsafe { CStr::from_ptr(path) };
    // SAFETY: `path` is NUL-terminated.
    let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDONLY) };
    if fd < 0 {
        return TaskFileRead {
            status: 1,
            error: errno(),
            fd,
            read: 0,
        };
    }
    let mut outcome = TaskFileRead {
        fd,
        ..TaskFileRead::default()
    };
    while outcome.read < length {
        let Ok(offset) = libc::off_t::try_from(outcome.read) else {
            break;
        };
        // SAFETY: `buffer` is writable for `length` bytes and `read < length`.
        let rest = unsafe { buffer.add(outcome.read) };
        // SAFETY: `rest` is writable for `length - read` bytes and `fd` is open.
        let n = unsafe { libc::pread(fd, rest.cast(), length - outcome.read, offset) };
        if let Ok(n @ 1..) = usize::try_from(n) {
            outcome.read += n;
            continue;
        }
        let error = errno();
        if n < 0 && [libc::EINTR, libc::EAGAIN, libc::EWOULDBLOCK].contains(&error) {
            continue;
        }
        (outcome.status, outcome.error) = if n == 0 { (2, 0) } else { (3, error) };
        break;
    }
    // SAFETY: `fd` is open and owned here.
    unsafe { libc::close(fd) };
    if outcome.status == 0 && outcome.read == length && !peek {
        // SAFETY: `path` is NUL-terminated.
        unsafe { libc::unlink(path.as_ptr()) };
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

/// What packing a task payload did.
#[repr(C)]
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

/// `Server::task_pack`'s payload placement: inline into `dest` when it fits in
/// `dest_cap`, otherwise a temp file created from `template` (an mkstemp pattern),
/// whose path and length are written into `dest` as a `PacketTask`. Native builds
/// the `EventData` header and sets its `len`/flag from the result; the Zend
/// serialization of the PHP value stays native.
///
/// # Safety
/// `dest` is writable for `dest_cap` bytes (at least `sizeof(PacketTask)` for the
/// temp-file case); `data` lends `len` readable bytes; `template` is a
/// NUL-terminated mkstemp pattern.
pub unsafe fn swr_v1_process_task_pack(
    dest: *mut u8,
    dest_cap: usize,
    data: *const u8,
    len: usize,
    template: *const c_char,
) -> TaskPack {
    if len < dest_cap {
        // SAFETY: `dest` lends `dest_cap > len` writable bytes; `data` lends `len`.
        unsafe { std::ptr::copy_nonoverlapping(data, dest, len) };
        return TaskPack {
            ok: true,
            tmpfile: false,
            info_len: len,
        };
    }
    let mut pkg = PacketTask {
        length: len,
        tmpfile: [0; TASK_TMP_PATH_SIZE],
    };
    // SAFETY: `template` is a NUL-terminated string.
    let template = unsafe { CStr::from_ptr(template) };
    let fd = make_tmpfile(template, &mut pkg.tmpfile);
    if fd < 0 {
        return TaskPack::default();
    }
    // SAFETY: `data` lends `len` bytes.
    let wrote = spill(fd, unsafe { std::slice::from_raw_parts(data, len) });
    // `fd` is open and owned here.
    swoole_rs_corebase::sys::close(fd);
    if !wrote {
        return TaskPack::default();
    }
    // SAFETY: `dest` lends `dest_cap` (>= sizeof(PacketTask)) bytes; copy the POD's bytes.
    unsafe {
        std::ptr::copy_nonoverlapping((&raw const pkg).cast::<u8>(), dest, size_of::<PacketTask>());
    }
    TaskPack {
        ok: true,
        tmpfile: true,
        info_len: size_of::<PacketTask>(),
    }
}

/// `swoole::EventData`'s header (`DataHead`, include/swoole.h), which native
/// asserts is 40 bytes; the payload follows it.
#[repr(C)]
pub struct EventHead {
    fd: c_long,
    msg_id: u64,
    len: u32,
    reactor_id: i16,
    kind: u8,
    flags: u8,
    server_fd: u16,
    ext_flags: u16,
    reserved: u32,
    time: f64,
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

fn payload(task: *mut EventHead) -> *mut u8 {
    task.wrapping_add(1).cast()
}

/// `Server::task_dump`: the header, and the temporary file's name if it has one.
///
/// # Safety
/// `task` is a live `EventData`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole6Server9task_dumpEPNS_9EventDataE")
)]
pub unsafe extern "C" fn task_dump(task: *mut EventHead) {
    let mut buf = [0 as c_char; 1024];
    // SAFETY: `task` starts with its header; `buf` is writable for its length.
    unsafe { datahead_dump(task.cast(), buf.as_mut_ptr(), buf.len()) };
    // SAFETY: `dump` NUL-terminated `buf`.
    unsafe { libc::printf(c"%s".as_ptr(), buf.as_ptr()) };
    // SAFETY: the caller lends a live task.
    if unsafe { (*task).ext_flags } & TMPFILE != 0 {
        // SAFETY: as above.
        let pkg = unsafe { packet_task(task) };
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

/// A tmpfile task's `PacketTask`, copied as upstream does: all but its last
/// byte, so the name stays NUL-terminated.
///
/// # Safety
/// `task` is a live `EventData`.
unsafe fn packet_task(task: *mut EventHead) -> PacketTask {
    let mut pkg = PacketTask {
        length: 0,
        tmpfile: [0; TASK_TMP_PATH_SIZE],
    };
    // SAFETY: the payload holds at least `sizeof(PacketTask)` bytes.
    unsafe {
        ptr::copy_nonoverlapping(
            payload(task),
            (&raw mut pkg).cast::<u8>(),
            size_of::<PacketTask>() - 1,
        );
    }
    pkg
}

/// `Server::task_pack`: a task header from this worker, with `data` inline or
/// in a temporary file.
///
/// # Safety
/// `task` is a writable `EventData`; `data` lends `length` readable bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole6Server9task_packEPNS_9EventDataEPKvm")
)]
pub unsafe extern "C" fn task_pack(
    task: *mut EventHead,
    data: *const c_void,
    length: usize,
) -> bool {
    let id = swr_v1_core_current_task_id();
    swr_v1_core_set_current_task_id(id.wrapping_add(1));
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the worker id, narrowed to DataHead's int16_t reactor_id as upstream does"
    )]
    let reactor_id = swr_v1_worker_global_get(0) as i16;
    let head = EventHead {
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
    // SAFETY: the caller lends a writable task.
    unsafe { task.write(head) };
    let capacity = usize::try_from(SW_IPC_MAX_SIZE).unwrap_or(0) - size_of::<EventHead>();
    let template = task_tmpfile().as_ptr();
    // SAFETY: the payload holds `capacity` bytes; `data` lends `length`.
    let packed =
        unsafe { swr_v1_process_task_pack(payload(task), capacity, data.cast(), length, template) };
    if !packed.ok {
        return false;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "at most sizeof(PacketTask) or the pipe's capacity, in DataHead's uint32_t len"
    )]
    let len = packed.info_len as u32;
    // SAFETY: as above.
    unsafe { (*task).len = len };
    if packed.tmpfile {
        // SAFETY: as above.
        unsafe { (*task).ext_flags |= TMPFILE };
    }
    true
}

/// `Server::task_unpack`: the task's payload, read into `buffer` from its
/// temporary file if it has one.
///
/// # Safety
/// `task` is a live `EventData`, `buffer` a live `String` and `packet` writable.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(
        export_name = "_ZN6swoole6Server11task_unpackEPNS_9EventDataEPNS_6StringEPNS_9PacketPtrE"
    )
)]
pub unsafe extern "C" fn task_unpack(
    task: *mut EventHead,
    buffer: *mut CppString,
    packet: *mut PacketPtr,
) -> bool {
    // SAFETY: the caller lends a live task.
    let flags = unsafe { (*task).ext_flags };
    if flags & TMPFILE == 0 {
        // SAFETY: as above.
        let length = unsafe { (*task).len } as usize;
        // SAFETY: the caller lends a writable packet.
        unsafe {
            packet.write(PacketPtr {
                length,
                data: payload(task).cast(),
            });
        }
        return true;
    }
    // SAFETY: as above.
    let pkg = unsafe { packet_task(task) };
    // SAFETY: the caller lends a live String.
    if unsafe { (*buffer).size } < pkg.length {
        // SAFETY: as above.
        unsafe { &mut *buffer }.extend(pkg.length);
    }
    // SAFETY: as above.
    let str = unsafe { (*buffer).str };
    let path = pkg.tmpfile.as_ptr().cast::<c_char>();
    // SAFETY: `str` holds at least `pkg.length` bytes; `path` is NUL-terminated.
    let read =
        unsafe { swr_v1_process_task_file_read(path, str.cast(), pkg.length, flags & PEEK != 0) };
    if read.status != 0 {
        if read.error != 0 {
            // SAFETY: this thread's errno.
            unsafe { *errno_location() = read.error };
        }
        // SAFETY: `path` is NUL-terminated.
        let name = unsafe { CStr::from_ptr(path) }.to_string_lossy();
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
        return false;
    }
    // SAFETY: the caller lends a live String.
    unsafe { (*buffer).length = pkg.length };
    // SAFETY: the caller lends a writable packet.
    unsafe {
        packet.write(PacketPtr {
            length: pkg.length,
            data: str.cast(),
        });
    }
    true
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
    let task = task.cast::<EventHead>();
    let mut packet = PacketPtr {
        length: 0,
        data: ptr::null_mut(),
    };
    // SAFETY: per the caller.
    if !unsafe { task_unpack(task, buffer.cast(), &raw mut packet) } {
        return None;
    }
    // SAFETY: as above.
    let flags = unsafe { (*task).ext_flags };
    Some((packet.data, packet.length, flags))
}

#[cfg(target_os = "linux")]
fn errno_location() -> *mut c_int {
    // SAFETY: always callable; returns this thread's errno.
    unsafe { libc::__errno_location() }
}

#[cfg(not(target_os = "linux"))]
fn errno_location() -> *mut c_int {
    // SAFETY: always callable; returns this thread's errno.
    unsafe { libc::__error() }
}

#[cfg(test)]
mod tests {
    use super::{
        PacketTask, TaskFileRead, TaskPack, spill, swr_v1_process_task_file_read,
        swr_v1_process_task_pack,
    };
    use crate::methods::tests::{install, logged};
    use std::ffi::CString;

    fn read(path: &CString, length: usize, peek: bool) -> (TaskFileRead, Vec<u8>) {
        let mut buffer = vec![0; length];
        let outcome = unsafe {
            swr_v1_process_task_file_read(path.as_ptr(), buffer.as_mut_ptr(), length, peek)
        };
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
        let out = unsafe {
            swr_v1_process_task_pack(
                dest.as_mut_ptr(),
                dest.len(),
                data.as_ptr(),
                data.len(),
                template.as_ptr(),
            )
        };
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
        let mut dest = vec![0u8; 300]; // smaller than the payload, so it spills to a file
        let out = unsafe {
            swr_v1_process_task_pack(
                dest.as_mut_ptr(),
                dest.len(),
                payload.as_ptr(),
                payload.len(),
                template.as_ptr(),
            )
        };
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
        unsafe {
            swr_v1_process_task_pack(
                dest.as_mut_ptr(),
                dest.len(),
                payload.as_ptr(),
                payload.len(),
                template.as_ptr(),
            )
        }
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
            let mut packet = PacketPtr {
                length: 0,
                data: ptr::null_mut(),
            };
            assert!(unsafe { task_unpack(task, &raw mut string, &raw mut packet) });
            assert_eq!(packet.length, 5);
            assert_eq!(packet.data, payload(task).cast());
            assert!(string.str.is_null());
        }

        #[test]
        #[cfg_attr(miri, ignore = "Miri has no access")]
        fn a_large_task_spills_to_a_file_that_unpacking_reads_and_removes() {
            let _turn = TASK_ID.lock();
            assert!(unsafe { swoole_rs_corebase::swr_v1_core_set_task_tmpdir(c"/tmp".as_ptr()) });
            let data = vec![7u8; 9000];
            let mut buf = event();
            let task = buf.as_mut_ptr().cast::<EventHead>();
            assert!(unsafe { task_pack(task, data.as_ptr().cast(), data.len()) });
            let head = unsafe { &*task };
            assert_eq!(
                (head.len as usize, head.ext_flags),
                (size_of::<PacketTask>(), TMPFILE)
            );
            let pkg = unsafe { packet_task(task) };
            let path = unsafe { CStr::from_ptr(pkg.tmpfile.as_ptr().cast()) }
                .to_str()
                .unwrap()
                .to_owned();
            assert!(path.starts_with("/tmp/swoole.task."));
            let mut string = empty_string();
            let mut packet = PacketPtr {
                length: 0,
                data: ptr::null_mut(),
            };
            assert!(unsafe { task_unpack(task, &raw mut string, &raw mut packet) });
            assert_eq!(
                (string.length, string.size, packet.length),
                (9000, 9000, 9000)
            );
            assert_eq!(packet.data, string.str.cast());
            let read = unsafe { std::slice::from_raw_parts(packet.data.cast::<u8>(), 9000) };
            assert_eq!(read, data.as_slice());
            assert!(!std::path::Path::new(&path).exists());
            // A second unpack finds no file.
            assert!(!unsafe { task_unpack(task, &raw mut string, &raw mut packet) });
        }
    }
}
