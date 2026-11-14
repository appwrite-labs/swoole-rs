//! Upstream's `core-tests/src/reactor/base.cpp`, one Rust function per TEST with the same
//! inputs and expected values, over the crates and the C++ tests/modules/hooks/system.py builds. `fixture.cc` gives the
//! Reactor, SocketPair and C++-linkage APIs C entry points; the handlers, callbacks and every
//! assertion are here. Cargo.toml names the build script that links them.
//!
//! `main --list` prints the cases; `main NAME` runs one in this process, as core-tests' main
//! does (`swoole_init`, the case, `swoole_clean`), and exits 0 when every assertion held.
//! A failed ASSERT panics (inside a handler called from C++ that aborts); a failed EXPECT is
//! printed and fails the case at exit, as GoogleTest's nonfatal assertions do.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::mem::offset_of;
use std::ptr::{self, null_mut};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering::Relaxed};
use std::time::{SystemTime, UNIX_EPOCH};
// The crates, once: the C++ in libswoole.a and the cases call them by symbol.
use swoole_rs_ffi as _;

const SW_OK: c_int = 0;
const SW_ERR: c_int = -1;
const SW_EVENT_READ: c_int = 1 << 9;
const SW_EVENT_WRITE: c_int = 1 << 10;
const SW_EVENT_ERROR: c_int = 1 << 11;
const SW_EVENT_ONCE: c_int = 1 << 12;
const SW_EVENTLOOP_WAIT_EXIT: c_int = 1;
const SW_REACTOR_MAXEVENTS: u32 = 4096;
const SW_PIPE_CLOSE_WORKER: c_int = 2;
// enum swFdType.
const SW_FD_SESSION: c_int = 0;
const SW_FD_PIPE: c_int = 3;
const SW_FD_STREAM: c_int = 4;
const SW_FD_AIO: c_int = 5;
const SW_FD_CO_SOCKET: c_int = 6;
const SW_FD_CO_POLL: c_int = 7;
const SW_FD_CO_EVENT: c_int = 8;
const SW_FD_USER: c_int = 16;
const SW_FD_STREAM_CLIENT: c_int = 17;
// enum swGlobalHookType.
const SW_GLOBAL_HOOK_ON_REACTOR_CREATE: c_int = 5;
const SW_GLOBAL_HOOK_ON_REACTOR_DESTROY: c_int = 9;
// Reactor::Type and Reactor::EndCallback.
const TYPE_AUTO: c_int = 0;
const TYPE_EPOLL: c_int = 1;
const TYPE_POLL: c_int = 3;
const PRIORITY_DEFER_TASK: c_int = 1;
// glibc's values (the core-tests build is Linux).
const EINTR: c_int = 4;
const EBADF: c_int = 9;
const EEXIST: c_int = 17;
const EINVAL: c_int = 22;
const EMFILE: c_int = 24;
const SIGIO: c_int = 29;
const SOCK_STREAM: c_int = 1;
const SOCK_DGRAM: c_int = 2;
const RLIMIT_NOFILE: c_int = 7;

const DATA_SIZE: usize = 2 * 1_000_000;
const PKT: &[u8] = b"hello world\r\n";
const HELLO: &[u8] = b"hello world\0";

enum Reactor {}
enum Event {}
enum SocketPair {}

/// `network::Socket`'s leading fields (native/reactor/swoole_rs_socket.h); `main` checks the
/// offsets against the fixture's.
#[repr(C)]
struct Socket {
    fd: c_int,
    fd_type: c_int,
    socket_type: c_int,
    events: c_int,
    enable_tcp_nodelay: bool,
    kernel_nobufs: bool,
    removed: u8,
    flags: [u8; 17],
    chunk_size: u32,
    out_buffer: *mut c_void,
    send_timer: u64,
    buffer_size: u32,
    close_wait: u8,
    nonblock: u8,
    ssl_send: u8,
    send_wait: u8,
    object: *mut c_void,
    ssl: *mut c_void,
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

#[repr(C)]
struct ReactorView {
    kind: c_int,
    max_event_num: u32,
    ready: bool,
    timed_out: bool,
    write: *const c_void,
    close: *const c_void,
    defer_tasks: *const c_void,
    ptr: *mut c_void,
}

#[repr(C)]
struct Iovec {
    base: *const c_void,
    len: usize,
}

type Handler = extern "C" fn(*mut Reactor, *mut Event) -> c_int;
type Plain = extern "C" fn(*mut c_void);

unsafe extern "C" {
    #[link_name = "_Z11swoole_initv"]
    fn swoole_init();
    #[link_name = "_Z12swoole_cleanv"]
    fn swoole_clean();
    fn swr_v1_core_last_error() -> c_int;

    static swr_case_error_operation_not_support: c_int;
    static swr_case_error_output_buffer_overflow: c_int;
    static swr_case_error_package_length_too_large: c_int;
    static swr_case_error_socket_not_exists: c_int;
    static swr_case_error_event_remove_failed: c_int;
    static swr_case_socket_offsets: [usize; 8];

    // crates/reactor's `new Reactor(max_event, type)` and `delete reactor`.
    #[link_name = "swr_v1_reactor_new"]
    fn swr_case_reactor_new(max_event: c_int, kind: c_int) -> *mut Reactor;
    #[link_name = "swr_v1_reactor_delete"]
    fn swr_case_reactor_delete(reactor: *mut Reactor);
    fn swr_case_tg_reactor() -> *mut Reactor;
    fn swr_case_set_tg_reactor(reactor: *mut Reactor);
    fn swr_case_reactor_view_of(reactor: *const Reactor) -> ReactorView;
    fn swr_case_reactor_flags(reactor: *mut Reactor, wait_exit: c_int, once: c_int, running: c_int);
    fn swr_case_reactor_set_max_event_num(reactor: *mut Reactor, n: u32);
    fn swr_case_reactor_set_timeout_msec(reactor: *mut Reactor, msec: c_int);
    fn swr_case_reactor_set_ptr(reactor: *mut Reactor, ptr: *mut c_void);
    fn swr_case_reactor_change(
        reactor: *mut Reactor,
        socket: *mut Socket,
        op: c_int,
        events: c_int,
    ) -> c_int;
    fn swr_case_reactor_wait(reactor: *mut Reactor) -> c_int;
    fn swr_case_reactor_set_handler(
        reactor: *mut Reactor,
        fd_type: c_int,
        event: c_int,
        handler: *const c_void,
    );
    fn swr_case_reactor_isset_handler(reactor: *mut Reactor, fd_type: c_int, event: c_int) -> bool;
    fn swr_case_reactor_exists_fd(reactor: *mut Reactor, fd: c_int) -> bool;
    fn swr_case_reactor_event_num(reactor: *mut Reactor) -> usize;
    fn swr_case_reactor_set_end_callback(
        reactor: *mut Reactor,
        id: c_int,
        f: extern "C" fn(*mut Reactor),
    );
    fn swr_case_reactor_erase_end_callback(reactor: *mut Reactor, id: c_int);
    fn swr_case_event_defer(f: Plain, data: *mut c_void);
    fn swr_case_reactor_set_task(reactor: *mut Reactor, which: c_int, f: Plain, data: *mut c_void);
    fn swr_case_reactor_drain_write_buffer(reactor: *mut Reactor, socket: *mut Socket);
    fn swr_case_event_socket(event: *const Event) -> *mut Socket;

    fn swr_case_event_init(flags: c_int) -> c_int;
    fn swr_case_event_free() -> c_int;
    fn swr_case_event_wait() -> c_int;
    fn swr_case_event_add(socket: *mut Socket, events: c_int) -> c_int;
    fn swr_case_event_write(socket: *mut Socket, data: *const c_void, len: usize) -> isize;
    fn swr_case_event_writev(socket: *mut Socket, iov: *const Iovec, iovcnt: usize) -> isize;
    fn swr_case_event_get_socket(fd: c_int) -> *mut Socket;
    fn swr_case_event_set_handler(fd_type: c_int, event: c_int, handler: *const c_void);
    fn swr_case_event_is_available() -> bool;

    fn swr_case_pair_new(kind: c_int, blocking: bool, protocol: c_int) -> *mut SocketPair;
    fn swr_case_pair_delete(pair: *mut SocketPair);
    fn swr_case_pair_ready(pair: *const SocketPair) -> bool;
    fn swr_case_pair_socket(pair: *const SocketPair, master: bool) -> *mut Socket;
    fn swr_case_pair_set_blocking(pair: *mut SocketPair, blocking: bool);
    fn swr_case_unix_socket_set_buffer_size(pair: *const SocketPair, size: usize) -> bool;
    fn swr_case_pair_close(pair: *mut SocketPair, which: c_int) -> bool;

    fn swr_case_socket_new() -> *mut Socket;
    fn swr_case_make_socket(fd: c_int, fd_type: c_int) -> *mut Socket;
    fn swr_case_socket_free(socket: *mut Socket);
    fn swr_case_socket_move_fd(socket: *mut Socket) -> c_int;
    fn swr_case_socket_out_buffer_length(socket: *const Socket) -> u32;
    fn swr_case_socket_wait_event(socket: *const Socket, timeout_ms: c_int, events: c_int)
    -> c_int;

    fn swr_case_add_hook(kind: c_int, f: Plain);
    fn swr_case_clear_last_error();
    fn swr_case_set_print_backtrace_on_error(enable: bool);
    fn swr_case_signal_set(
        signo: c_int,
        handler: extern "C" fn(c_int),
        restart: c_int,
        mask: c_int,
    );
    fn swr_case_signal_block_all();
    fn swr_v1_timer_add_plain(
        ms: i64,
        persistent: bool,
        f: Plain,
        data: *mut c_void,
    ) -> *mut c_void;
    fn swr_case_random_bytes(buf: *mut c_char, size: usize) -> usize;
    fn swr_case_tg_buffer() -> *mut CppString;
}

unsafe extern "C" {
    fn read(fd: c_int, buf: *mut c_void, count: usize) -> isize;
    fn write(fd: c_int, buf: *const c_void, count: usize) -> isize;
    fn pipe(fds: *mut c_int) -> c_int;
    fn dup(fd: c_int) -> c_int;
    fn dup2(old: c_int, new: c_int) -> c_int;
    fn close(fd: c_int) -> c_int;
    fn kill(pid: c_int, sig: c_int) -> c_int;
    fn getpid() -> c_int;
    fn usleep(usec: u32) -> c_int;
    fn getrlimit(resource: c_int, limit: *mut [u64; 2]) -> c_int;
    fn setrlimit(resource: c_int, limit: *const [u64; 2]) -> c_int;
    fn __errno_location() -> *mut c_int;
}

static EXPECT_FAILED: AtomicBool = AtomicBool::new(false);

/// GoogleTest's EXPECT_*: report and carry on.
macro_rules! expect_eq {
    ($left:expr, $right:expr) => {{
        let (left, right) = ($left, $right);
        if left != right {
            eprintln!(
                "{}:{}: expected {:?} == {:?}",
                file!(),
                line!(),
                left,
                right
            );
            EXPECT_FAILED.store(true, Relaxed);
        }
    }};
}

fn errno() -> c_int {
    // SAFETY: the calling thread's errno.
    unsafe { *__errno_location() }
}

fn last_error() -> c_int {
    // SAFETY: reads the thread's Swoole error.
    unsafe { swr_v1_core_last_error() }
}

fn microtime() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

fn handler(f: Handler) -> *const c_void {
    f as *const c_void
}

unsafe fn sock(pair: *const SocketPair, master: bool) -> *mut Socket {
    unsafe { swr_case_pair_socket(pair, master) }
}

unsafe fn add(reactor: *mut Reactor, socket: *mut Socket, events: c_int) -> c_int {
    unsafe { swr_case_reactor_change(reactor, socket, 0, events) }
}

unsafe fn set(reactor: *mut Reactor, socket: *mut Socket, events: c_int) -> c_int {
    unsafe { swr_case_reactor_change(reactor, socket, 1, events) }
}

unsafe fn del(reactor: *mut Reactor, socket: *mut Socket) -> c_int {
    unsafe { swr_case_reactor_change(reactor, socket, 2, 0) }
}

unsafe fn ev_socket(event: *mut Event) -> *mut Socket {
    unsafe { swr_case_event_socket(event) }
}

unsafe fn sock_read(socket: *const Socket, buf: &mut [u8]) -> isize {
    unsafe { read((*socket).fd, buf.as_mut_ptr().cast(), buf.len()) }
}

unsafe fn sock_write(socket: *const Socket, data: &[u8]) -> isize {
    unsafe { write((*socket).fd, data.as_ptr().cast(), data.len()) }
}

/// A `String str(DATA_SIZE)` holding `DATA_SIZE - 1` random bytes and a NUL.
fn random_data() -> Vec<u8> {
    let mut data = vec![0u8; DATA_SIZE];
    // SAFETY: the buffer holds DATA_SIZE bytes.
    unsafe { swr_case_random_bytes(data.as_mut_ptr().cast(), DATA_SIZE - 1) };
    data
}

static REACTOR_CREATE_COUNT: AtomicI32 = AtomicI32::new(0);
static REACTOR_DESTROY_COUNT: AtomicI32 = AtomicI32::new(0);

/// crates/reactor's handle on `reactor`.
///
/// # Safety
/// `reactor` is the case's live reactor, on this thread.
unsafe fn running(reactor: *mut Reactor) -> swoole_rs_ffi::Running<'static> {
    // SAFETY: as the caller guarantees; the cases hold no reference to its state.
    unsafe { swoole_rs_ffi::Running::new(reactor.cast()) }
}

/// `swoole_event_add_or_update(socket, event)`, which is crates/reactor's.
///
/// # Safety
/// `socket` is a live socket that stays one while registered.
unsafe fn event_add_or_update(socket: *mut Socket, event: c_int) -> c_int {
    // SAFETY: as the caller guarantees.
    let socket = unsafe { swoole_rs_ffi::Socket::from_ptr(socket.cast()) }.unwrap();
    if swoole_rs_ffi::event_api::add_or_update(socket, event) {
        SW_OK
    } else {
        SW_ERR
    }
}

/// `swoole_event_del(socket)`, which is crates/reactor's.
///
/// # Safety
/// `socket` is a live socket.
unsafe fn event_del(socket: *mut Socket) -> c_int {
    // SAFETY: as the caller guarantees.
    let socket = unsafe { swoole_rs_ffi::Socket::from_ptr(socket.cast()) }.unwrap();
    if swoole_rs_ffi::event_api::event_del(socket) {
        SW_OK
    } else {
        SW_ERR
    }
}

/// `pair->write(data, len)`, which is crates/tls's.
///
/// # Safety
/// `pair` is a live `SocketPair`.
unsafe fn pair_write(pair: *const SocketPair, data: &[u8]) -> isize {
    // SAFETY: as the caller guarantees; the C++ class is the Rust record.
    let pair = unsafe { &*pair.cast::<swoole_rs_ffi::socket_pair::SocketPair>() };
    swoole_rs_ffi::socket_pair::write_from(pair, data).map_or(-1, usize::cast_signed)
}

/// `reactor->get_handler(fd_type, event)`, as its address.
///
/// # Safety
/// As [`running`].
unsafe fn get_handler(reactor: *mut Reactor, fd_type: c_int, event: c_int) -> *const c_void {
    // SAFETY: as the caller guarantees.
    let reactor = unsafe { running(reactor) };
    swoole_rs_ffi::reactor_upstream::reactor_get_handler(reactor, fd_type, event)
        .map_or(ptr::null(), |handler| handler as *const c_void)
}

unsafe fn create() {
    unsafe {
        swr_case_event_init(0);
        let reactor = swr_case_tg_reactor();
        let view = swr_case_reactor_view_of(reactor);
        assert_eq!(view.max_event_num, SW_REACTOR_MAXEVENTS);
        assert!(view.ready);
        assert!(!view.write.is_null());
        assert!(!view.close.is_null());
        assert!(view.defer_tasks.is_null());
        assert!(
            swoole_rs_ffi::reactor_upstream::reactor_default_write_handler(running(reactor))
                .is_some()
        );
        // coroutine socket reactor, then the system reactor
        let all = [SW_EVENT_READ, SW_EVENT_WRITE, SW_EVENT_ERROR];
        for (fd_type, events) in [
            (SW_FD_CO_SOCKET, &all[..]),
            (SW_FD_CO_POLL, &all[..]),
            (SW_FD_CO_EVENT, &all[..]),
            (SW_FD_AIO, &all[..1]),
        ] {
            for &event in events {
                assert!(!get_handler(reactor, fd_type, event).is_null());
            }
        }
        swr_case_event_free();
    }
}

extern "C" fn count_destroy(_: *mut c_void) {
    REACTOR_DESTROY_COUNT.fetch_add(1, Relaxed);
}

/// ON_SCOPE_EXIT { setrlimit(RLIMIT_NOFILE, &original_limit); }
struct RestoreLimit([u64; 2]);

impl Drop for RestoreLimit {
    fn drop(&mut self) {
        // SAFETY: setrlimit(2) with a readable limit.
        unsafe { setrlimit(RLIMIT_NOFILE, &self.0) };
    }
}

unsafe fn create_failure() {
    unsafe {
        assert!(swr_case_tg_reactor().is_null());
        REACTOR_DESTROY_COUNT.store(0, Relaxed);
        swr_case_add_hook(SW_GLOBAL_HOOK_ON_REACTOR_DESTROY, count_destroy);

        let mut original_limit = [0u64; 2];
        assert_eq!(getrlimit(RLIMIT_NOFILE, &mut original_limit), 0);
        let limit = [0, original_limit[1]];
        assert_eq!(setrlimit(RLIMIT_NOFILE, &limit), 0);
        let _restore = RestoreLimit(original_limit);

        swr_case_clear_last_error();
        let retval = swr_case_event_init(0);
        let error = last_error();
        let restore_result = setrlimit(RLIMIT_NOFILE, &original_limit);

        assert_eq!(restore_result, 0);
        assert_eq!(retval, SW_ERR);
        assert_eq!(error, EMFILE);
        assert!(swr_case_tg_reactor().is_null());
        assert_eq!(REACTOR_DESTROY_COUNT.load(Relaxed), 1);

        assert_eq!(swr_case_event_init(0), SW_OK);
        assert_eq!(swr_case_event_free(), SW_OK);
        assert_eq!(REACTOR_DESTROY_COUNT.load(Relaxed), 2);
    }
}

unsafe fn set_handler() {
    unsafe {
        let reactor = swr_case_reactor_new(SW_REACTOR_MAXEVENTS as c_int, TYPE_AUTO);
        for (event, fake) in [
            (SW_EVENT_READ, 1usize),
            (SW_EVENT_WRITE, 2),
            (SW_EVENT_ERROR, 3),
        ] {
            swr_case_reactor_set_handler(reactor, SW_FD_SESSION, event, fake as *const c_void);
            assert!(swr_case_reactor_isset_handler(
                reactor,
                SW_FD_SESSION,
                event
            ));
            assert_eq!(
                get_handler(reactor, SW_FD_SESSION, event),
                fake as *const c_void
            );
        }
        swr_case_reactor_delete(reactor);
    }
}

/// wait's and write's handler: one "hello world" datagram, then the socket leaves.
extern "C" fn read_hello(reactor: *mut Reactor, event: *mut Event) -> c_int {
    let mut buffer = [0u8; 16];
    // SAFETY: the event's socket, live during dispatch.
    unsafe {
        let socket = ev_socket(event);
        let n = sock_read(socket, &mut buffer);
        expect_eq!(n, HELLO.len() as isize);
        expect_eq!(
            CStr::from_bytes_until_nul(&buffer).ok(),
            Some(c"hello world")
        );
        del(reactor, socket);
    }
    SW_OK
}

unsafe fn wait() {
    unsafe {
        let p = swr_case_pair_new(0, true, SOCK_DGRAM);
        assert!(swr_case_pair_ready(p));

        assert_eq!(swr_case_event_init(SW_EVENTLOOP_WAIT_EXIT), SW_OK);
        assert!(!swr_case_tg_reactor().is_null());
        assert!(swr_case_reactor_view_of(swr_case_tg_reactor()).ready);

        swr_case_event_set_handler(SW_FD_PIPE, SW_EVENT_READ, handler(read_hello));

        let ret = swr_case_event_add(sock(p, false), SW_EVENT_READ);
        assert_eq!(
            swr_case_event_get_socket((*sock(p, false)).fd),
            sock(p, false)
        );
        assert_eq!(ret, SW_OK);

        assert_eq!(pair_write(p, HELLO), HELLO.len() as isize);

        assert_eq!(swr_case_event_wait(), SW_OK);
        assert!(swr_case_tg_reactor().is_null());
        swr_case_pair_delete(p);
    }
}

unsafe fn get_missing_socket() {
    unsafe {
        let p = swr_case_pair_new(0, true, SOCK_DGRAM);
        assert!(swr_case_pair_ready(p));
        assert_eq!(swr_case_event_init(0), SW_OK);

        let socket = sock(p, false);
        let fd = (*socket).fd;

        expect_eq!(swr_case_event_get_socket(fd), null_mut());
        expect_eq!(swr_case_reactor_exists_fd(swr_case_tg_reactor(), fd), false);
        expect_eq!(swr_case_reactor_event_num(swr_case_tg_reactor()), 0);
        expect_eq!(swr_case_event_add(socket, SW_EVENT_READ), SW_OK);
        expect_eq!(swr_case_event_get_socket(fd), socket);
        expect_eq!(event_del(socket), SW_OK);
        expect_eq!(swr_case_event_free(), SW_OK);
        swr_case_pair_delete(p);
    }
}

unsafe fn write_case() {
    unsafe {
        let p = swr_case_pair_new(0, true, SOCK_DGRAM);
        assert!(swr_case_pair_ready(p));
        swr_case_pair_set_blocking(p, false);
        swr_case_unix_socket_set_buffer_size(p, 65536);

        assert_eq!(swr_case_event_init(SW_EVENTLOOP_WAIT_EXIT), SW_OK);
        assert!(!swr_case_tg_reactor().is_null());

        swr_case_event_set_handler(SW_FD_PIPE, SW_EVENT_READ, handler(read_hello));
        assert_eq!(swr_case_event_add(sock(p, false), SW_EVENT_READ), SW_OK);

        let n = swr_case_event_write(sock(p, true), HELLO.as_ptr().cast(), HELLO.len());
        assert_eq!(n, HELLO.len() as isize);

        assert_eq!(swr_case_event_wait(), SW_OK);
        assert!(swr_case_tg_reactor().is_null());
        swr_case_pair_delete(p);
    }
}

unsafe fn wait_timeout() {
    unsafe {
        assert_eq!(swr_case_event_init(SW_EVENTLOOP_WAIT_EXIT), SW_OK);
        assert!(!swr_case_tg_reactor().is_null());

        swr_case_reactor_set_timeout_msec(swr_case_tg_reactor(), 30);
        let started_at = microtime();
        assert_eq!(swr_case_reactor_wait(swr_case_tg_reactor()), SW_OK);

        let dr = microtime() - started_at;
        assert!(dr >= 0.03, "{dr}");
        assert!(dr < 0.05, "{dr}");

        swr_case_event_free();
    }
}

unsafe fn wait_error() {
    unsafe {
        assert_eq!(swr_case_event_init(SW_EVENTLOOP_WAIT_EXIT), SW_OK);
        assert!(!swr_case_tg_reactor().is_null());

        // EINVAL: maxevents is less than or equal to zero.
        swr_case_reactor_set_max_event_num(swr_case_tg_reactor(), 0);
        assert_eq!(swr_case_reactor_wait(swr_case_tg_reactor()), SW_ERR);
        assert_eq!(errno(), EINVAL);

        swr_case_event_free();
    }
}

unsafe fn writev() {
    unsafe {
        // `network::Socket fake_sock{}`: a record on upstream's stack, never freed.
        let fake_sock = swr_case_socket_new();

        let p = swr_case_pair_new(0, true, SOCK_DGRAM);
        assert!(swr_case_pair_ready(p));

        (*fake_sock).fd = (*sock(p, true)).fd;

        swr_case_event_init(SW_EVENTLOOP_WAIT_EXIT);

        let iov = [
            Iovec {
                base: c"hello ".as_ptr().cast(),
                len: 6,
            },
            Iovec {
                base: c"world\n".as_ptr().cast(),
                len: 6,
            },
        ];
        assert_eq!(swr_case_event_writev(fake_sock, iov.as_ptr(), 2), 12);

        let mut buf = [0u8; 32];
        assert_eq!(sock_read(sock(p, false), &mut buf), 12);
        assert_eq!(&buf[..12], b"hello world\n");

        (*fake_sock).ssl = usize::MAX as *mut c_void;
        assert_eq!(swr_case_event_writev(fake_sock, iov.as_ptr(), 2), -1);
        assert_eq!(last_error(), swr_case_error_operation_not_support);

        swr_case_event_wait();
        swr_case_pair_delete(p);
    }
}

/// write_2m's handler: fill the thread buffer until DATA_SIZE bytes arrived.
extern "C" fn read_2m(reactor: *mut Reactor, event: *mut Event) -> c_int {
    // SAFETY: the event's socket and the thread buffer, live during dispatch.
    unsafe {
        let tg_buf = swr_case_tg_buffer();
        let socket = ev_socket(event);
        let n = read(
            (*socket).fd,
            (*tg_buf).str.add((*tg_buf).length).cast(),
            (*tg_buf).size - (*tg_buf).length,
        );
        if n <= 0 {
            return SW_ERR;
        }
        // `String::grow`: count the bytes read, doubling a buffer they fill.
        (*tg_buf).length += n as usize;
        if (*tg_buf).length == (*tg_buf).size {
            (*tg_buf.cast::<swoole_rs_ffi::string::CppString>()).reserve((*tg_buf).size * 2);
        }
        if (*tg_buf).length == DATA_SIZE {
            (*tg_buf.cast::<swoole_rs_ffi::string::CppString>()).append(b"\0");
            del(reactor, socket);
        }
    }
    SW_OK
}

unsafe fn write_2m() {
    unsafe {
        let p = swr_case_pair_new(0, true, SOCK_STREAM);
        assert!(swr_case_pair_ready(p));

        assert_eq!(swr_case_event_init(SW_EVENTLOOP_WAIT_EXIT), SW_OK);
        assert!(!swr_case_tg_reactor().is_null());

        swr_case_event_set_handler(SW_FD_PIPE, SW_EVENT_READ, handler(read_2m));

        swr_case_pair_set_blocking(p, false);
        swr_case_unix_socket_set_buffer_size(p, 65536);

        assert_eq!(swr_case_event_add(sock(p, false), SW_EVENT_READ), SW_OK);

        let data = random_data();

        let tg_buf = swr_case_tg_buffer();
        (*tg_buf).length = 0;
        (*tg_buf).offset = 0;

        let socket = sock(p, true);
        (*socket).buffer_size = 2 * 1024 * 1024;

        let n = swr_case_event_write(socket, data.as_ptr().cast(), data.len());
        assert_eq!(n, data.len() as isize);
        assert!(swr_case_socket_out_buffer_length(socket) > 1024);

        println!("{}", swr_case_socket_out_buffer_length(socket));

        assert_eq!(
            swr_case_event_write(socket, data.as_ptr().cast(), 256 * 1024),
            SW_ERR as isize
        );
        assert_eq!(last_error(), swr_case_error_output_buffer_overflow);

        assert_eq!(
            swr_case_event_write(
                socket,
                data.as_ptr().cast(),
                (*socket).buffer_size as usize + 8192
            ),
            SW_ERR as isize
        );
        assert_eq!(last_error(), swr_case_error_package_length_too_large);

        assert_eq!(swr_case_event_wait(), SW_OK);
        assert!(!swr_case_event_is_available());
        // ASSERT_STREQ: the two as C strings.
        assert_eq!(
            CStr::from_ptr((*swr_case_tg_buffer()).str),
            CStr::from_bytes_until_nul(&data).unwrap()
        );
        swr_case_pair_delete(p);
    }
}

unsafe fn bad_fd() {
    unsafe {
        swr_case_event_init(SW_EVENTLOOP_WAIT_EXIT);
        let socket = swr_case_make_socket(999999, SW_FD_STREAM_CLIENT);
        (*socket).nonblock = 1;
        let n = swr_case_event_write(socket, c"hello world".as_ptr().cast(), 11);
        assert_eq!(n, SW_ERR as isize);
        assert_eq!(last_error(), EBADF);
        swr_case_event_free();
        swr_case_socket_move_fd(socket);
        swr_case_socket_free(socket);
    }
}

extern "C" fn pipe_read(reactor: *mut Reactor, event: *mut Event) -> c_int {
    let mut buf = [0u8; 1024];
    // SAFETY: the event's socket, live during dispatch.
    unsafe {
        let socket = ev_socket(event);
        let n = sock_read(socket, &mut buf);
        expect_eq!(n, PKT.len() as isize);
        expect_eq!(&buf[..n.max(0) as usize], PKT);
        del(reactor, socket);
    }
    SW_OK
}

extern "C" fn pipe_write(reactor: *mut Reactor, event: *mut Event) -> c_int {
    // SAFETY: the event's socket, live during dispatch.
    unsafe {
        let socket = ev_socket(event);
        expect_eq!(sock_write(socket, PKT), PKT.len() as isize);
        del(reactor, socket);
    }
    SW_OK
}

extern "C" fn count_write(_: *mut Reactor, event: *mut Event) -> c_int {
    // SAFETY: the socket's object is reactor_test_func's write_count.
    unsafe { *(*ev_socket(event)).object.cast::<c_int>() += 1 };
    SW_OK
}

/// Upstream's reactor_test_func.
unsafe fn reactor_test_func(reactor: *mut Reactor) {
    unsafe {
        let p = swr_case_pair_new(1, false, 0);
        assert!(swr_case_pair_ready(p));

        swr_case_reactor_set_handler(reactor, SW_FD_PIPE, SW_EVENT_READ, handler(pipe_read));
        swr_case_reactor_set_handler(reactor, SW_FD_PIPE, SW_EVENT_WRITE, handler(pipe_write));

        assert_eq!(add(reactor, sock(p, false), SW_EVENT_READ), SW_OK);
        assert_eq!(add(reactor, sock(p, true), SW_EVENT_WRITE), SW_OK);

        let unsock = swr_case_pair_new(0, false, SOCK_STREAM);
        assert!(swr_case_pair_ready(unsock));

        let write_count: *mut c_int = Box::into_raw(Box::new(0));
        let sock2 = sock(unsock, false);
        (*sock2).object = write_count.cast();
        (*sock2).fd_type = SW_FD_STREAM;

        swr_case_reactor_set_handler(reactor, SW_FD_STREAM, SW_EVENT_WRITE, handler(count_write));
        assert_eq!(
            add(
                reactor,
                sock2,
                SW_FD_STREAM | SW_EVENT_WRITE | SW_EVENT_ONCE
            ),
            SW_OK
        );

        assert_eq!(*write_count, 0);
        assert_eq!(swr_case_reactor_wait(reactor), SW_OK);
        assert_eq!(*write_count, 1);

        drop(Box::from_raw(write_count));
        swr_case_pair_delete(unsock);
        swr_case_pair_delete(p);
    }
}

unsafe fn local_reactor(kind: c_int, body: impl FnOnce(*mut Reactor)) {
    unsafe {
        let reactor = swr_case_reactor_new(1024, kind);
        swr_case_reactor_flags(reactor, 1, -1, -1);
        body(reactor);
        swr_case_reactor_delete(reactor);
    }
}

unsafe fn epoll() {
    unsafe { local_reactor(TYPE_EPOLL, |r| reactor_test_func(r)) }
}

unsafe fn poll() {
    unsafe { local_reactor(TYPE_POLL, |r| reactor_test_func(r)) }
}

struct PollSocketState {
    sockets: [*mut Socket; 2],
    dispatched_socket: *mut Socket,
    replacement_socket: *mut Socket,
    source_fd: c_int,
    replacement_count: c_int,
}

extern "C" fn reuse_read(reactor: *mut Reactor, event: *mut Event) -> c_int {
    // SAFETY: the socket's object is the case's PollSocketState.
    unsafe {
        let socket = ev_socket(event);
        let state = &mut *(*socket).object.cast::<PollSocketState>();
        let mut data = [0u8; 1];
        expect_eq!(sock_read(socket, &mut data), 1);
        state.dispatched_socket = socket;

        let other_socket = if socket == state.sockets[0] {
            state.sockets[1]
        } else {
            state.sockets[0]
        };
        expect_eq!(del(reactor, other_socket), SW_OK);
        let fd = swr_case_socket_move_fd(other_socket);
        expect_eq!(dup2(state.source_fd, fd), fd);

        state.replacement_socket = swr_case_make_socket(fd, SW_FD_USER);
        (*state.replacement_socket).object = (&raw mut state.replacement_count).cast();
        expect_eq!(add(reactor, state.replacement_socket, SW_EVENT_READ), SW_OK);
    }
    SW_OK
}

extern "C" fn count_user(_: *mut Reactor, event: *mut Event) -> c_int {
    // SAFETY: the socket's object is the state's replacement_count.
    unsafe { *(*ev_socket(event)).object.cast::<c_int>() += 1 };
    SW_OK
}

unsafe fn poll_readiness_after_fd_reuse() {
    unsafe {
        let sockets = [
            swr_case_pair_new(0, false, SOCK_STREAM),
            swr_case_pair_new(0, false, SOCK_STREAM),
        ];
        assert!(swr_case_pair_ready(sockets[0]));
        assert!(swr_case_pair_ready(sockets[1]));

        let mut pipe_fds = [0 as c_int; 2];
        assert_eq!(pipe(pipe_fds.as_mut_ptr()), 0);

        let reactor = swr_case_reactor_new(32, TYPE_POLL);
        swr_case_reactor_flags(reactor, -1, 1, -1);

        let state = Box::into_raw(Box::new(PollSocketState {
            sockets: [sock(sockets[0], false), sock(sockets[1], false)],
            dispatched_socket: null_mut(),
            replacement_socket: null_mut(),
            source_fd: pipe_fds[0],
            replacement_count: 0,
        }));
        for &socket in &(*state).sockets {
            (*socket).fd_type = SW_FD_STREAM;
            (*socket).object = state.cast();
        }

        swr_case_reactor_set_handler(reactor, SW_FD_STREAM, SW_EVENT_READ, handler(reuse_read));
        swr_case_reactor_set_handler(reactor, SW_FD_USER, SW_EVENT_READ, handler(count_user));

        assert_eq!(add(reactor, (*state).sockets[0], SW_EVENT_READ), SW_OK);
        assert_eq!(add(reactor, (*state).sockets[1], SW_EVENT_READ), SW_OK);
        assert_eq!(sock_write(sock(sockets[0], true), b"x"), 1);
        assert_eq!(sock_write(sock(sockets[1], true), b"x"), 1);
        assert_eq!(swr_case_reactor_wait(reactor), SW_OK);

        assert_eq!((*state).replacement_count, 0);
        assert!(!(*state).dispatched_socket.is_null());
        assert!(!(*state).replacement_socket.is_null());

        // A local Reactor does not defer Socket::free(), so release sockets after the batch.
        assert_eq!(del(reactor, (*state).dispatched_socket), SW_OK);
        assert_eq!(del(reactor, (*state).replacement_socket), SW_OK);
        swr_case_socket_free((*state).replacement_socket);
        assert_eq!(close(pipe_fds[0]), 0);
        assert_eq!(close(pipe_fds[1]), 0);

        swr_case_reactor_delete(reactor);
        drop(Box::from_raw(state));
        swr_case_pair_delete(sockets[1]);
        swr_case_pair_delete(sockets[0]);
    }
}

/// `network::Socket s{}; s.fd = fd;`: records on upstream's stack, never freed.
unsafe fn fake_socket(fd: c_int) -> *mut Socket {
    unsafe {
        let socket = swr_case_socket_new();
        (*socket).fd = fd;
        socket
    }
}

unsafe fn poll_extra() {
    unsafe {
        let reactor = swr_case_reactor_new(32, TYPE_POLL);

        let fake_sock1 = fake_socket(12345);
        let fake_sock2 = fake_socket(99999);

        assert_eq!(add(reactor, fake_sock1, SW_EVENT_READ), SW_OK);
        assert_eq!(add(reactor, fake_sock2, SW_EVENT_READ), SW_OK);

        assert_eq!(add(reactor, fake_sock1, SW_EVENT_READ), SW_ERR);

        assert_eq!(swr_case_reactor_event_num(reactor), 2);

        assert_eq!(
            set(
                reactor,
                fake_sock2,
                SW_EVENT_READ | SW_EVENT_WRITE | SW_EVENT_ERROR
            ),
            SW_OK
        );

        assert_eq!(del(reactor, fake_sock2), SW_OK);
        assert_eq!(swr_case_reactor_event_num(reactor), 1);

        let fake_sock3 = fake_socket(88888);

        assert_eq!(
            set(reactor, fake_sock3, SW_EVENT_READ | SW_EVENT_WRITE),
            SW_ERR
        );
        assert_eq!(last_error(), swr_case_error_socket_not_exists);

        assert_eq!(del(reactor, fake_sock3), SW_ERR);
        assert_eq!(last_error(), swr_case_error_socket_not_exists);

        let fake_socks: Vec<*mut Socket> = (0..32).map(|i| fake_socket(i + 1024)).collect();
        for (i, &socket) in fake_socks.iter().enumerate() {
            assert_eq!(
                add(reactor, socket, SW_EVENT_READ),
                if i <= 30 { SW_OK } else { SW_ERR }
            );
        }
        for (i, &socket) in fake_socks.iter().enumerate().rev() {
            assert_eq!(del(reactor, socket), if i <= 30 { SW_OK } else { SW_ERR });
        }
        swr_case_reactor_delete(reactor);
    }
}

extern "C" fn end_timed_out(reactor: *mut Reactor) {
    // SAFETY: the reactor running its end callbacks.
    assert!(unsafe { swr_case_reactor_view_of(reactor) }.timed_out);
}

extern "C" fn on_sigio(_: c_int) {}

unsafe fn poll_extra2() {
    unsafe {
        let reactor = swr_case_reactor_new(32, TYPE_POLL);
        swr_case_reactor_flags(reactor, -1, 1, -1);
        swr_case_reactor_set_timeout_msec(reactor, 10);
        swr_case_reactor_set_end_callback(reactor, PRIORITY_DEFER_TASK, end_timed_out);
        assert_eq!(swr_case_reactor_wait(reactor), SW_OK);

        swr_case_signal_set(SIGIO, on_sigio, 0, 0);

        swr_case_reactor_erase_end_callback(reactor, PRIORITY_DEFER_TASK);
        swr_case_reactor_set_timeout_msec(reactor, 1000);

        let t = std::thread::spawn(|| {
            swr_case_signal_block_all();
            usleep(10000);
            kill(getpid(), SIGIO);
        });
        *__errno_location() = 0;
        assert_eq!(swr_case_reactor_wait(reactor), SW_OK);
        assert_eq!(errno(), EINTR);

        t.join().unwrap();
        swr_case_reactor_delete(reactor);
    }
}

unsafe fn add_or_update() {
    unsafe {
        let p = swr_case_pair_new(0, true, SOCK_DGRAM);
        assert!(swr_case_pair_ready(p));

        assert_eq!(swr_case_event_init(SW_EVENTLOOP_WAIT_EXIT), SW_OK);
        assert!(!swr_case_tg_reactor().is_null());

        let socket = sock(p, false);
        assert_eq!(event_add_or_update(socket, SW_EVENT_READ), SW_OK);
        assert!((*socket).events & SW_EVENT_READ != 0);

        assert_eq!(event_add_or_update(socket, SW_EVENT_WRITE), SW_OK);
        assert!((*socket).events & SW_EVENT_READ != 0);
        assert!((*socket).events & SW_EVENT_WRITE != 0);

        swr_case_event_free();
        swr_case_pair_delete(p);
    }
}

extern "C" fn increment(count: *mut c_void) {
    // SAFETY: the case's counter.
    unsafe { *count.cast::<c_int>() += 1 };
}

unsafe fn defer_task() {
    unsafe {
        swr_case_event_init(SW_EVENTLOOP_WAIT_EXIT);
        let reactor = swr_case_tg_reactor();
        assert_eq!(
            swr_case_reactor_view_of(reactor).max_event_num,
            SW_REACTOR_MAXEVENTS
        );

        let count: *mut c_int = Box::into_raw(Box::new(0));
        // The thread's reactor, which `reactor` is.
        swr_case_event_defer(increment, count.cast());
        swr_case_event_wait();
        assert_eq!(*count, 1);
        drop(Box::from_raw(count));
        swr_case_event_free();
    }
}

static EVENT_LOOP_COUNT: AtomicI32 = AtomicI32::new(0);

extern "C" fn future_task(data: *mut c_void) {
    // SAFETY: the task's data is the case's C string.
    assert_eq!(unsafe { CStr::from_ptr(data.cast()) }, c"hello world");
    EVENT_LOOP_COUNT.fetch_add(1, Relaxed);
}

unsafe fn cycle() {
    unsafe {
        local_reactor(TYPE_POLL, |reactor| {
            swr_case_reactor_set_task(
                reactor,
                0,
                future_task,
                c"hello world".as_ptr().cast_mut().cast(),
            );
            reactor_test_func(reactor);
            assert!(EVENT_LOOP_COUNT.load(Relaxed) > 0);
        })
    }
}

extern "C" fn event_idle_callback(data: *mut c_void) {
    // SAFETY: the task's data is the case's C string.
    assert_eq!(unsafe { CStr::from_ptr(data.cast()) }, c"hello world");
}

unsafe fn priority_idle_task() {
    unsafe {
        local_reactor(TYPE_POLL, |reactor| {
            swr_case_reactor_set_task(
                reactor,
                1,
                event_idle_callback,
                c"hello world".as_ptr().cast_mut().cast(),
            );
            reactor_test_func(reactor);
        })
    }
}

extern "C" fn hook_create(data: *mut c_void) {
    // SAFETY: the hook's data is the Reactor being created.
    assert_eq!(
        TYPE_POLL,
        unsafe { swr_case_reactor_view_of(data.cast()) }.kind
    );
    REACTOR_CREATE_COUNT.fetch_add(1, Relaxed);
}

extern "C" fn hook_destroy(data: *mut c_void) {
    // SAFETY: the hook's data is the Reactor being destroyed.
    assert_eq!(
        TYPE_POLL,
        unsafe { swr_case_reactor_view_of(data.cast()) }.kind
    );
    REACTOR_DESTROY_COUNT.fetch_add(1, Relaxed);
}

unsafe fn hook() {
    unsafe {
        REACTOR_CREATE_COUNT.store(0, Relaxed);
        REACTOR_DESTROY_COUNT.store(0, Relaxed);

        swr_case_add_hook(SW_GLOBAL_HOOK_ON_REACTOR_CREATE, hook_create);
        swr_case_add_hook(SW_GLOBAL_HOOK_ON_REACTOR_DESTROY, hook_destroy);

        local_reactor(TYPE_POLL, |reactor| reactor_test_func(reactor));

        assert_eq!(REACTOR_CREATE_COUNT.load(Relaxed), 1);
        assert_eq!(REACTOR_DESTROY_COUNT.load(Relaxed), 1);
    }
}

unsafe fn set_fd() {
    unsafe {
        let p = swr_case_pair_new(0, true, SOCK_DGRAM);
        for kind in [TYPE_EPOLL, TYPE_POLL] {
            let reactor = swr_case_reactor_new(1024, kind);
            assert_eq!(add(reactor, sock(p, false), SW_EVENT_READ), SW_OK);
            assert_eq!(set(reactor, sock(p, false), SW_EVENT_WRITE), SW_OK);
            swr_case_reactor_delete(reactor);
        }
        swr_case_pair_delete(p);
    }
}

extern "C" fn error_event_handler(reactor: *mut Reactor, event: *mut Event) -> c_int {
    // SAFETY: the reactor's ptr is test_error_event's retval.
    unsafe {
        expect_eq!(del(reactor, ev_socket(event)), SW_OK);
        swr_case_reactor_flags(reactor, -1, -1, 0);
        *swr_case_reactor_view_of(reactor).ptr.cast::<c_int>()
    }
}

unsafe fn test_error_event(kind: c_int, mut retval: c_int) {
    unsafe {
        let p = swr_case_pair_new(1, true, 0);
        assert!(swr_case_pair_ready(p));

        let reactor = swr_case_reactor_new(1024, kind);
        swr_case_set_tg_reactor(reactor);

        swr_case_reactor_set_ptr(reactor, (&raw mut retval).cast());

        swr_case_reactor_set_handler(
            reactor,
            SW_FD_PIPE,
            SW_EVENT_ERROR,
            handler(error_event_handler),
        );

        add(reactor, sock(p, true), SW_EVENT_ERROR);
        add(reactor, sock(p, false), SW_EVENT_ERROR);

        swr_case_pair_close(p, SW_PIPE_CLOSE_WORKER);
        assert_eq!(swr_case_reactor_wait(reactor), SW_OK);
        swr_case_reactor_delete(reactor);
        swr_case_set_tg_reactor(null_mut());
        swr_case_pair_delete(p);
    }
}

unsafe fn error_event() {
    unsafe {
        test_error_event(TYPE_EPOLL, SW_OK);
        test_error_event(TYPE_POLL, SW_OK);

        test_error_event(TYPE_EPOLL, SW_ERR);
        test_error_event(TYPE_POLL, SW_ERR);
    }
}

unsafe fn error() {
    unsafe {
        let p = swr_case_pair_new(0, true, SOCK_DGRAM);
        let worker = sock(p, false);

        swr_case_set_print_backtrace_on_error(true);

        let reactor = swr_case_reactor_new(1024, TYPE_EPOLL);
        assert_eq!(add(reactor, worker, SW_EVENT_READ), SW_OK);
        assert_eq!(add(reactor, worker, SW_EVENT_WRITE), SW_ERR);
        assert_eq!(errno(), EEXIST);

        // `network::Socket bad_sock;`: a record on upstream's stack, never freed.
        let bad_sock = swr_case_socket_new();
        (*bad_sock).removed = 1;
        (*bad_sock).fd_type = SW_FD_PIPE;
        (*bad_sock).fd = dup((*worker).fd);
        assert_eq!(del(reactor, bad_sock), SW_ERR);
        assert_eq!(last_error(), swr_case_error_event_remove_failed);

        assert_eq!(add(reactor, bad_sock, SW_EVENT_READ), SW_OK);
        close((*bad_sock).fd);

        #[expect(clippy::eq_op, reason = "upstream's input, base.cpp line 721")]
        let events = SW_EVENT_READ | SW_EVENT_READ;
        assert_eq!(set(reactor, bad_sock, events), SW_ERR);
        assert_eq!(errno(), EBADF);

        assert_eq!(del(reactor, bad_sock), SW_OK);

        swr_case_reactor_delete(reactor);

        let reactor = swr_case_reactor_new(1024, TYPE_POLL);
        assert_eq!(add(reactor, worker, SW_EVENT_READ), SW_OK);
        assert_eq!(del(reactor, worker), SW_OK);
        assert_eq!(del(reactor, worker), SW_ERR);
        assert_eq!(last_error(), swr_case_error_socket_not_exists);
        swr_case_reactor_delete(reactor);
        swr_case_pair_delete(p);
    }
}

unsafe fn drain_write_buffer() {
    unsafe {
        let p = swr_case_pair_new(0, true, SOCK_STREAM);
        assert!(swr_case_pair_ready(p));

        assert_eq!(swr_case_event_init(SW_EVENTLOOP_WAIT_EXIT), SW_OK);

        swr_case_pair_set_blocking(p, false);
        swr_case_unix_socket_set_buffer_size(p, 65536);

        let data = random_data();

        let wsock = sock(p, true);

        let n = swr_case_event_write(wsock, data.as_ptr().cast(), data.len());
        assert_eq!(n, data.len() as isize);
        assert!(swr_case_socket_out_buffer_length(wsock) > 1024);

        let rsock = sock(p, false) as usize;
        std::thread::scope(|scope| {
            let t = scope.spawn(|| {
                usleep(10000);
                let rsock = rsock as *mut Socket;
                let mut rbuf = vec![0u8; DATA_SIZE];
                let mut length = 0;
                loop {
                    swr_case_socket_wait_event(rsock, 1000, SW_EVENT_READ);
                    let n = sock_read(rsock, &mut rbuf[length..]);
                    if n > 0 {
                        length += n as usize;
                        if length == rbuf.len() {
                            break;
                        }
                    }
                }
                assert!(rbuf == data);
            });

            swr_case_reactor_drain_write_buffer(swr_case_tg_reactor(), wsock);

            assert_eq!(swr_case_event_wait(), SW_OK);
            assert!(!swr_case_event_is_available());
            t.join().unwrap();
        });
        swr_case_pair_delete(p);
    }
}

extern "C" fn write_pkt(pair: *mut c_void) {
    // SAFETY: the timer's data is handle_fail's UnixSocket.
    unsafe { sock_write(sock(pair.cast(), true), PKT) };
}

extern "C" fn fail_read(reactor: *mut Reactor, event: *mut Event) -> c_int {
    let mut buffer = [0u8; 16];
    // SAFETY: the event's socket, live during dispatch.
    unsafe {
        let socket = ev_socket(event);
        let n = sock_read(socket, &mut buffer);
        expect_eq!(n, PKT.len() as isize);
        expect_eq!(&buffer[..n.max(0) as usize], PKT);
        expect_eq!(del(reactor, socket), 0);

        expect_eq!(swr_case_reactor_event_num(reactor), 0);
    }
    SW_ERR
}

extern "C" fn fail_write(reactor: *mut Reactor, event: *mut Event) -> c_int {
    // SAFETY: the socket's object is handle_fail's UnixSocket.
    unsafe {
        let socket = ev_socket(event);
        expect_eq!(set(reactor, socket, SW_EVENT_READ), 0);
        swr_v1_timer_add_plain(10, false, write_pkt, (*socket).object);
    }
    SW_ERR
}

unsafe fn handle_fail() {
    unsafe {
        let p = swr_case_pair_new(0, true, SOCK_DGRAM);
        assert!(swr_case_pair_ready(p));

        assert_eq!(swr_case_event_init(SW_EVENTLOOP_WAIT_EXIT), SW_OK);
        assert!(!swr_case_tg_reactor().is_null());

        swr_case_event_set_handler(SW_FD_PIPE, SW_EVENT_READ, handler(fail_read));
        swr_case_event_set_handler(SW_FD_PIPE, SW_EVENT_WRITE, handler(fail_write));

        let socket = sock(p, false);
        (*socket).object = p.cast();

        assert_eq!(
            swr_case_event_add(socket, SW_EVENT_READ | SW_EVENT_WRITE),
            SW_OK
        );

        assert_eq!(swr_case_event_wait(), SW_OK);
        assert!(swr_case_tg_reactor().is_null());
        swr_case_pair_delete(p);
    }
}

const CASES: &[(&str, unsafe fn())] = &[
    ("create", create),
    ("create_failure", create_failure),
    ("set_handler", set_handler),
    ("wait", wait),
    ("get_missing_socket", get_missing_socket),
    ("write", write_case),
    ("wait_timeout", wait_timeout),
    ("wait_error", wait_error),
    ("writev", writev),
    ("write_2m", write_2m),
    ("bad_fd", bad_fd),
    ("epoll", epoll),
    ("poll", poll),
    (
        "poll_readiness_after_fd_reuse",
        poll_readiness_after_fd_reuse,
    ),
    ("poll_extra", poll_extra),
    ("poll_extra2", poll_extra2),
    ("add_or_update", add_or_update),
    ("defer_task", defer_task),
    ("cycle", cycle),
    ("priority_idle_task", priority_idle_task),
    ("hook", hook),
    ("set_fd", set_fd),
    ("error_event", error_event),
    ("error", error),
    ("drain_write_buffer", drain_write_buffer),
    ("handle_fail", handle_fail),
];

fn main() {
    let name = std::env::args().nth(1).expect("usage: main --list | NAME");
    if name == "--list" {
        for (case, _) in CASES {
            println!("reactor.{case}");
        }
        return;
    }
    let case = name.strip_prefix("reactor.").expect("reactor.NAME");
    let (_, body) = CASES
        .iter()
        .find(|(known, _)| *known == case)
        .expect("unknown case");
    let mirrored = [
        offset_of!(Socket, fd),
        offset_of!(Socket, fd_type),
        offset_of!(Socket, events),
        offset_of!(Socket, removed),
        offset_of!(Socket, buffer_size),
        offset_of!(Socket, nonblock),
        offset_of!(Socket, object),
        offset_of!(Socket, ssl),
    ];
    // SAFETY: a constant table in the fixture.
    assert_eq!(
        mirrored,
        unsafe { ptr::read(&raw const swr_case_socket_offsets) },
        "network::Socket layout"
    );
    // core-tests' main sets SwooleG.max_sockets = 20000 after swoole_init; swoole_init takes
    // it from the soft descriptor limit.
    // SAFETY: setrlimit(2) with a readable limit.
    unsafe { setrlimit(RLIMIT_NOFILE, &[20000, 20000]) };
    // SAFETY: once per process, before any other Swoole call; the case, then cleanup.
    unsafe {
        swoole_init();
        body();
        swoole_clean();
    }
    if EXPECT_FAILED.load(Relaxed) {
        std::process::exit(1);
    }
}
