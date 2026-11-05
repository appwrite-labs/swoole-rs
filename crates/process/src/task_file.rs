//! A server task's payload: `Server::task_pack` puts it inline, or in a temporary
//! file when it is too large for the pipe; `Server::task_unpack` reads it back
//! (`File::read_all`: pread from offset 0, retrying EINTR/EAGAIN, stopping at EOF or
//! another error) and unlinks the file unless the task is only peeked;
//! `Server::task_dump` prints it. Native lends the file template and grows the
//! receiving `String` ([`TaskNative`]).
use std::ffi::{CStr, c_char, c_int, c_long, c_void};
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};

use swoole_rs_corebase::constants::TaskFlag;
use swoole_rs_corebase::{
    datahead_dump, log, microtime, swr_v1_core_current_task_id, swr_v1_core_set_current_task_id,
};

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

fn errno() -> i32 {
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
const TASK_TMP_PATH_SIZE: usize = 256;

/// The payload descriptor written into `EventData::data` when a task spills to a
/// temp file (matches `swoole::PacketTask`).
#[repr(C)]
struct PacketTask {
    length: usize,
    tmpfile: [u8; TASK_TMP_PATH_SIZE],
}

/// What packing a task payload did.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TaskPack {
    /// false on a temp-file create or write failure.
    pub ok: bool,
    /// true when the payload spilled to a temp file and `dest` holds a `PacketTask`.
    pub tmpfile: bool,
    /// The byte count for `info.len`: the payload length, or `sizeof(PacketTask)`.
    pub info_len: usize,
}

fn write_all(fd: c_int, data: *const u8, len: usize) -> bool {
    let mut written = 0;
    while written < len {
        // SAFETY: `data` lends `len` readable bytes and `written < len`.
        let rest = unsafe { data.add(written) };
        // SAFETY: `fd` is open; `rest` lends `len - written` readable bytes.
        let n = unsafe { libc::write(fd, rest.cast(), len - written) };
        if let Ok(n @ 1..) = usize::try_from(n) {
            written += n;
            continue;
        }
        let error = errno();
        if n < 0 && [libc::EINTR, libc::EAGAIN, libc::EWOULDBLOCK].contains(&error) {
            continue;
        }
        return false;
    }
    true
}

/// mkstemp (mkostemp on Linux, matching `swoole_tmpfile`) over the pattern in `buf`,
/// which is rewritten in place to the created path. Returns the fd, or negative.
fn make_tmpfile(buf: &mut [u8; TASK_TMP_PATH_SIZE]) -> c_int {
    let pattern = buf.as_mut_ptr().cast::<c_char>();
    #[cfg(target_os = "linux")]
    // SAFETY: `pattern` is a NUL-terminated mkstemp pattern buffer in `buf`.
    let fd = unsafe { libc::mkostemp(pattern, libc::O_WRONLY | libc::O_CREAT) };
    #[cfg(not(target_os = "linux"))]
    // SAFETY: `pattern` is a NUL-terminated mkstemp pattern buffer in `buf`.
    let fd = unsafe { libc::mkstemp(pattern) };
    fd
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
    let pattern = unsafe { CStr::from_ptr(template) }.to_bytes();
    if pattern.len() >= pkg.tmpfile.len() {
        return TaskPack::default();
    }
    for (byte, &pattern) in pkg.tmpfile.iter_mut().zip(pattern) {
        *byte = pattern;
    }
    let fd = make_tmpfile(&mut pkg.tmpfile);
    if fd < 0 {
        return TaskPack::default();
    }
    let wrote = write_all(fd, data, len);
    // SAFETY: `fd` is open and owned here.
    unsafe { libc::close(fd) };
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

/// `swoole::String`'s fields.
#[repr(C)]
pub struct StringRecord {
    length: usize,
    size: usize,
    offset: libc::off_t,
    str: *mut c_char,
    allocator: *const c_void,
}

/// `swoole::PacketPtr`.
#[repr(C)]
pub struct PacketPtr {
    length: usize,
    data: *mut c_char,
}

/// What the task methods below borrow from native: `SwooleG.task_tmpfile` and
/// `String::extend`.
#[repr(C)]
pub struct TaskNative {
    pub tmpfile: unsafe extern "C" fn() -> *const c_char,
    pub extend: unsafe extern "C" fn(string: *mut StringRecord, size: usize),
}

static NATIVE: AtomicPtr<TaskNative> = AtomicPtr::new(ptr::null_mut());

/// Registers native's table once at load.
///
/// # Safety
/// `native` lives as long as the process.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_process_task_set_native(native: *const TaskNative) {
    NATIVE.store(native.cast_mut(), Ordering::Release);
}

fn native() -> &'static TaskNative {
    // SAFETY: native registers its static table before any server runs a task.
    unsafe { &*NATIVE.load(Ordering::Acquire) }
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
    // SAFETY: native's table returns `SwooleG.task_tmpfile`'s NUL-terminated text.
    let template = unsafe { (native().tmpfile)() };
    // SAFETY: the payload holds `capacity` bytes; `data` lends `length`.
    let packed =
        unsafe { swr_v1_process_task_pack(payload(task), capacity, data.cast(), length, template) };
    if !packed.ok {
        log::warning(
            c"Server::task_pack",
            format_args!("write to tmpfile failed"),
        );
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
    buffer: *mut StringRecord,
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
        // SAFETY: as above; `String::extend` reallocates it.
        unsafe { (native().extend)(buffer, pkg.length) };
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
            data: str,
        });
    }
    true
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
        PacketTask, TaskFileRead, TaskPack, swr_v1_process_task_file_read, swr_v1_process_task_pack,
    };
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

    mod methods {
        use super::super::*;

        unsafe extern "C" fn tmpfile() -> *const c_char {
            c"/tmp/swr-task-method-XXXXXX".as_ptr()
        }
        unsafe extern "C" fn extend(string: *mut StringRecord, size: usize) {
            let grown = Box::leak(vec![0 as c_char; size].into_boxed_slice());
            unsafe {
                (*string).str = grown.as_mut_ptr();
                (*string).size = size;
            }
        }
        static NATIVE_TABLE: TaskNative = TaskNative { tmpfile, extend };

        /// An `EventData`-sized, 8-aligned buffer.
        fn event() -> Vec<u64> {
            vec![0; 8192 / 8 + 8]
        }

        fn empty_string() -> StringRecord {
            StringRecord {
                length: 0,
                size: 0,
                offset: 0,
                str: ptr::null_mut(),
                allocator: ptr::null(),
            }
        }

        #[test]
        fn a_small_task_packs_inline_and_unpacks_in_place() {
            unsafe { swr_v1_process_task_set_native(&raw const NATIVE_TABLE) };
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
        fn a_large_task_spills_to_a_file_that_unpacking_reads_and_removes() {
            unsafe { swr_v1_process_task_set_native(&raw const NATIVE_TABLE) };
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
            assert!(path.starts_with("/tmp/swr-task-method-"));
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
            assert_eq!(packet.data, string.str);
            let read = unsafe { std::slice::from_raw_parts(packet.data.cast::<u8>(), 9000) };
            assert_eq!(read, data.as_slice());
            assert!(!std::path::Path::new(&path).exists());
            // A second unpack finds no file.
            assert!(!unsafe { task_unpack(task, &raw mut string, &raw mut packet) });
        }
    }
}
