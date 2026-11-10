//! Upstream's `core-tests/src/coroutine/system.cpp` (hook.cpp in hook.rs, socket.cpp in socket.rs,
//! network/client.cpp in tests/modules/clients/client.rs, network/dns.cpp in dns.rs,
//! network/address.cpp in address.rs, network/socket.cpp in network_socket.rs,
//! coroutine/gethostbyname.cpp in gethostbyname.rs, coroutine/accept.cpp in hook.rs, os/timer.cpp
//! in timer.rs, os/pipe.cpp in pipe.rs, os/process_pool.cpp in process_pool.rs and
//! protocol/ssl.cpp in ssl.rs), one
//! Rust function per TEST with the same inputs and expected values, over the
//! hybrid's libswoole.so: the Rust `System` operations under their upstream
//! symbols, and `swr_v1_system_*` and `swr_v1_hook_socket_poll` where upstream's
//! signature carries C++ library types. The binary also runs coroutine/base.cpp
//! and file.cpp (tests/modules/coroutine/base.rs), and the repo-owned C++ witnesses
//! in ../fixture.cc.
//!
//! `main --list` prints the cases; `main NAME` runs one, in this process, as
//! core-tests' main does (`swoole_init`, the case, `swoole_clean`). It exits 0 when
//! every assertion held. A failed assertion panics; inside a coroutine the panic
//! cannot unwind through the context switch, so the trampoline `_exit(1)`s instead.

use std::ffi::{CStr, c_char, c_int, c_long, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};

mod address;
// Upstream's core-tests/src/network/client.cpp: a clients test, built here to share this
// binary's libswoole.so build and runner.
#[path = "../../../clients/client.rs"]
mod client;
mod dns;
mod gethostbyname;
// Upstream's core-tests/src/coroutine/base.cpp and file.cpp: coroutine tests, built here
// for the same reason.
#[path = "../../../coroutine/base.rs"]
mod base;
mod hook;
mod network_socket;
mod pipe;
mod process_pool;
mod socket;
mod ssl;
mod timer;

const SW_EVENT_READ: c_int = 1 << 9;
const SW_EVENT_WRITE: c_int = 1 << 10;
const SW_ERROR_CO_TIMEDOUT: c_int = 10018;
const SW_SIGNO_MAX: c_int = 128;
const SW_ERR: c_int = -1;
const SW_STACK_BUFFER_SIZE: usize = 65536;
const SW_SOCKET_BUFFER_SIZE: u32 = 8_388_608;
const SW_NUM_MILLION: usize = 1_000_000;
// glibc's values (the core-tests build is Linux).
const EAI_NONAME: c_int = -2;
const ENOENT: c_int = 2;
const EBADF: c_int = 9;
const EBUSY: c_int = 16;
const EINVAL: c_int = 22;
const EAGAIN: c_int = 11;
const ETIMEDOUT: c_int = 110;
const SIGKILL: c_int = 9;
const SIGUSR1: c_int = 10;
const SIGUSR2: c_int = 12;
const WNOHANG: c_int = 1;
const LOCK_SH: c_int = 1;
const LOCK_EX: c_int = 2;
const LOCK_NB: c_int = 4;
const LOCK_UN: c_int = 8;
const O_RDONLY: c_int = 0;
const O_WRONLY: c_int = 1;
const O_CREAT: c_int = 0o100;
const O_NONBLOCK: c_int = 0o4000;
const F_GETFL: c_int = 3;
const F_SETFL: c_int = 4;
const AF_UNIX: c_int = 1;
const AF_INET: c_int = 2;
const AF_INET6: c_int = 10;
const SOCK_STREAM: c_int = 1;
const SOCK_DGRAM: c_int = 2;
const SOL_SOCKET: c_int = 1;
const SO_SNDBUF: c_int = 7;
const SO_RCVBUF: c_int = 8;
const RLIMIT_NOFILE: c_int = 7;

const TEST_FILE: &CStr = c"/tmp/swoole-core-test";
const DATA_SIZE: usize = 8 * 1024 * 1024;
const DATA_SIZE_2: usize = 64 * 1024;
const TEST_HTTP_DOMAIN: &CStr = c"www.gov.cn";
const TEST_STR: &[u8] = b"hello world, hello swoole\n";
const GREETING: &[u8] = b"hello world, hello swoole";

type Routine = unsafe extern "C-unwind" fn(*mut c_void);
type Put = unsafe extern "C" fn(*mut c_void, *const c_char, usize);

#[repr(C)]
#[derive(Clone, Copy)]
struct PollFd {
    fd: i32,
    events: i32,
    revents: i32,
}

/// `swoole::String`.
#[repr(C)]
struct CppString {
    length: usize,
    size: usize,
    offset: i64,
    str: *mut c_char,
    allocator: *const c_void,
}

/// `std::shared_ptr<T>` (object, control block); passed by address.
#[repr(C)]
struct SharedPtr {
    object: *mut CppString,
    control: *mut c_void,
}

#[link(name = "swoole")]
unsafe extern "C-unwind" {
    #[link_name = "_Z11swoole_initv"]
    fn swoole_init();
    #[link_name = "_Z12swoole_cleanv"]
    fn swoole_clean();
    #[link_name = "_Z19swoole_random_bytesPcm"]
    fn swoole_random_bytes(buf: *mut c_char, size: usize) -> usize;
    #[link_name = "_ZN6swoole6StringC1EmPKNS_9AllocatorE"]
    fn string_new(this: *mut CppString, size: usize, allocator: *const c_void);
    fn swr_v1_core_last_error() -> c_int;
    fn swr_v1_core_set_signal_listener_num(value: u32);

    fn swoole_coroutine_create(routine: Routine, arg: *mut c_void) -> c_long;
    fn swoole_coroutine_get_id() -> c_long;
    #[link_name = "_Z20swoole_coroutine_getl"]
    fn swoole_coroutine_get(cid: c_long) -> *mut c_void;
    #[link_name = "_ZN6swoole9Coroutine6cancelEv"]
    fn coroutine_cancel(co: *mut c_void) -> bool;

    fn swoole_coroutine_open(path: *const c_char, flags: c_int, mode: u32) -> c_int;
    fn swoole_coroutine_read(fd: c_int, buf: *mut c_void, count: usize) -> isize;
    fn swoole_coroutine_write(fd: c_int, buf: *const c_void, count: usize) -> isize;
    fn swoole_coroutine_flock(fd: c_int, operation: c_int) -> c_int;
    fn swoole_coroutine_close(fd: c_int) -> c_int;

    #[link_name = "_ZN6swoole9coroutine6System5sleepEd"]
    fn system_sleep(sec: f64) -> c_int;
    #[link_name = "_ZN6swoole9coroutine6System10write_fileEPKcS3_mbi"]
    fn system_write_file(
        file: *const c_char,
        buf: *const c_char,
        length: usize,
        lock: bool,
        flags: c_int,
    ) -> isize;
    #[link_name = "_ZN6swoole9coroutine6System10wait_eventEiid"]
    fn system_wait_event(fd: c_int, events: c_int, timeout: f64) -> c_int;
    #[link_name = "_ZN6swoole9coroutine6System7waitpidEiPiid"]
    fn system_waitpid(pid: c_int, status: *mut c_int, options: c_int, timeout: f64) -> c_int;
    #[link_name = "_ZN6swoole9coroutine6System4execEPKcbSt10shared_ptrINS_6StringEEPi"]
    fn system_exec(
        command: *const c_char,
        errors: bool,
        buffer: *const SharedPtr,
        status: *mut c_int,
    ) -> bool;
    fn swr_v1_system_read_file(file: *const c_char, lock: bool, put: Put, out: *mut c_void)
    -> bool;
    fn swr_v1_system_getaddrinfo(
        host: *const c_char,
        family: c_long,
        socktype: c_long,
        protocol: c_long,
        service: *const c_char,
        timeout: f64,
        put: Put,
        out: *mut c_void,
    ) -> bool;
    fn swr_v1_system_wait_signal(signals: *const c_long, count: usize, timeout: f64) -> c_int;
    fn swr_v1_hook_socket_poll(fds: *mut PollFd, count: usize, timeout: f64) -> bool;
}

unsafe extern "C" {
    fn socketpair(domain: c_int, kind: c_int, protocol: c_int, fds: *mut c_int) -> c_int;
    fn setsockopt(fd: c_int, level: c_int, name: c_int, value: *const c_void, length: u32)
    -> c_int;
    fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
    fn read(fd: c_int, buf: *mut c_void, count: usize) -> isize;
    fn write(fd: c_int, buf: *const c_void, count: usize) -> isize;
    fn open(path: *const c_char, flags: c_int, ...) -> c_int;
    fn unlink(path: *const c_char) -> c_int;
    fn kill(pid: c_int, signal: c_int) -> c_int;
    fn getpid() -> c_int;
    fn fork() -> c_int;
    fn sleep(seconds: u32) -> u32;
    fn _exit(status: c_int) -> !;
    fn setrlimit(resource: c_int, limit: *const [u64; 2]) -> c_int;
}

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

fn last_error() -> c_int {
    // SAFETY: reads this thread's last error.
    unsafe { swr_v1_core_last_error() }
}

/// The bytes before the first NUL, as `ASSERT_STREQ` compares.
fn c_str(bytes: &[u8]) -> &[u8] {
    &bytes[..bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len())]
}

fn random_bytes(buf: &mut [u8]) -> usize {
    // SAFETY: `buf` is writable for its length.
    unsafe { swoole_random_bytes(buf.as_mut_ptr().cast(), buf.len()) }
}

unsafe extern "C-unwind" fn trampoline(arg: *mut c_void) {
    // SAFETY: `go` boxed this closure for this call.
    let body = unsafe { Box::from_raw(arg.cast::<Box<dyn FnOnce()>>()) };
    if catch_unwind(AssertUnwindSafe(body)).is_err() {
        // SAFETY: ends the process without its exit handlers, mid-coroutine.
        unsafe { _exit(1) };
    }
}

/// `Coroutine::create(fn)`; outside the event loop, core-tests' `test::coroutine::run`:
/// a fresh loop running the coroutine to the end. Either way `ASSERT_GT(cid, 0)`.
fn go<'a>(body: impl FnOnce() + 'a) {
    let body: Box<dyn FnOnce() + 'a> = Box::new(body);
    // The borrows `body` holds outlive it, as the C++ lambdas' captures do.
    let arg = Box::into_raw(Box::new(body)).cast::<c_void>();
    // SAFETY: `trampoline` takes the box `arg` owns.
    let cid = unsafe { swoole_coroutine_create(trampoline, arg) };
    assert!(cid > 0);
}

fn run<'a>(body: impl FnOnce() + 'a) {
    go(body);
}

fn current() -> *mut c_void {
    // SAFETY: the running coroutine's id and native object.
    unsafe { swoole_coroutine_get(swoole_coroutine_get_id()) }
}

fn co_sleep(sec: f64) -> c_int {
    // SAFETY: called in a coroutine.
    unsafe { system_sleep(sec) }
}

fn wait_event(fd: c_int, events: c_int, timeout: f64) -> c_int {
    // SAFETY: called in a coroutine.
    unsafe { system_wait_event(fd, events, timeout) }
}

fn wait_signal(signal: c_int, timeout: f64) -> c_int {
    let signals = [c_long::from(signal)];
    // SAFETY: one signal.
    unsafe { swr_v1_system_wait_signal(signals.as_ptr(), 1, timeout) }
}

fn waitpid(pid: c_int, status: &mut c_int, options: c_int, timeout: f64) -> c_int {
    // SAFETY: a writable status, in a coroutine.
    unsafe { system_waitpid(pid, status, options, timeout) }
}

fn write_file(file: &CStr, buf: &[u8], lock: bool, flags: c_int) -> isize {
    // SAFETY: `buf` is readable for its length.
    unsafe { system_write_file(file.as_ptr(), buf.as_ptr().cast(), buf.len(), lock, flags) }
}

unsafe extern "C" fn put_bytes(out: *mut c_void, data: *const c_char, length: usize) {
    // SAFETY: `out` is the caller's Vec; `data` holds `length` bytes.
    unsafe {
        (*out.cast::<Vec<Vec<u8>>>()).push(std::slice::from_raw_parts(data.cast(), length).to_vec())
    };
}

/// `System::read_file`: the content, or None.
fn read_file(file: &CStr, lock: bool) -> Option<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = Vec::new();
    // SAFETY: `put_bytes` appends to `out`.
    let read =
        unsafe { swr_v1_system_read_file(file.as_ptr(), lock, put_bytes, (&raw mut out).cast()) };
    read.then(|| out.pop().unwrap_or_default())
}

/// `System::getaddrinfo`.
fn getaddrinfo(
    host: &CStr,
    family: c_int,
    socktype: c_int,
    protocol: c_int,
    service: &CStr,
    timeout: f64,
) -> Vec<String> {
    let mut out: Vec<Vec<u8>> = Vec::new();
    // SAFETY: `put_bytes` appends to `out`.
    unsafe {
        swr_v1_system_getaddrinfo(
            host.as_ptr(),
            c_long::from(family),
            c_long::from(socktype),
            c_long::from(protocol),
            service.as_ptr(),
            timeout,
            put_bytes,
            (&raw mut out).cast(),
        )
    };
    out.into_iter()
        .map(|address| String::from_utf8(address).unwrap())
        .collect()
}

/// `System::socket_poll` over core-tests' one-entry map.
fn socket_poll(fds: &mut [PollFd], timeout: f64) -> bool {
    // SAFETY: `fds` is writable, with distinct fds.
    unsafe { swr_v1_hook_socket_poll(fds.as_mut_ptr(), fds.len(), timeout) }
}

/// core-tests' `UnixSocket`: socketpair(2); the master (socks[1]) writes, the worker
/// (socks[0]) reads.
struct UnixSocket {
    socks: [c_int; 2],
    ready: bool,
}

impl UnixSocket {
    fn new(blocking: bool, protocol: c_int) -> Self {
        let mut socks = [-1; 2];
        // SAFETY: two writable descriptors.
        let ready = unsafe { socketpair(AF_UNIX, protocol, 0, socks.as_mut_ptr()) } == 0;
        let pair = Self { socks, ready };
        if ready {
            pair.set_blocking(blocking);
            assert!(pair.set_buffer_size(SW_SOCKET_BUFFER_SIZE));
        }
        pair
    }

    fn get_socket(&self, master: bool) -> c_int {
        self.socks[usize::from(!master)]
    }

    fn set_blocking(&self, blocking: bool) {
        for fd in self.socks {
            // SAFETY: fcntl(2) on this pair's descriptors.
            unsafe {
                let flags = fcntl(fd, F_GETFL);
                fcntl(
                    fd,
                    F_SETFL,
                    if blocking {
                        flags & !O_NONBLOCK
                    } else {
                        flags | O_NONBLOCK
                    },
                );
            }
        }
    }

    fn set_buffer_size(&self, size: u32) -> bool {
        self.socks.iter().all(|&fd| {
            [SO_SNDBUF, SO_RCVBUF].iter().all(|&name| {
                // SAFETY: an int option on this pair's descriptors.
                unsafe { setsockopt(fd, SOL_SOCKET, name, (&raw const size).cast(), 4) == 0 }
            })
        })
    }

    fn write(&self, buf: &[u8]) -> isize {
        fd_write(self.get_socket(true), buf)
    }
}

impl Drop for UnixSocket {
    fn drop(&mut self) {
        for fd in self.socks {
            // SAFETY: this pair's descriptors.
            unsafe { swoole_coroutine_close(fd) };
        }
    }
}

fn fd_write(fd: c_int, buf: &[u8]) -> isize {
    // SAFETY: `buf` is readable for its length.
    unsafe { write(fd, buf.as_ptr().cast(), buf.len()) }
}

fn fd_read(fd: c_int, buf: &mut [u8]) -> isize {
    // SAFETY: `buf` is writable for its length.
    unsafe { read(fd, buf.as_mut_ptr().cast(), buf.len()) }
}

fn spawn_exec(body: impl FnOnce()) -> c_int {
    // SAFETY: fork(2); the child runs `body` and exits.
    let pid = unsafe { fork() };
    assert_ne!(pid, -1);
    if pid == 0 {
        body();
        // SAFETY: ends the child.
        unsafe { _exit(0) };
    }
    pid
}

fn file() {
    run(|| {
        let mut buf = vec![0u8; DATA_SIZE];
        let size = buf.len();
        assert_eq!(random_bytes(&mut buf[..size - 1]), size - 1);
        buf[size - 1] = 0;
        assert_eq!(
            write_file(TEST_FILE, &buf, true, 0),
            isize::try_from(size).unwrap()
        );
        let data = read_file(TEST_FILE, true);
        assert!(data.is_some());
        assert_eq!(c_str(&buf), c_str(&data.unwrap()));
        // SAFETY: a NUL-terminated path.
        unsafe { unlink(TEST_FILE.as_ptr()) };
    });
}

fn flock() {
    let mut buf = vec![0u8; 65536];
    let size = buf.len();
    assert_eq!(random_bytes(&mut buf[..size - 1]), size - 1);
    buf[size - 1] = 0;
    let buf = &buf;

    run(move || {
        // SAFETY: the coroutine file API with a NUL-terminated path.
        let fd = unsafe { swoole_coroutine_open(TEST_FILE.as_ptr(), O_WRONLY | O_CREAT, 0o666) };
        assert!(fd > 0);
        // SAFETY: as above.
        unsafe { swoole_coroutine_flock(fd, LOCK_EX) };

        for _ in 0..4 {
            go(move || {
                // SAFETY: as above.
                let fd = unsafe { swoole_coroutine_open(TEST_FILE.as_ptr(), O_RDONLY, 0) };
                assert!(fd > 0);
                // SAFETY: as above.
                unsafe { swoole_coroutine_flock(fd, LOCK_SH) };
                let mut read_buf = vec![0u8; DATA_SIZE_2];
                let size = read_buf.len();
                // SAFETY: `read_buf` is writable for size - 1 bytes.
                let rn =
                    unsafe { swoole_coroutine_read(fd, read_buf.as_mut_ptr().cast(), size - 1) };
                assert_eq!(rn, isize::try_from(size - 1).unwrap());
                read_buf[size - 1] = 0;
                // SAFETY: as above.
                unsafe { swoole_coroutine_flock(fd, LOCK_UN) };
                assert_eq!(c_str(&read_buf), c_str(buf));
                // SAFETY: as above.
                unsafe { swoole_coroutine_close(fd) };
            });
        }

        // SAFETY: `buf` is readable for size - 1 bytes.
        let wn = unsafe { swoole_coroutine_write(fd, buf.as_ptr().cast(), buf.len() - 1) };
        assert_eq!(wn, isize::try_from(buf.len() - 1).unwrap());
        // SAFETY: as above.
        unsafe { swoole_coroutine_flock(fd, LOCK_UN) };
        // SAFETY: as above.
        unsafe { swoole_coroutine_close(fd) };
    });

    // SAFETY: a NUL-terminated path.
    unsafe { unlink(TEST_FILE.as_ptr()) };
}

fn flock_nb() {
    run(|| {
        // SAFETY: the coroutine file API with a NUL-terminated path.
        let fd = unsafe { swoole_coroutine_open(TEST_FILE.as_ptr(), O_WRONLY | O_CREAT, 0o666) };
        // SAFETY: as above.
        assert_eq!(unsafe { swoole_coroutine_flock(fd, LOCK_EX | LOCK_NB) }, 0);

        let t = std::thread::spawn(|| {
            // SAFETY: open(2) with a NUL-terminated path.
            let fd = unsafe { open(TEST_FILE.as_ptr(), O_WRONLY | O_CREAT, 0o666) };
            // SAFETY: outside a coroutine these are the plain calls.
            assert_eq!(unsafe { swoole_coroutine_flock(fd, LOCK_EX) }, 0);
            // SAFETY: as above.
            assert_eq!(unsafe { swoole_coroutine_flock(fd, LOCK_UN) }, 0);
            // SAFETY: as above.
            unsafe { swoole_coroutine_close(fd) };
            // SAFETY: a NUL-terminated path.
            unsafe { unlink(TEST_FILE.as_ptr()) };
        });

        // SAFETY: as above.
        assert_eq!(unsafe { swoole_coroutine_flock(fd, LOCK_UN) }, 0);

        t.join().unwrap();
    });
}

fn cancel_sleep() {
    run(|| {
        let co = current();
        go(move || {
            co_sleep(0.002);
            // SAFETY: `co` is suspended in its sleep.
            unsafe { coroutine_cancel(co) };
        });
        co_sleep(1000.0);
    });
}

fn test_getaddrinfo(
    host: &CStr,
    family: c_int,
    kind: c_int,
    protocol: c_int,
    service: &CStr,
    timeout: f64,
) {
    let ip_list = getaddrinfo(host, family, kind, protocol, service, timeout);
    assert!(!ip_list.is_empty());
    for ip in &ip_list {
        let parsed: std::net::IpAddr = ip.parse().unwrap();
        assert_eq!(parsed.is_ipv4(), family == AF_INET);
        if std::env::var_os("GITHUB_ACTIONS").is_none() {
            println!("{ip}");
            assert!(std::net::TcpStream::connect((parsed, 443)).is_ok());
        }
    }
}

fn getaddrinfo_() {
    run(|| {
        test_getaddrinfo(TEST_HTTP_DOMAIN, AF_INET, SOCK_STREAM, 0, c"http", -1.0);
        test_getaddrinfo(TEST_HTTP_DOMAIN, AF_INET6, SOCK_STREAM, 0, c"http", -1.0);
    });
}

fn getaddrinfo_fail() {
    run(|| {
        let ip_list = getaddrinfo(
            c"w11.baidu.com-not-exists",
            AF_INET,
            SOCK_STREAM,
            0,
            c"http",
            -1.0,
        );
        assert_eq!(ip_list.len(), 0);
        assert_eq!(last_error(), EAI_NONAME);
    });
}

fn getaddrinfo_timeout() {
    run(|| {
        let ip_list = getaddrinfo(
            c"w12.baidu.com-not-exists",
            AF_INET,
            SOCK_STREAM,
            0,
            c"http",
            0.005,
        );
        assert_eq!(ip_list.len(), 0);
        assert_eq!(last_error(), SW_ERROR_CO_TIMEDOUT);
    });
}

fn wait_signal_() {
    run(|| {
        go(|| {
            co_sleep(0.002);
            // SAFETY: signals this process.
            unsafe { kill(getpid(), SIGUSR1) };
        });
        assert_eq!(wait_signal(SIGUSR1, 1.0), SIGUSR1);
        assert_eq!(wait_signal(SIGUSR2, 0.1), -1);
    });
}

fn wait_signal_invalid_signo() {
    run(|| {
        assert_eq!(wait_signal(SW_SIGNO_MAX, -1.0), SW_ERR);
        assert_eq!(last_error(), EINVAL);
    });
}

fn wait_signal_fail() {
    run(|| {
        // SAFETY: SwooleG.signal_listener_num = 1.
        unsafe { swr_v1_core_set_signal_listener_num(1) };
        assert_eq!(wait_signal(SIGUSR1, 1.0), SW_ERR);
        assert_eq!(last_error(), EBUSY);
        // SAFETY: as above.
        unsafe { swr_v1_core_set_signal_listener_num(0) };
    });
}

fn wait_event_readable() {
    let p = UnixSocket::new(true, SOCK_DGRAM);
    assert!(p.ready);
    let p = &p;

    run(move || {
        go(move || {
            co_sleep(0.002);
            assert!(p.write(GREETING) > 0);
        });

        // bad fd
        assert_eq!(wait_event(9999, SW_EVENT_READ, 1.0), -1);
        assert_eq!(errno(), EBADF);
        assert_eq!(last_error(), EBADF);

        // trigger event
        let mut buffer = [0u8; 128];
        let pipe_sock = p.get_socket(false);
        // readable
        assert_eq!(wait_event(pipe_sock, SW_EVENT_READ, 1.0), SW_EVENT_READ);
        // readable + writable
        assert_eq!(
            wait_event(pipe_sock, SW_EVENT_READ | SW_EVENT_WRITE, 1.0),
            SW_EVENT_READ | SW_EVENT_WRITE
        );

        let n = fd_read(pipe_sock, &mut buffer);
        let n = usize::try_from(n).unwrap();
        assert_eq!(GREETING.len(), n);
        assert_eq!(GREETING, &buffer[..n]);

        // timeout
        let pipe_sock_2 = p.get_socket(true);
        assert_eq!(wait_event(pipe_sock_2, SW_EVENT_READ, 0.1), -1);
        assert_eq!(errno(), SW_ERROR_CO_TIMEDOUT);
        assert_eq!(last_error(), SW_ERROR_CO_TIMEDOUT);
    });
}

fn wait_event_writable() {
    let p = UnixSocket::new(true, SOCK_STREAM);
    assert!(p.ready);
    p.set_blocking(false);
    p.set_buffer_size(65536);
    // sw_tg_buffer(), cleared: a String of SW_STACK_BUFFER_SIZE that grow() doubles when full.
    let mut tg_buf = vec![0u8; SW_STACK_BUFFER_SIZE];
    let mut tg_length = 0;

    let size = 2 * SW_NUM_MILLION;
    let mut str = vec![0u8; size];
    assert_eq!(random_bytes(&mut str[..size - 1]), size - 1);
    str[size - 1] = 0;
    let (p, str) = (&p, &str);

    run(|| {
        go(move || {
            co_sleep(0.002);
            let pipe_sock = p.get_socket(true);
            let mut ptr = &str[..];
            while !ptr.is_empty() {
                let retval = fd_write(pipe_sock, &ptr[..ptr.len().min(8192)]);
                if retval > 0 {
                    ptr = &ptr[retval.unsigned_abs()..];
                } else if retval == 0 || (retval < 0 && errno() != EAGAIN) {
                    break;
                }
                wait_event(pipe_sock, SW_EVENT_WRITE, 1.0);
            }
        });

        let pipe_sock = p.get_socket(false);
        while tg_length < size - 1 {
            let retval = fd_read(pipe_sock, &mut tg_buf[tg_length..]);
            if retval > 0 {
                tg_length += retval.unsigned_abs();
                if tg_length == tg_buf.len() {
                    tg_buf.resize(tg_buf.len() * 2, 0);
                }
                continue;
            } else if retval == 0 && (retval < 0 && errno() != EAGAIN) {
                break;
            }
            wait_event(pipe_sock, SW_EVENT_READ, 1.0);
        }
        if tg_length == tg_buf.len() {
            tg_buf.push(0);
        }
        tg_buf[tg_length] = 0;
        assert_eq!(c_str(&tg_buf), c_str(str));
    });
}

fn wait_event_fail() {
    let p = UnixSocket::new(true, SOCK_DGRAM);
    let p = &p;
    run(move || {
        assert_eq!(wait_event(9999, 0, 1.0), SW_ERR);
        assert_eq!(last_error(), EINVAL);

        assert_eq!(wait_event(p.get_socket(true), SW_EVENT_READ, 0.0), SW_ERR);
        assert_eq!(last_error(), ETIMEDOUT);

        assert_eq!(
            wait_event(p.get_socket(false), SW_EVENT_WRITE, 0.0),
            SW_EVENT_WRITE
        );

        assert_eq!(wait_event(9999, SW_EVENT_WRITE, 0.0), -1);
        assert_eq!(last_error(), EBADF);

        assert_eq!(wait_event(9999, SW_EVENT_WRITE, 1.0), -1);
        assert_eq!(last_error(), EBADF);
    });
}

fn swoole_stream_select() {
    let p = UnixSocket::new(true, SOCK_STREAM);
    let mut fds = [PollFd {
        fd: p.get_socket(false),
        events: SW_EVENT_READ,
        revents: 0,
    }];

    run(|| {
        // try timeout to trigger socket_poll_timeout function
        assert!(!socket_poll(&mut fds, 0.5));
    });

    // start normal process
    let p = &p;
    run(|| {
        let text = b"Hello world";
        let len = text.len();

        // child pipe
        go(move || {
            co_sleep(0.05);
            let pipe_sock = p.get_socket(true);
            assert_eq!(fd_write(pipe_sock, text), isize::try_from(len).unwrap());
        });

        // master pipe
        let result = socket_poll(&mut fds, 0.5);
        assert!(result);

        let mut buffer = [0u8; 128];
        let pipe_sock = p.get_socket(false);
        let retval = fd_read(pipe_sock, &mut buffer);

        assert_eq!(retval, isize::try_from(len).unwrap());
        assert_eq!(&text[..], &buffer[..len]);
    });
}

fn timeout_is_zero() {
    let p = UnixSocket::new(true, SOCK_STREAM);
    let mut fds = [PollFd {
        fd: p.get_socket(false),
        events: SW_EVENT_READ,
        revents: 0,
    }];
    let p = &p;

    // timeout is 0
    run(|| {
        let text = b"Hello world";
        let len = text.len();

        // child pipe
        go(move || {
            let pipe_sock = p.get_socket(true);
            assert_eq!(fd_write(pipe_sock, text), isize::try_from(len).unwrap());
        });

        // master pipe
        let result = socket_poll(&mut fds, 0.0);
        assert!(result);

        // child pipe
        go(move || {
            let pipe_sock = p.get_socket(true);
            assert_eq!(fd_write(pipe_sock, text), isize::try_from(len).unwrap());
        });

        // master pipe
        let pipe_sock = p.get_socket(false);
        let result = wait_event(pipe_sock, SW_EVENT_READ, 0.0);
        assert!(result != 0);
    });
}

fn exec() {
    run(|| {
        let mut status = 0;
        // swoole::make_string(1024): a String of 1024 bytes from the default allocator.
        let string = Box::into_raw(Box::new(std::mem::MaybeUninit::<CppString>::uninit()))
            .cast::<CppString>();
        // SAFETY: constructs the String in place.
        unsafe { string_new(string, 1024, std::ptr::null()) };
        let buffer = SharedPtr {
            object: string,
            control: std::ptr::null_mut(),
        };
        // SAFETY: a live String and a writable status, in a coroutine.
        assert!(unsafe { system_exec(c"ls /".as_ptr(), true, &raw const buffer, &raw mut status) });
        // SAFETY: the String exec filled.
        let output = unsafe {
            std::slice::from_raw_parts((*buffer.object).str.cast::<u8>(), (*buffer.object).length)
        };
        assert!(output.windows(3).any(|window| window == b"tmp"));
    });
}

fn waitpid_() {
    let pid = spawn_exec(|| {
        // SAFETY: sleep(3).
        unsafe { sleep(2000) };
    });

    run(move || {
        let mut status = 0;
        assert_eq!(waitpid(pid, &mut status, WNOHANG, -1.0), 0);
        assert_eq!(waitpid(pid, &mut status, 0, 0.1), -1);
        assert_eq!(last_error(), ETIMEDOUT);

        // SAFETY: kills the child.
        unsafe { kill(pid, SIGKILL) };
        co_sleep(0.3);
        assert_eq!(waitpid(pid, &mut status, 0, 0.1), pid);
    });
}

fn waitpid_any() {
    let pid = spawn_exec(|| {
        // SAFETY: sleep(3).
        unsafe { sleep(2000) };
    });

    run(move || {
        let mut status = 0;
        assert_eq!(waitpid(pid, &mut status, WNOHANG, -1.0), 0);
        assert_eq!(waitpid(pid, &mut status, 0, 0.1), -1);
        assert_eq!(last_error(), ETIMEDOUT);

        // SAFETY: kills the child.
        unsafe { kill(pid, SIGKILL) };
        co_sleep(0.3);
        assert_eq!(waitpid(-1, &mut status, 0, 0.1), pid);
    });
}

fn read_file_fail() {
    run(|| {
        assert_eq!(read_file(c"/tmp/not-exists", true), None);
        assert_eq!(errno(), ENOENT);
    });
}

fn write_file_fail() {
    run(|| {
        assert_eq!(
            write_file(c"/tmp/not-exists/file.log", TEST_STR, false, 0),
            -1
        );
        assert_eq!(errno(), ENOENT);
    });
}

/// `coroutine_system.<name>`, in upstream's order.
const CASES: Cases = &[
    ("file", file),
    ("flock", flock),
    ("flock_nb", flock_nb),
    ("cancel_sleep", cancel_sleep),
    ("getaddrinfo", getaddrinfo_),
    ("getaddrinfo_fail", getaddrinfo_fail),
    ("getaddrinfo_timeout", getaddrinfo_timeout),
    ("wait_signal", wait_signal_),
    ("wait_signal_invalid_signo", wait_signal_invalid_signo),
    ("wait_signal_fail", wait_signal_fail),
    ("wait_event_readable", wait_event_readable),
    ("wait_event_writable", wait_event_writable),
    ("wait_event_fail", wait_event_fail),
    ("swoole_stream_select", swoole_stream_select),
    ("timeout_is_zero", timeout_is_zero),
    ("exec", exec),
    ("waitpid", waitpid_),
    ("waitpid_any", waitpid_any),
    ("read_file_fail", read_file_fail),
    ("write_file_fail", write_file_fail),
];

type Cases = &'static [(&'static str, fn())];

/// Each suite and its cases.
const SUITES: &[(&str, Cases)] = &[
    ("coroutine_system", CASES),
    ("coroutine_hook", hook::CASES),
    ("coroutine_socket", socket::CASES),
    ("rust_socket_scopes", socket::SCOPE_CASES),
    ("client", client::CASES),
    ("dns", dns::CASES),
    ("coroutine_gethostbyname", gethostbyname::CASES),
    ("address", address::CASES),
    ("coroutine_base", base::CASES),
    ("coroutine_async_file", base::ASYNC_FILE_CASES),
    ("rust_dns_request", socket::DNS_REQUEST_CASES),
    ("socket", network_socket::CASES),
    ("timer", timer::CASES),
    ("ssl", ssl::CASES),
    ("pipe", pipe::CASES),
    ("process_pool", process_pool::CASES),
];

fn main() {
    let name = std::env::args().nth(1).expect("usage: main --list | NAME");
    if name == "--list" {
        for (suite, cases) in SUITES {
            for (case, _) in *cases {
                println!("{suite}.{case}");
            }
        }
        return;
    }
    let (suite, case) = name.split_once('.').expect("SUITE.NAME");
    let (_, cases) = SUITES
        .iter()
        .find(|(known, _)| *known == suite)
        .expect("unknown suite");
    let (_, body) = cases
        .iter()
        .find(|(known, _)| *known == case)
        .expect("unknown case");
    // core-tests' main sets SwooleG.max_sockets = 20000 after swoole_init; swoole_init
    // takes it from the soft descriptor limit.
    // SAFETY: setrlimit(2) with a readable limit.
    unsafe { setrlimit(RLIMIT_NOFILE, &[20000, 20000]) };
    // SAFETY: once per process, before any other Swoole call.
    unsafe { swoole_init() };
    if suite == "coroutine_hook" {
        hook::prepare();
    }
    if suite == "process_pool" {
        process_pool::check_layout();
    }
    body();
    // SAFETY: after the case, as core-tests' main.
    unsafe { swoole_clean() };
}
