//! Upstream's `core-tests/src/coroutine/hook.cpp`, one function per TEST with the
//! same inputs and expected values, over the hybrid's C ABI `swoole_coroutine_*`
//! exports. Upstream's test helpers (`File`, `String`, `create_socket_pair`,
//! `file_get_contents`, `Address::assign`) are written out here over libc.

use super::{
    AF_INET, AF_UNIX, EINVAL, LOCK_EX, LOCK_NB, LOCK_SH, LOCK_UN, O_CREAT, O_NONBLOCK, O_RDONLY,
    O_WRONLY, SO_RCVBUF, SO_SNDBUF, SOCK_STREAM, SOL_SOCKET, SW_ERR, SW_EVENT_WRITE, TEST_FILE,
    TEST_STR, co_sleep, errno, go, last_error, random_bytes, run, swoole_coroutine_close,
    swoole_coroutine_flock, swoole_coroutine_open, swoole_coroutine_read, swoole_coroutine_write,
};
use std::cell::Cell;
use std::ffi::{CStr, c_char, c_int, c_long, c_void};
use std::time::Instant;

// glibc's values (the core-tests build is Linux).
const O_RDWR: c_int = 2;
const O_TRUNC: c_int = 0o1000;
const SEEK_SET: c_int = 0;
const R_OK: c_int = 4;
const F_OK: c_int = 0;
const F_GETFL: c_int = 3;
const F_SETFL: c_int = 4;
const SO_SNDTIMEO: c_int = 21;
const SO_RCVTIMEO: c_int = 20;
const POLLIN: i16 = 1;
const POLLOUT: i16 = 4;
const AI_PASSIVE: c_int = 1;
const EAI_NONAME: c_int = -2;
const HOST_NOT_FOUND: c_int = 1;
const ENOTSOCK: c_int = 88;
const EWOULDBLOCK: c_int = 11;
const ECANCELED: c_int = 125;
const EAFNOSUPPORT: c_int = 97;
const SOCK_CLOEXEC: c_int = 0o2000000;
const SW_OK: c_int = 0;
const SW_ERROR_INVALID_PARAMS: c_int = 505;
const SW_ERROR_CO_SOCKET_CLOSE_WAIT: c_int = 10019;
/// `struct stat`'s `st_size` offset, in 8-byte words, on x86_64 and aarch64 Linux.
pub(super) const ST_SIZE: usize = 6;
/// `struct dirent`'s `d_name` offset on Linux.
const D_NAME: usize = 19;

const HOST_1: &CStr = c"www.baidu.com";
const HOST_2: &CStr = c"www.xxxxxxxxxxxxxxxxxxxxx00000xxxxxxxxx----not_found.com";
const TEST_TMP_FILE: &CStr = c"/tmp/swoole_core_test_file";
const TEST_TMP_DIR: &CStr = c"/tmp/swoole_core_test_dir";
const TEST_LOG_FILE: &CStr = c"/tmp/swoole.log";
const TEST_JPG_FILE: &str = "/examples/test.jpg";

/// Large enough for glibc's `struct stat` and `struct statvfs`; zeroed, so
/// comparing whole buffers compares the structs.
pub(super) type Stat = [u64; 32];

#[repr(C)]
struct PollFd {
    fd: c_int,
    events: i16,
    revents: i16,
}

#[repr(C)]
struct AddrInfo {
    flags: c_int,
    family: c_int,
    socktype: c_int,
    protocol: c_int,
    addrlen: u32,
    addr: *mut c_void,
    canonname: *mut c_char,
    next: *mut AddrInfo,
}

#[repr(C)]
struct IoVec {
    base: *mut c_void,
    len: usize,
}

#[repr(C)]
struct MsgHdr {
    name: *mut c_void,
    namelen: u32,
    iov: *mut IoVec,
    iovlen: usize,
    control: *mut c_void,
    controllen: usize,
    flags: c_int,
}

#[repr(C)]
struct SockAddrIn {
    family: u16,
    port: u16,
    addr: [u8; 4],
    zero: [u8; 8],
}

/// `swoole::Protocol`, as the coroutine socket's record holds it.
#[repr(C)]
struct Protocol {
    split_by_eof: bool,
    package_eof: [u8; 8],
    package_eof_len: u8,
    package_length_type: u8,
    package_length_size: u8,
    package_length_offset: u16,
    package_body_offset: u16,
    package_max_length: u32,
    callbacks: [*mut c_void; 5],
}

/// The head of the hybrid's coroutine `Socket`, which is `swr_v1_co_socket`
/// (native/hooks/swoole_rs_co_socket.h), up to `sock_fd`.
#[repr(C)]
struct CoSocket {
    err_code: c_int,
    err_msg: *const c_char,
    open_length_check: bool,
    open_eof_check: bool,
    http2: bool,
    protocol: Protocol,
    kind: c_int,
    socket: *mut c_void,
    sock_domain: c_int,
    sock_type: c_int,
    sock_protocol: c_int,
    sock_fd: c_int,
}

unsafe extern "C" {
    fn swoole_coroutine_sleep(sec: c_int);
    pub(super) fn swoole_coroutine_usleep(usec: c_int);
    fn swoole_coroutine_access(path: *const c_char, mode: c_int) -> c_int;
    pub(super) fn swoole_coroutine_lseek(fd: c_int, offset: i64, whence: c_int) -> i64;
    pub(super) fn swoole_coroutine_fstat(fd: c_int, buf: *mut Stat) -> c_int;
    fn swoole_coroutine_stat(path: *const c_char, buf: *mut Stat) -> c_int;
    fn swoole_coroutine_lstat(path: *const c_char, buf: *mut Stat) -> c_int;
    fn swoole_coroutine_readlink(path: *const c_char, buf: *mut c_char, len: usize) -> isize;
    fn swoole_coroutine_unlink(path: *const c_char) -> c_int;
    fn swoole_coroutine_mkdir(path: *const c_char, mode: u32) -> c_int;
    fn swoole_coroutine_rmdir(path: *const c_char) -> c_int;
    fn swoole_coroutine_rename(from: *const c_char, to: *const c_char) -> c_int;
    fn swoole_coroutine_statvfs(path: *const c_char, buf: *mut Stat) -> c_int;
    pub(super) fn swoole_coroutine_fsync(fd: c_int) -> c_int;
    fn swoole_coroutine_fdatasync(fd: c_int) -> c_int;
    pub(super) fn swoole_coroutine_ftruncate(fd: c_int, length: i64) -> c_int;
    fn swoole_coroutine_fopen(path: *const c_char, mode: *const c_char) -> *mut c_void;
    fn swoole_coroutine_fdopen(fd: c_int, mode: *const c_char) -> *mut c_void;
    fn swoole_coroutine_freopen(
        path: *const c_char,
        mode: *const c_char,
        stream: *mut c_void,
    ) -> *mut c_void;
    fn swoole_coroutine_fread(ptr: *mut c_void, size: usize, n: usize, fp: *mut c_void) -> usize;
    fn swoole_coroutine_fwrite(ptr: *const c_void, size: usize, n: usize, fp: *mut c_void)
    -> usize;
    fn swoole_coroutine_fgets(s: *mut c_char, size: c_int, fp: *mut c_void) -> *mut c_char;
    fn swoole_coroutine_fputs(s: *const c_char, fp: *mut c_void) -> c_int;
    fn swoole_coroutine_fflush(fp: *mut c_void) -> c_int;
    fn swoole_coroutine_feof(fp: *mut c_void) -> c_int;
    fn swoole_coroutine_fclose(fp: *mut c_void) -> c_int;
    fn swoole_coroutine_opendir(name: *const c_char) -> *mut c_void;
    fn swoole_coroutine_readdir(dir: *mut c_void) -> *const c_char;
    fn swoole_coroutine_closedir(dir: *mut c_void) -> c_int;
    fn swoole_coroutine_socket(domain: c_int, kind: c_int, protocol: c_int) -> c_int;
    fn swoole_coroutine_socket_create(fd: c_int) -> c_int;
    fn swoole_coroutine_socket_unwrap(fd: c_int) -> c_int;
    fn swoole_coroutine_socket_exists(fd: c_int) -> u8;
    fn swoole_coroutine_send(fd: c_int, buf: *const c_void, len: usize, flags: c_int) -> isize;
    fn swoole_coroutine_sendmsg(fd: c_int, msg: *const MsgHdr, flags: c_int) -> isize;
    fn swoole_coroutine_recv(fd: c_int, buf: *mut c_void, len: usize, flags: c_int) -> isize;
    fn swoole_coroutine_recvmsg(fd: c_int, msg: *mut MsgHdr, flags: c_int) -> isize;
    fn swoole_coroutine_connect(fd: c_int, addr: *const SockAddrIn, len: u32) -> c_int;
    fn swoole_coroutine_accept(fd: c_int, addr: *mut SockAddrIn, len: *mut u32) -> c_int;
    fn swoole_coroutine_poll(fds: *mut PollFd, n: u64, timeout: c_int) -> c_int;
    fn swoole_coroutine_poll_fake(fds: *mut PollFd, n: u64, timeout: c_int) -> c_int;
    fn swoole_coroutine_socket_set_timeout(fd: c_int, which: c_int, timeout: f64) -> c_int;
    fn swoole_coroutine_socket_set_connect_timeout(fd: c_int, timeout: f64) -> c_int;
    fn swoole_coroutine_socket_wait_event(fd: c_int, event: c_int, timeout: f64) -> c_int;
    fn swoole_coroutine_getaddrinfo(
        name: *const c_char,
        service: *const c_char,
        req: *const AddrInfo,
        pai: *mut *mut AddrInfo,
    ) -> c_int;
    fn swoole_coroutine_gethostbyname(name: *const c_char) -> *mut c_void;
    #[link_name = "_Z34swoole_coroutine_get_socket_objecti"]
    fn swoole_coroutine_get_socket_object(fd: c_int) -> *mut CoSocket;
    #[link_name = "_Z37swoole_coroutine_get_socket_object_exi"]
    fn swoole_coroutine_get_socket_object_ex(fd: c_int) -> *mut CoSocket;
    pub(super) fn swr_v1_system_gethostbyname(
        host: *const c_char,
        length: usize,
        family: c_long,
        timeout: f64,
        put: super::Put,
        out: *mut c_void,
    ) -> bool;
}

unsafe extern "C" {
    fn socketpair(domain: c_int, kind: c_int, protocol: c_int, fds: *mut c_int) -> c_int;
    fn setsockopt(fd: c_int, level: c_int, name: c_int, value: *const c_void, length: u32)
    -> c_int;
    fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
    fn open(path: *const c_char, flags: c_int, ...) -> c_int;
    fn close(fd: c_int) -> c_int;
    fn unlink(path: *const c_char) -> c_int;
    fn access(path: *const c_char, mode: c_int) -> c_int;
    fn fstat(fd: c_int, buf: *mut Stat) -> c_int;
    fn statvfs(path: *const c_char, buf: *mut Stat) -> c_int;
    fn opendir(name: *const c_char) -> *mut c_void;
    fn readdir(dir: *mut c_void) -> *const c_char;
    fn closedir(dir: *mut c_void) -> c_int;
    fn freeaddrinfo(ai: *mut AddrInfo);
    fn bind(fd: c_int, addr: *const SockAddrIn, len: u32) -> c_int;
    fn listen(fd: c_int, backlog: c_int) -> c_int;
    fn getsockname(fd: c_int, addr: *mut SockAddrIn, len: *mut u32) -> c_int;
    fn __h_errno_location() -> *mut c_int;
}

fn ms_since(start: Instant) -> u128 {
    start.elapsed().as_millis()
}

/// `readdir`'s entry name.
fn d_name(entry: *const c_char) -> Vec<u8> {
    assert!(!entry.is_null());
    // SAFETY: a live dirent; d_name is NUL-terminated.
    unsafe { CStr::from_ptr(entry.byte_add(D_NAME)) }
        .to_bytes()
        .to_vec()
}

fn file_size(path: &CStr) -> i64 {
    i64::try_from(std::fs::metadata(path.to_str().unwrap()).unwrap().len()).unwrap()
}

fn file_get_contents(path: &CStr) -> Vec<u8> {
    std::fs::read(path.to_str().unwrap()).unwrap()
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn set_nonblock(fd: c_int) {
    // SAFETY: fcntl(2) on a live descriptor.
    unsafe { fcntl(fd, F_SETFL, fcntl(fd, F_GETFL) | O_NONBLOCK) };
}

/// core-tests' `create_socket_pair`: two `Socket(fd, SW_SOCK_UNIX_STREAM)`s over a
/// stream socketpair (nonblocking), each with `set_buffer_size(65536)`. The cases
/// take the descriptors (`move_fd`, `get_fd`).
fn create_socket_pair() -> [c_int; 2] {
    let mut pair = [-1; 2];
    // SAFETY: two writable descriptors.
    unsafe { socketpair(AF_UNIX, SOCK_STREAM, 0, pair.as_mut_ptr()) };
    for fd in pair {
        set_nonblock(fd);
        for name in [SO_SNDBUF, SO_RCVBUF] {
            let size: u32 = 65536;
            // SAFETY: an int option on a live descriptor.
            unsafe { setsockopt(fd, SOL_SOCKET, name, (&raw const size).cast(), 4) };
        }
    }
    pair
}

/// `sw_tg_buffer()` after `clear()` and `append_random_bytes(256 * 1024, false)`.
fn random_buffer() -> Vec<u8> {
    let mut buffer = vec![0u8; 256 * 1024];
    assert_eq!(random_bytes(&mut buffer), buffer.len());
    buffer
}

fn write_fd(fd: c_int, buf: &[u8]) -> isize {
    // SAFETY: `buf` is readable for its length.
    unsafe { swoole_coroutine_write(fd, buf.as_ptr().cast(), buf.len()) }
}

fn read_fd(fd: c_int, buf: &mut [u8]) -> isize {
    // SAFETY: `buf` is writable for its length.
    unsafe { swoole_coroutine_read(fd, buf.as_mut_ptr().cast(), buf.len()) }
}

fn co_open(path: &CStr, flags: c_int, mode: u32) -> c_int {
    // SAFETY: a NUL-terminated path.
    unsafe { swoole_coroutine_open(path.as_ptr(), flags, mode) }
}

fn co_close(fd: c_int) -> c_int {
    // SAFETY: closes `fd`.
    unsafe { swoole_coroutine_close(fd) }
}

fn co_flock(fd: c_int, operation: c_int) -> c_int {
    // SAFETY: flock(2) on `fd`.
    unsafe { swoole_coroutine_flock(fd, operation) }
}

fn co_unlink(path: &CStr) -> c_int {
    // SAFETY: a NUL-terminated path.
    unsafe { swoole_coroutine_unlink(path.as_ptr()) }
}

fn plain_unlink(path: &CStr) {
    // SAFETY: a NUL-terminated path.
    unsafe { unlink(path.as_ptr()) };
}

fn plain_access(path: &CStr, mode: c_int) -> c_int {
    // SAFETY: a NUL-terminated path.
    unsafe { access(path.as_ptr(), mode) }
}

fn socket_create(fd: c_int) -> c_int {
    // SAFETY: wraps `fd`.
    unsafe { swoole_coroutine_socket_create(fd) }
}

fn fputs(s: &CStr, fp: *mut c_void) -> c_int {
    // SAFETY: a NUL-terminated string and an open stream.
    unsafe { swoole_coroutine_fputs(s.as_ptr(), fp) }
}

fn test_file_hook() {
    let mut buf = [0u8; 8192];
    let n_buf = buf.len();
    assert_eq!(random_bytes(&mut buf), n_buf);

    let fd = co_open(TEST_FILE, O_WRONLY | O_TRUNC | O_CREAT, 0o666);
    assert_eq!(write_fd(fd, &buf), isize::try_from(n_buf).unwrap());

    // SAFETY: an open descriptor.
    assert_eq!(unsafe { swoole_coroutine_fsync(fd) }, 0);
    // SAFETY: as above.
    assert_eq!(unsafe { swoole_coroutine_fdatasync(fd) }, 0);

    co_close(fd);

    let fd = co_open(TEST_FILE, O_RDONLY, 0);
    let mut data = [0u8; 8192];
    assert_eq!(read_fd(fd, &mut data), isize::try_from(n_buf).unwrap());
    assert_eq!(buf, data);
    co_close(fd);

    let mut statbuf: Stat = [0; 32];
    // SAFETY: a NUL-terminated path and a buffer larger than `struct stat`.
    unsafe { swoole_coroutine_stat(TEST_FILE.as_ptr(), &mut statbuf) };

    // File f1(test_file, File::RW)
    let f1_size = file_size(TEST_FILE);
    assert_eq!(statbuf[ST_SIZE] as i64, f1_size);

    let mut statbuf2: Stat = [0; 32];
    // SAFETY: as above.
    unsafe { swoole_coroutine_lstat(TEST_FILE.as_ptr(), &mut statbuf2) };
    assert_eq!(statbuf2[ST_SIZE] as i64, f1_size);

    assert_eq!(co_unlink(TEST_FILE), 0);

    // File f2(test_file, File::RW | File::CREATE): open(2) with mode 0.
    // SAFETY: a NUL-terminated path.
    let f2 = unsafe { open(TEST_FILE.as_ptr(), O_RDWR | O_CREAT, 0) };
    // SAFETY: an open descriptor.
    unsafe { swoole_coroutine_lseek(f2, 0, SEEK_SET) };

    // SAFETY: as above.
    let fp2 = unsafe { swoole_coroutine_fdopen(f2, c"w+".as_ptr()) };
    assert!(!fp2.is_null());

    fputs(c"hello\n", fp2);
    fputs(c"world\n", fp2);
    // SAFETY: an open stream.
    assert_eq!(unsafe { swoole_coroutine_fflush(fp2) }, 0);

    // SAFETY: an open descriptor.
    unsafe { swoole_coroutine_lseek(f2, 0, SEEK_SET) };
    // SAFETY: as above.
    let fp3 = unsafe { swoole_coroutine_fdopen(f2, c"r+".as_ptr()) };
    assert!(!fp3.is_null());

    let mut rbuf = [0u8; 2048];
    // SAFETY: `rbuf` is writable for its length.
    assert_eq!(
        unsafe { swoole_coroutine_fread(rbuf.as_mut_ptr().cast(), rbuf.len(), 1, fp3) },
        0
    );
    assert_eq!(super::c_str(&rbuf), b"hello\nworld\n");

    // SAFETY: f2.close(), then both streams, in upstream's order.
    unsafe {
        close(f2);
        swoole_coroutine_fclose(fp2);
        swoole_coroutine_fclose(fp3);
    }

    assert_eq!(co_unlink(TEST_FILE), 0);
}

fn file() {
    run(test_file_hook);
    test_file_hook();
}

fn gethostbyname() {
    run(|| {
        // SAFETY: a NUL-terminated name.
        let result1 = unsafe { swoole_coroutine_gethostbyname(HOST_1.as_ptr()) };
        assert!(!result1.is_null());

        // SAFETY: as above.
        let result2 = unsafe { swoole_coroutine_gethostbyname(HOST_2.as_ptr()) };
        assert!(result2.is_null());
        // SAFETY: this thread's h_errno.
        assert_eq!(unsafe { *__h_errno_location() }, HOST_NOT_FOUND);
    });
}

fn getaddrinfo() {
    run(|| {
        let hints = AddrInfo {
            flags: AI_PASSIVE,
            family: AF_INET,
            socktype: SOCK_STREAM,
            protocol: 0,
            addrlen: 0,
            addr: std::ptr::null_mut(),
            canonname: std::ptr::null_mut(),
            next: std::ptr::null_mut(),
        };

        let mut result: *mut AddrInfo = std::ptr::null_mut();
        // SAFETY: NUL-terminated names, readable hints, a writable result.
        let result1 = unsafe {
            swoole_coroutine_getaddrinfo(HOST_1.as_ptr(), c"http".as_ptr(), &hints, &mut result)
        };
        assert_eq!(result1, 0);

        let mut curr = result;
        let mut count = 0;
        // SAFETY: walks getaddrinfo's list.
        while !curr.is_null() && !unsafe { (*curr).addr }.is_null() {
            // SAFETY: as above.
            curr = unsafe { (*curr).next };
            count += 1;
        }
        assert!(count >= 1);
        // SAFETY: getaddrinfo's list.
        unsafe { freeaddrinfo(result) };

        result = std::ptr::null_mut();
        // SAFETY: as above, with no service.
        let result2 = unsafe {
            swoole_coroutine_getaddrinfo(HOST_2.as_ptr(), std::ptr::null(), &hints, &mut result)
        };
        assert_eq!(result2, EAI_NONAME);
        assert!(result.is_null());
        // SAFETY: freeaddrinfo(NULL), as upstream.
        unsafe { freeaddrinfo(result) };
    });
}

fn test_fstat() {
    let fd = co_open(TEST_TMP_FILE, O_RDONLY, 0);
    let mut statbuf_1: Stat = [0; 32];
    // SAFETY: a buffer larger than `struct stat`.
    unsafe { swoole_coroutine_fstat(fd, &mut statbuf_1) };

    let mut statbuf_2: Stat = [0; 32];
    // SAFETY: as above.
    unsafe { fstat(fd, &mut statbuf_2) };

    assert_eq!(statbuf_1, statbuf_2);

    co_close(fd);
}

fn fstat_() {
    run(test_fstat);
    test_fstat();
}

// Upstream reads /tmp, whose free counts change whenever anything on the host writes to
// its filesystem between the two calls. procfs's never change, and its block size, name
// length and flags are nonzero, so a call that left the buffer unfilled still differs.
fn test_statvfs() {
    let mut statbuf_1: Stat = [0; 32];
    // SAFETY: a buffer larger than `struct statvfs`.
    unsafe { swoole_coroutine_statvfs(c"/proc".as_ptr(), &mut statbuf_1) };

    let mut statbuf_2: Stat = [0; 32];
    // SAFETY: as above.
    unsafe { statvfs(c"/proc".as_ptr(), &mut statbuf_2) };

    assert_eq!(statbuf_1, statbuf_2);
}

fn statvfs_() {
    run(test_statvfs);
    test_statvfs();
}

fn test_hook_dir() {
    // SAFETY: NUL-terminated paths.
    unsafe {
        assert_eq!(swoole_coroutine_mkdir(TEST_TMP_DIR.as_ptr(), 0o666), 0);
        assert_eq!(swoole_coroutine_access(TEST_TMP_DIR.as_ptr(), R_OK), 0);
        assert_eq!(swoole_coroutine_rmdir(TEST_TMP_DIR.as_ptr()), 0);
    }
    assert_eq!(plain_access(TEST_TMP_DIR, R_OK), -1);
}

fn dir() {
    run(test_hook_dir);
    test_hook_dir();
}

pub(super) unsafe extern "C" fn put_ip(out: *mut c_void, data: *const c_char, length: usize) {
    // SAFETY: `out` is the caller's String; `data` holds `length` bytes.
    unsafe {
        let bytes = std::slice::from_raw_parts(data.cast(), length);
        *out.cast::<String>() = String::from_utf8(bytes.to_vec()).unwrap();
    }
}

fn test_hook_socket() {
    // SAFETY: socket(2).
    let sock = unsafe { swoole_coroutine_socket(AF_INET, SOCK_STREAM, 0) };
    assert!(sock > 0);
    let mut ip = String::new();
    let host = c"www.baidu.com";
    // System::gethostbyname("www.baidu.com", AF_INET, 10)
    // SAFETY: the host's bytes; `put_ip` writes `ip`, live for the call.
    unsafe {
        swr_v1_system_gethostbyname(
            host.as_ptr(),
            host.count_bytes(),
            c_long::from(AF_INET),
            10.0,
            put_ip,
            (&raw mut ip).cast(),
        )
    };
    // sa.assign(SW_SOCK_TCP, ip, 80)
    let sa = SockAddrIn {
        family: AF_INET as u16,
        port: 80u16.to_be(),
        addr: ip
            .parse::<std::net::Ipv4Addr>()
            .map_or([0; 4], |addr| addr.octets()),
        zero: [0; 8],
    };
    let len = u32::try_from(size_of::<SockAddrIn>()).unwrap();
    // SAFETY: a readable address of `len` bytes.
    assert_eq!(unsafe { swoole_coroutine_connect(sock, &sa, len) }, 0);
    // SAFETY: waits on `sock`.
    assert_eq!(
        unsafe { swoole_coroutine_socket_wait_event(sock, SW_EVENT_WRITE, 5.0) },
        SW_OK
    );

    let req: &[u8] =
        b"GET / HTTP/1.1\r\nHost: www.baidu.com\r\nConnection: close\r\nKeepAlive: off\r\n\r\n";
    // SAFETY: `req` is readable for its length.
    let sent = unsafe { swoole_coroutine_send(sock, req.as_ptr().cast(), req.len(), 0) };
    assert_eq!(sent, isize::try_from(req.len()).unwrap());

    // swoole::String resp(1024)
    let mut resp = vec![0u8; 1024];
    let mut length = 0;
    loop {
        let spare = &mut resp[length..];
        // SAFETY: `spare` is writable for its length.
        let n = unsafe { swoole_coroutine_recv(sock, spare.as_mut_ptr().cast(), spare.len(), 0) };
        if n <= 0 {
            break;
        }
        length += n.unsigned_abs();
        if length == resp.len() {
            resp.resize(resp.len() * 2, 0);
        }
    }

    assert!(length > 100);
    assert!(contains(&resp[..length], b"baidu.com"));
    co_close(sock);
}

fn socket() {
    run(test_hook_socket);
    test_hook_socket();
}

fn rename() {
    run(|| {
        let mut buf = [0u8; 8192];
        let n_buf = buf.len();
        assert_eq!(random_bytes(&mut buf), n_buf);

        let fd = co_open(TEST_FILE, O_WRONLY | O_TRUNC | O_CREAT, 0o666);
        assert_eq!(write_fd(fd, &buf), isize::try_from(n_buf).unwrap());
        co_close(fd);

        let to_file_name = c"/tmp/swoole-core-test.bak";
        // SAFETY: NUL-terminated paths.
        assert_eq!(
            unsafe { swoole_coroutine_rename(TEST_FILE.as_ptr(), to_file_name.as_ptr()) },
            0
        );
        assert_eq!(plain_access(TEST_TMP_DIR, F_OK), -1);
        assert_eq!(plain_access(to_file_name, F_OK), 0);

        co_unlink(to_file_name);
    });
}

fn flock() {
    let start_time = Instant::now();
    run(move || {
        go(move || {
            let fd = co_open(TEST_TMP_FILE, O_WRONLY, 0);

            assert_eq!(co_flock(fd, 16), SW_ERR);
            assert_eq!(last_error(), EINVAL);

            assert_eq!(co_flock(fd, LOCK_EX), 0);
            co_sleep(0.1);
            assert_eq!(co_flock(fd, LOCK_UN), 0);

            assert_eq!(co_flock(fd, LOCK_SH), 0);
            assert_eq!(co_flock(fd, LOCK_UN), 0);
            assert!(ms_since(start_time) <= 1000);
            co_close(fd);
        });
        go(|| {
            let fd = co_open(TEST_TMP_FILE, O_WRONLY, 0);
            assert_eq!(co_flock(fd, LOCK_SH), 0);
            co_sleep(0.5);
            assert_eq!(co_flock(fd, LOCK_UN), 0);
            co_close(fd);
        });
    });
    // LOCK_NB
    run(|| {
        let fd1 = co_open(TEST_TMP_FILE, O_WRONLY, 0);
        assert_eq!(co_flock(fd1, LOCK_EX), 0);
        let fd2 = co_open(TEST_TMP_FILE, O_WRONLY, 0);
        assert_eq!(co_flock(fd2, LOCK_EX | LOCK_NB), -1);
        assert_eq!(co_flock(fd1, LOCK_UN), 0);
        co_close(fd1);
        co_close(fd2);
    });
}

fn read_dir() {
    // SAFETY: opendir(3)/readdir(3) on /tmp, which has `.` and `..`.
    let (dir1, dir2) = unsafe {
        let fp = opendir(c"/tmp".as_ptr());
        let dir1 = d_name(readdir(fp));
        let dir2 = d_name(readdir(fp));
        closedir(fp);
        (dir1, dir2)
    };

    let fn_ = || {
        // SAFETY: a NUL-terminated path.
        let fp = unsafe { swoole_coroutine_opendir(c"/tmp".as_ptr()) };
        assert!(!fp.is_null());

        // SAFETY: an open directory stream.
        let entry = unsafe { swoole_coroutine_readdir(fp) };
        assert_eq!(d_name(entry), dir1);

        // SAFETY: as above.
        let entry = unsafe { swoole_coroutine_readdir(fp) };
        assert_eq!(d_name(entry), dir2);

        // SAFETY: as above.
        unsafe { swoole_coroutine_closedir(fp) };
    };

    run(fn_);
    fn_();
}

fn readlink() {
    let fn_ = || {
        let mut buf1 = [0u8; 1024];

        // SAFETY: `buf1` is writable for its length.
        let retval = unsafe {
            swoole_coroutine_readlink(
                c"/proc/self/cwd".as_ptr(),
                buf1.as_mut_ptr().cast(),
                buf1.len(),
            )
        };
        assert_ne!(retval, -1);

        // getcwd(buf2, sizeof(buf2))
        let buf2 = std::env::current_dir().unwrap();
        assert_eq!(super::c_str(&buf1), buf2.as_os_str().as_encoded_bytes());
    };

    run(fn_);
    fn_();
}

fn stdio_1() {
    let fn_ = || {
        // SAFETY: NUL-terminated path and mode.
        let fp1 = unsafe { swoole_coroutine_fopen(TEST_FILE.as_ptr(), c"w+".as_ptr()) };
        let str_ = c"hello world";
        let n = fputs(str_, fp1);
        assert!(n != 0);
        // SAFETY: an open stream.
        unsafe { swoole_coroutine_fclose(fp1) };

        // SAFETY: as above.
        let fp2 = unsafe { swoole_coroutine_fopen(TEST_FILE.as_ptr(), c"r+".as_ptr()) };
        let mut buf = [0u8; 1024];
        // SAFETY: `buf` is writable for its length.
        let str2 = unsafe { swoole_coroutine_fgets(buf.as_mut_ptr().cast(), 1024, fp2) };

        assert!(!str2.is_null());
        // SAFETY: fgets NUL-terminated `buf`.
        assert_eq!(unsafe { CStr::from_ptr(str2) }, str_);
        // SAFETY: an open stream.
        unsafe { swoole_coroutine_fclose(fp2) };

        plain_unlink(TEST_FILE);
    };

    run(fn_);
    fn_();
}

fn stdio_2() {
    let fn_ = || {
        let size = 1024;

        // SAFETY: NUL-terminated path and mode.
        let fp1 = unsafe { swoole_coroutine_fopen(TEST_FILE.as_ptr(), c"w+".as_ptr()) };
        let mut str_ = vec![0u8; size];
        assert_eq!(random_bytes(&mut str_), size);
        // SAFETY: `str_` is readable for its length.
        let n = unsafe { swoole_coroutine_fwrite(str_.as_ptr().cast(), 1, size, fp1) };
        assert_eq!(n, size);
        // SAFETY: an open stream.
        unsafe { swoole_coroutine_fclose(fp1) };

        // SAFETY: as above.
        let fp2 = unsafe { swoole_coroutine_fopen(TEST_FILE.as_ptr(), c"r+".as_ptr()) };
        let mut buf = vec![0u8; size];
        // SAFETY: `buf` is writable for its length.
        let len = unsafe { swoole_coroutine_fread(buf.as_mut_ptr().cast(), 1, size, fp2) };
        assert_eq!(len, size);

        // SAFETY: as above.
        let len = unsafe { swoole_coroutine_fread(buf.as_mut_ptr().cast(), 1, size, fp2) };
        assert_eq!(len, 0);

        // SAFETY: an open stream.
        assert!(unsafe { swoole_coroutine_feof(fp2) } != 0);

        assert_eq!(buf, str_);
        // SAFETY: as above.
        unsafe { swoole_coroutine_fclose(fp2) };

        plain_unlink(TEST_FILE);
    };

    run(fn_);
    fn_();
}

fn sleep() {
    run(|| {
        {
            let ms1 = Instant::now();
            // SAFETY: in a coroutine.
            unsafe { swoole_coroutine_sleep(2) };
            assert!(ms_since(ms1) >= 1900);
        }
        {
            let us_1 = Instant::now();
            // SAFETY: as above.
            unsafe { swoole_coroutine_usleep(50000) };
            assert!(us_1.elapsed().as_micros() >= 45000);
        }
    });
}

fn exists() {
    run(|| {
        let fd = 100; // fake fd
        assert_eq!(socket_create(fd), 0);
        // SAFETY: a lookup.
        assert!(unsafe { swoole_coroutine_socket_exists(fd) } != 0);
        // SAFETY: as above.
        let sock = unsafe { swoole_coroutine_get_socket_object(fd) };
        assert!(!sock.is_null());
        // SAFETY: the live socket the map holds.
        assert_eq!(unsafe { (*sock).sock_fd }, fd);
        co_close(fd);
    });
}

fn set_errno(value: c_int) {
    unsafe extern "C" {
        fn __errno_location() -> *mut c_int;
    }
    // SAFETY: this thread's errno.
    unsafe { *__errno_location() = value };
}

fn timeout() {
    run(|| {
        let mut pairs = [-1; 2];
        // SAFETY: two writable descriptors.
        unsafe { socketpair(AF_UNIX, SOCK_STREAM, 0, pairs.as_mut_ptr()) };
        let text: &[u8] = b"Hello World";
        let length = text.len();

        // unregister fd
        set_errno(0);
        // SAFETY: a descriptor the hook map doesn't hold.
        assert_eq!(
            unsafe { swoole_coroutine_socket_set_timeout(pairs[0], SO_SNDTIMEO, 0.05) },
            -1
        );
        assert_eq!(errno(), EINVAL);

        set_errno(0);
        // SAFETY: as above.
        assert_eq!(
            unsafe { swoole_coroutine_socket_set_connect_timeout(pairs[0], 0.05) },
            -1
        );
        assert_eq!(errno(), EINVAL);

        go(|| {
            assert_eq!(socket_create(pairs[0]), 0);

            // unknown which
            // SAFETY: a wrapped descriptor.
            unsafe {
                assert_eq!(swoole_coroutine_socket_set_timeout(pairs[0], 100, 0.05), -1);
                swoole_coroutine_socket_set_timeout(pairs[0], SO_SNDTIMEO, 0.05);
            }
            let result = write_fd(pairs[0], text);
            assert_eq!(co_close(pairs[0]), 0);
            assert_eq!(result, isize::try_from(length).unwrap());
        });

        let mut data = vec![0u8; length];
        assert_eq!(socket_create(pairs[1]), 0);
        // SAFETY: a wrapped descriptor.
        unsafe { swoole_coroutine_socket_set_timeout(pairs[1], SO_RCVTIMEO, 0.05) };
        let result = read_fd(pairs[1], &mut data);
        assert_eq!(co_close(pairs[1]), 0);
        assert_eq!(result, isize::try_from(length).unwrap());
        assert_eq!(data, text);
    });
}

fn sendmsg_and_recvmsg() {
    run(|| {
        let mut pairs = [-1; 2];
        // SAFETY: two writable descriptors.
        unsafe { socketpair(AF_UNIX, SOCK_STREAM, 0, pairs.as_mut_ptr()) };

        let text: &[u8] = b"Hello World";
        let length = text.len();

        go(|| {
            let mut ivec = IoVec {
                base: text.as_ptr().cast_mut().cast(),
                len: length,
            };
            let msg = MsgHdr {
                name: std::ptr::null_mut(),
                namelen: 0,
                iov: &mut ivec,
                iovlen: 1,
                control: std::ptr::null_mut(),
                controllen: 0,
                flags: 0,
            };

            // SAFETY: a message over `text`.
            let ret = unsafe { swoole_coroutine_sendmsg(pairs[0], &msg, 0) };
            assert_eq!(co_close(pairs[0]), 0);
            assert_eq!(ret, isize::try_from(length).unwrap());
        });

        let mut buf = vec![0u8; length + 1];
        let mut ivec = IoVec {
            base: buf.as_mut_ptr().cast(),
            len: length,
        };
        let mut msg = MsgHdr {
            name: std::ptr::null_mut(),
            namelen: 0,
            iov: &mut ivec,
            iovlen: 1,
            control: std::ptr::null_mut(),
            controllen: 0,
            flags: 0,
        };

        // SAFETY: a message over `buf`.
        let ret = unsafe { swoole_coroutine_recvmsg(pairs[1], &mut msg, 0) };
        buf[usize::try_from(ret).unwrap()] = 0;
        assert_eq!(co_close(pairs[1]), 0);
        assert_eq!(super::c_str(&buf), text);
    });
}

fn lseek() {
    // get_jpg_file(): the source root (the case's working directory) + TEST_JPG_FILE.
    let file = std::env::current_dir().unwrap().display().to_string() + TEST_JPG_FILE;
    let file = std::ffi::CString::new(file).unwrap();
    let fd = co_open(&file, O_RDONLY, u32::from(b'r'));
    // SAFETY: an open descriptor.
    let offset = unsafe { swoole_coroutine_lseek(fd, 0, SEEK_SET) };
    co_close(fd);
    assert_eq!(offset, 0);
}

fn socket_close() {
    run(|| {
        let pair = create_socket_pair();
        let buffer = random_buffer();

        let (write, read) = (Cell::new(false), Cell::new(false));
        let fd = pair[0];
        socket_create(fd);

        // write co
        go(|| {
            for _ in 0..32 {
                let result = write_fd(fd, &buffer);
                if result < 0 && errno() == ECANCELED {
                    assert_eq!(co_close(fd), -1);
                    assert_eq!(errno(), SW_ERROR_CO_SOCKET_CLOSE_WAIT);
                    write.set(true);
                    break;
                }
            }
        });

        // read co
        go(|| {
            for _ in 0..32 {
                let mut buf = [0u8; 4096];
                let result = read_fd(fd, &mut buf);
                if result < 0 && errno() == ECANCELED {
                    assert_eq!(co_close(fd), 0);
                    read.set(true);
                    break;
                }
            }
        });

        co_sleep(0.1);
        assert_eq!(co_close(fd), -1);
        assert_eq!(errno(), SW_ERROR_CO_SOCKET_CLOSE_WAIT);
        assert!(write.get());
        assert!(read.get());
    });
}

/// `bzero(fds, sizeof(pollfd))` (the first entry only), then both entries' `fd` and
/// `events`: the second keeps its last `revents`, as upstream's does.
fn poll_fds(fds: &mut [PollFd; 2], first: (c_int, i16), second: (c_int, i16)) {
    fds[0] = PollFd {
        fd: first.0,
        events: first.1,
        revents: 0,
    };
    (fds[1].fd, fds[1].events) = second;
}

fn poll() {
    run(|| {
        let pair = create_socket_pair();
        let buffer = random_buffer();

        let (fd0, fd1) = (pair[0], pair[1]);
        socket_create(fd0);
        socket_create(fd1);

        go(|| {
            let result = write_fd(fd0, &buffer);
            assert!(result > 0);
            co_sleep(0.01);
            let result = write_fd(fd1, &buffer[..16 * 1024]);
            assert!(result > 0);
        });

        let mut fds = [
            PollFd {
                fd: 0,
                events: 0,
                revents: 0,
            },
            PollFd {
                fd: 0,
                events: 0,
                revents: 0,
            },
        ];
        let mut buf = [0u8; 4096];

        poll_fds(&mut fds, (fd0, POLLIN), (fd1, POLLIN));
        // SAFETY: two writable entries.
        assert_eq!(
            unsafe { swoole_coroutine_poll(fds.as_mut_ptr(), 2, 1000) },
            1
        );
        assert!(fds[1].revents & POLLIN != 0);

        let result = read_fd(fd1, &mut buf);
        assert!(result > 1024);

        co_sleep(0.02);

        poll_fds(&mut fds, (fd0, POLLIN), (fd1, POLLIN));
        // SAFETY: as above.
        assert_eq!(
            unsafe { swoole_coroutine_poll(fds.as_mut_ptr(), 2, 1000) },
            2
        );
        assert!(fds[0].revents & POLLIN != 0);
        assert!(fds[1].revents & POLLIN != 0);
        assert!(read_fd(fd0, &mut buf) > 1024);
        assert!(read_fd(fd1, &mut buf) > 1024);

        co_sleep(0.02);

        poll_fds(&mut fds, (fd0, POLLIN | POLLOUT), (fd1, POLLIN | POLLOUT));
        // SAFETY: as above.
        assert_eq!(
            unsafe { swoole_coroutine_poll(fds.as_mut_ptr(), 2, 1000) },
            2
        );
        assert!(fds[0].revents & POLLIN != 0);
        assert!(fds[1].revents & POLLIN != 0);
        assert!(fds[0].revents & POLLOUT == 0); // not writable
        assert!(fds[1].revents & POLLOUT != 0);
        assert!(read_fd(fd0, &mut buf) > 1024);
        assert!(read_fd(fd1, &mut buf) > 1024);
    });
}

fn poll_fake() {
    run(|| {
        let pair = create_socket_pair();
        let buffer = random_buffer();

        let (fd0, fd1) = (pair[0], pair[1]);
        socket_create(fd0);
        socket_create(fd1);

        go(|| {
            let result = write_fd(fd0, &buffer);
            assert!(result > 0);
            co_sleep(0.01);
            let result = write_fd(fd1, &buffer[..16 * 1024]);
            assert!(result > 0);
        });

        let mut fds = [
            PollFd {
                fd: 0,
                events: 0,
                revents: 0,
            },
            PollFd {
                fd: 0,
                events: 0,
                revents: 0,
            },
        ];
        let mut buf = [0u8; 4096];

        fds[0] = PollFd {
            fd: fd1,
            events: POLLIN,
            revents: 0,
        };
        // SAFETY: one writable entry.
        assert_eq!(
            unsafe { swoole_coroutine_poll_fake(fds.as_mut_ptr(), 1, 1000) },
            1
        );
        assert!(fds[0].revents & POLLIN != 0);

        assert!(read_fd(fd1, &mut buf) > 1024);

        // bzero(fds, sizeof(pollfd)): the first entry only.
        fds[0] = PollFd {
            fd: 0,
            events: 0,
            revents: 0,
        };
        // SAFETY: two writable entries.
        assert_eq!(
            unsafe { swoole_coroutine_poll_fake(fds.as_mut_ptr(), 2, 1000) },
            -1
        );
        assert_eq!(last_error(), SW_ERROR_INVALID_PARAMS);

        co_sleep(0.02);

        fds[0] = PollFd {
            fd: fd0,
            events: POLLIN | POLLOUT,
            revents: 0,
        };
        // SAFETY: one writable entry.
        assert_eq!(
            unsafe { swoole_coroutine_poll_fake(fds.as_mut_ptr(), 1, 1000) },
            1
        );
        assert!(fds[0].revents & POLLIN != 0);
        assert!(fds[0].revents & POLLOUT != 0);
    });
}

fn unwrap() {
    let pair = create_socket_pair();
    let (sock0, sock1) = (pair[0], pair[1]);

    // SAFETY: unwraps a descriptor the hook map doesn't hold.
    assert_eq!(unsafe { swoole_coroutine_socket_unwrap(sock0) }, -1);

    run(|| {
        // SAFETY: as above.
        assert_eq!(unsafe { swoole_coroutine_socket_unwrap(999999) }, -1);
        assert_eq!(socket_create(sock0), 0);
        assert!(write_fd(sock0, TEST_STR) > 0);

        // blocking, wrap-socket not exists
        let mut buf = [0u8; 128];
        assert_eq!(
            read_fd(sock1, &mut buf),
            isize::try_from(TEST_STR.len()).unwrap()
        );
        // SAFETY: as above.
        unsafe {
            assert_eq!(swoole_coroutine_socket_unwrap(sock1), -1);
            // unwrap
            assert_eq!(swoole_coroutine_socket_unwrap(sock0), 0);
            // fail to unwrap
            assert_eq!(swoole_coroutine_socket_unwrap(sock0), -1);
        }
    });
}

fn test_freopen() {
    let output_file = c"/tmp/output.txt";
    plain_unlink(output_file);
    plain_unlink(TEST_LOG_FILE);

    // SAFETY: NUL-terminated path and mode.
    let fp = unsafe { swoole_coroutine_fopen(TEST_LOG_FILE.as_ptr(), c"w".as_ptr()) };
    if fp.is_null() {
        eprintln!(
            "Failed to open '{}': errno={}",
            TEST_LOG_FILE.to_str().unwrap(),
            errno()
        );
    }
    assert!(!fp.is_null());
    fputs(c"hello\n", fp);

    // SAFETY: as above, over the open stream.
    assert!(
        !unsafe { swoole_coroutine_freopen(output_file.as_ptr(), c"w".as_ptr(), fp) }.is_null()
    );
    fputs(c"world\n", fp);

    // SAFETY: an open stream.
    unsafe { swoole_coroutine_fclose(fp) };

    let rs1 = file_get_contents(output_file);
    assert!(!contains(&rs1, b"hello\n"));
    assert!(contains(&rs1, b"world\n"));

    let rs2 = file_get_contents(TEST_LOG_FILE);
    assert!(contains(&rs2, b"hello\n"));
    assert!(!contains(&rs2, b"world\n"));

    plain_unlink(TEST_LOG_FILE);
    plain_unlink(output_file);
}

fn freopen() {
    run(test_freopen);
    test_freopen();
}

fn ftruncate() {
    run(|| {
        let fd = co_open(c"/tmp/123.txt", O_CREAT | O_RDWR, 0);
        assert!(fd > 0);

        let data: &[u8] = b"aaaaaaaaaaaaaaaaaaaaaaa";
        let length = data.len();
        let write_length = write_fd(fd, data);
        assert!(write_length == isize::try_from(length).unwrap());
        // SAFETY: an open descriptor.
        assert!(unsafe { swoole_coroutine_ftruncate(fd, 0) } == 0);
        co_close(fd);
    });
}

fn get_socket_fail() {
    {
        // SAFETY: a lookup.
        let rs = unsafe { swoole_coroutine_get_socket_object(999) };
        assert_eq!(errno(), ENOTSOCK);
        assert!(rs.is_null());
    }

    {
        let fd = Cell::new(-1);
        run(|| {
            // SAFETY: socket(2).
            fd.set(unsafe { swoole_coroutine_socket(AF_INET, SOCK_STREAM, 0) });
            assert!(errno() > 0);
        });

        // SAFETY: a lookup.
        let rs = unsafe { swoole_coroutine_get_socket_object_ex(fd.get()) };
        assert_eq!(errno(), EWOULDBLOCK);
        assert!(rs.is_null());
        co_close(fd.get());
    }
}

fn create_socket_fail() {
    run(|| {
        // SAFETY: socket(2) with an unknown family.
        let fd = unsafe { swoole_coroutine_socket(AF_INET + 99, SOCK_CLOEXEC, 0) };
        assert_eq!(fd, -1);
        assert_eq!(errno(), EAFNOSUPPORT);
    });
}

/// Fixtures the core-tests run prepares, before a case: `TEST_TMP_FILE` as
/// base.file_put_contents writes it (65535 random alphanumerics and a NUL), and no
/// `/tmp/123.txt`, which `ftruncate` creates with mode 0 and leaves behind.
pub fn prepare() {
    const ALNUM: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut content = vec![0u8; 65535];
    assert_eq!(random_bytes(&mut content), content.len());
    for byte in &mut content {
        *byte = ALNUM[usize::from(*byte) % ALNUM.len()];
    }
    content.push(0);
    std::fs::write(TEST_TMP_FILE.to_str().unwrap(), content).unwrap();
    let _ = std::fs::remove_file("/tmp/123.txt");
}

/// `coroutine_hook.<name>`, in upstream's order.
/// `core-tests/src/coroutine/accept.cpp`'s one TEST, `coroutine_hook.accept`.
fn accept() {
    run(|| {
        // SAFETY: plain socket calls on the case's own descriptors and locals.
        unsafe {
            let server_sock = swoole_coroutine_socket(AF_INET, SOCK_STREAM, 0);
            assert!(server_sock > 0);

            let server_addr = SockAddrIn {
                family: AF_INET as u16,
                port: 0,
                addr: [127, 0, 0, 1],
                zero: [0; 8],
            };
            let size = size_of::<SockAddrIn>() as u32;
            assert_eq!(bind(server_sock, &raw const server_addr, size), 0);
            assert_eq!(listen(server_sock, 128), 0);

            go(move || {
                std::thread::sleep(std::time::Duration::from_micros(10000));

                let client_sock = swoole_coroutine_socket(AF_INET, SOCK_STREAM, 0);
                assert!(client_sock > 0);

                let mut actual_server_addr = std::mem::zeroed::<SockAddrIn>();
                let mut addr_len = size_of::<SockAddrIn>() as u32;
                assert_eq!(
                    getsockname(server_sock, &raw mut actual_server_addr, &raw mut addr_len),
                    0
                );
                assert_eq!(
                    swoole_coroutine_connect(client_sock, &raw const actual_server_addr, addr_len),
                    0
                );

                let test_message = b"test_data";
                let sent_bytes =
                    swoole_coroutine_send(client_sock, test_message.as_ptr().cast(), 9, 0);
                assert_eq!(sent_bytes, 9);

                swoole_coroutine_close(client_sock);
            });

            let mut client_addr = std::mem::zeroed::<SockAddrIn>();
            let mut client_addr_len = size_of::<SockAddrIn>() as u32;
            let client_sock = swoole_coroutine_accept(
                server_sock,
                &raw mut client_addr,
                &raw mut client_addr_len,
            );
            assert!(client_sock > 0);

            let mut buffer = [0u8; 256];
            let received_bytes =
                swoole_coroutine_recv(client_sock, buffer.as_mut_ptr().cast(), buffer.len() - 1, 0);
            assert!(received_bytes > 0);
            assert_eq!(CStr::from_bytes_until_nul(&buffer).unwrap(), c"test_data");

            swoole_coroutine_close(client_sock);
            swoole_coroutine_close(server_sock);
        }
    });
}

pub const CASES: super::Cases = &[
    ("file", file),
    ("gethostbyname", gethostbyname),
    ("getaddrinfo", getaddrinfo),
    ("fstat", fstat_),
    ("statvfs", statvfs_),
    ("dir", dir),
    ("socket", socket),
    ("rename", rename),
    ("flock", flock),
    ("read_dir", read_dir),
    ("readlink", readlink),
    ("stdio_1", stdio_1),
    ("stdio_2", stdio_2),
    ("sleep", sleep),
    ("exists", exists),
    ("timeout", timeout),
    ("sendmsg_and_recvmsg", sendmsg_and_recvmsg),
    ("lseek", lseek),
    ("socket_close", socket_close),
    ("poll", poll),
    ("poll_fake", poll_fake),
    ("unwrap", unwrap),
    ("freopen", freopen),
    ("ftruncate", ftruncate),
    ("get_socket_fail", get_socket_fail),
    ("create_socket_fail", create_socket_fail),
    // core-tests/src/coroutine/accept.cpp
    ("accept", accept),
];
