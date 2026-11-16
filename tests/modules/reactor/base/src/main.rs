//! Upstream's `core-tests/src/reactor/base.cpp`, one Rust function per TEST with the same
//! inputs and expected values, over the crates' Rust API (the reactor, its event API, the
//! socket pairs and the hooks), linked with the C and C++ tests/modules/hooks/system.py builds.
//! Cargo.toml names the build script that links them.
//!
//! `main --list` prints the cases; `main NAME` runs one in this process, as core-tests' main
//! does (`swoole_init`, the case, `swoole_clean`), and exits 0 when every assertion held.
//! A failed ASSERT panics (inside a handler called from the loop that aborts); a failed EXPECT
//! is printed and fails the case at exit, as GoogleTest's nonfatal assertions do.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::io::IoSlice;
use std::ptr::{self, null_mut};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, Ordering::Relaxed};
use std::time::{SystemTime, UNIX_EPOCH};
use swoole_rs_ffi::socket_pair::{self, SocketPair};
use swoole_rs_ffi::{
    ChangeOp, ErrorCode, Event, Events, Handler, OwnedSocket, ReactorFields, ReactorPrepared,
    ReactorState, Running, Socket, SocketRecord, event_api, reactor_upstream,
};

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

/// A `network::Socket *`.
type Sock = *mut SocketRecord;

/// `swoole::String`.
#[repr(C)]
struct CppString {
    length: usize,
    size: usize,
    offset: i64,
    str: *mut c_char,
    allocator: *const c_void,
}

unsafe extern "C" {
    #[link_name = "_Z11swoole_initv"]
    fn swoole_init();
    #[link_name = "_Z12swoole_cleanv"]
    fn swoole_clean();
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
    swoole_rs_ffi::log::last_error()
}

fn error(code: ErrorCode) -> c_int {
    code as c_int
}

fn microtime() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

/// `SwooleTG.reactor`.
fn tg_reactor() -> *mut c_void {
    swoole_rs_ffi::thread_reactor()
}

/// The thread's reactor, which the case made.
fn tg() -> Running<'static> {
    let reactor = tg_reactor();
    assert!(!reactor.is_null());
    // SAFETY: the thread's live reactor, until the case frees it.
    unsafe { Running::new(reactor) }
}

thread_local! {
    /// The reactors `reactor_new` made and `reactor_delete` has not deleted.
    static OWNED: std::cell::RefCell<Vec<swoole_rs_ffi::OwnedReactor>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// `new Reactor(max_event, type)`.
fn reactor_new(max_event: c_int, kind: c_int) -> Running<'static> {
    let owned = swoole_rs_ffi::swr_v1_reactor_new(max_event, kind);
    let reactor = owned.as_ptr();
    OWNED.with_borrow_mut(|owned_list| owned_list.push(owned));
    // SAFETY: the new reactor, on this thread, live until `reactor_delete`.
    unsafe { Running::new(reactor) }
}

/// `delete reactor`.
unsafe fn reactor_delete(reactor: Running<'_>) {
    let owned = OWNED.with_borrow_mut(|owned_list| {
        let at = owned_list
            .iter()
            .position(|owned| owned.as_ptr() == reactor.as_ptr())
            .expect("a reactor from reactor_new");
        owned_list.swap_remove(at)
    });
    swoole_rs_ffi::swr_v1_reactor_delete(owned);
}

/// The reactor's public fields.
fn fields(reactor: Running<'_>) -> *mut ReactorFields {
    ReactorPrepared::of(reactor.as_ptr()).fields
}

/// `reactor->get_socket(fd)`.
unsafe fn get_socket(reactor: Running<'_>, fd: c_int) -> *mut c_void {
    // SAFETY: the live reactor's registry.
    let registry = unsafe { &*ReactorPrepared::of(reactor.as_ptr()).registry };
    swoole_rs_ffi::swr_v1_reactor_registry_get(Some(registry), fd)
}

/// `reactor->get_event_num()`.
unsafe fn event_num(reactor: Running<'_>) -> usize {
    // SAFETY: the live reactor's registry.
    let registry = unsafe { &*ReactorPrepared::of(reactor.as_ptr()).registry };
    swoole_rs_ffi::swr_v1_reactor_registry_count(Some(registry))
}

/// The live socket `socket` points to.
unsafe fn handle(socket: Sock) -> Socket<'static> {
    // SAFETY: as the caller guarantees: live while the case uses or registers it.
    unsafe { Socket::from_ptr(socket) }.unwrap()
}

/// SW_OK or SW_ERR for `ok`.
fn status(ok: bool) -> c_int {
    if ok { SW_OK } else { SW_ERR }
}

unsafe fn add(reactor: Running<'_>, socket: Sock, events: c_int) -> c_int {
    status(reactor_upstream::change(
        reactor,
        unsafe { handle(socket) },
        ChangeOp::Add,
        Events::from_bits(events),
    ))
}

unsafe fn set(reactor: Running<'_>, socket: Sock, events: c_int) -> c_int {
    status(reactor_upstream::change(
        reactor,
        unsafe { handle(socket) },
        ChangeOp::Set,
        Events::from_bits(events),
    ))
}

unsafe fn del(reactor: Running<'_>, socket: Sock) -> c_int {
    status(reactor_upstream::change(
        reactor,
        unsafe { handle(socket) },
        ChangeOp::Del,
        Events::NONE,
    ))
}

fn ev_socket(event: &Event) -> Sock {
    event.socket().map_or(null_mut(), Socket::as_ptr)
}

unsafe fn sock_read(socket: Sock, buf: &mut [u8]) -> isize {
    unsafe { read((*socket).fields.fd, buf.as_mut_ptr().cast(), buf.len()) }
}

unsafe fn sock_write(socket: Sock, data: &[u8]) -> isize {
    unsafe { write((*socket).fields.fd, data.as_ptr().cast(), data.len()) }
}

/// `new UnixSocket(blocking, protocol)`.
fn unix_socket(blocking: bool, protocol: c_int) -> Box<SocketPair> {
    Box::new(socket_pair::unix_socket(blocking, protocol))
}

/// `new Pipe(blocking)`.
fn pipe_pair(blocking: bool) -> Box<SocketPair> {
    Box::new(socket_pair::pipe(blocking))
}

/// `delete pair`.
fn pair_delete(mut pair: Box<SocketPair>) {
    socket_pair::drop(&mut pair);
}

/// `pair->ready()`.
fn ready(pair: &SocketPair) -> bool {
    pair.master_end().is_some() && pair.worker_end().is_some()
}

/// `pair->get_socket(master)`.
fn sock(pair: &SocketPair, master: bool) -> Sock {
    let (master_socket, worker_socket) = pair.raw_ends();
    if master { master_socket } else { worker_socket }
}

/// `pair->write(data, len)`.
fn pair_write(pair: &SocketPair, data: &[u8]) -> isize {
    socket_pair::write_from(pair, data).map_or(-1, usize::cast_signed)
}

/// `socket->get_out_buffer_length()`.
unsafe fn out_buffer_length(socket: Sock) -> u32 {
    unsafe { (*socket).fields.output.out_buffer.as_deref() }.map_or(0, |buffer| {
        swoole_rs_ffi::cpp::swr_cpp_buffer_length(buffer)
    })
}

/// A `String str(DATA_SIZE)` holding `DATA_SIZE - 1` random bytes and a NUL.
fn random_data() -> Vec<u8> {
    let mut data = vec![0u8; DATA_SIZE];
    swoole_rs_ffi::random_bytes(&mut data[..DATA_SIZE - 1]);
    data
}

/// `reactor->get_handler(fd_type, event)`, as its address.
fn get_handler(reactor: Running<'_>, fd_type: c_int, event: c_int) -> usize {
    reactor_upstream::reactor_get_handler(reactor, fd_type, swoole_rs_ffi::Events::from_bits(event))
        .map_or(0, |handler| handler as usize)
}

fn add_hook(kind: swoole_rs_ffi::HookType, f: unsafe extern "C" fn(*mut c_void)) {
    swoole_rs_ffi::swoole_add_hook(kind, f, 1);
}

static REACTOR_CREATE_COUNT: AtomicI32 = AtomicI32::new(0);
static REACTOR_DESTROY_COUNT: AtomicI32 = AtomicI32::new(0);

unsafe fn create() {
    unsafe {
        let _ = swoole_rs_ffi::swoole_event_init(0);
        let reactor = tg();
        let view = &*fields(reactor);
        assert_eq!(view.max_event_num, SW_REACTOR_MAXEVENTS);
        assert!(view.ready);
        // `reactor->write`: every reactor writes through the writers `swoole_init` registered.
        assert!(event_api::registered_writers().is_some());
        assert!(view.close.is_some());
        assert!(view.defer_tasks.is_none());
        assert!(reactor_upstream::reactor_default_write_handler(reactor).is_some());
        // coroutine socket reactor, then the system reactor
        let all = [SW_EVENT_READ, SW_EVENT_WRITE, SW_EVENT_ERROR];
        for (fd_type, events) in [
            (SW_FD_CO_SOCKET, &all[..]),
            (SW_FD_CO_POLL, &all[..]),
            (SW_FD_CO_EVENT, &all[..]),
            (SW_FD_AIO, &all[..1]),
        ] {
            for &event in events {
                assert_ne!(get_handler(reactor, fd_type, event), 0);
            }
        }
        let _ = swoole_rs_ffi::swoole_event_free();
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
        assert!(tg_reactor().is_null());
        REACTOR_DESTROY_COUNT.store(0, Relaxed);
        add_hook(swoole_rs_ffi::HookType::ON_REACTOR_DESTROY, count_destroy);

        let mut original_limit = [0u64; 2];
        assert_eq!(getrlimit(RLIMIT_NOFILE, &mut original_limit), 0);
        let limit = [0, original_limit[1]];
        assert_eq!(setrlimit(RLIMIT_NOFILE, &limit), 0);
        let _restore = RestoreLimit(original_limit);

        swoole_rs_ffi::log::set_last_error(0);
        let retval = swoole_rs_ffi::swoole_event_init(0);
        let error = last_error();
        let restore_result = setrlimit(RLIMIT_NOFILE, &original_limit);

        assert_eq!(restore_result, 0);
        assert_eq!(retval, SW_ERR);
        assert_eq!(error, EMFILE);
        assert!(tg_reactor().is_null());
        assert_eq!(REACTOR_DESTROY_COUNT.load(Relaxed), 1);

        assert_eq!(swoole_rs_ffi::swoole_event_init(0), SW_OK);
        assert!(swoole_rs_ffi::swoole_event_free());
        assert_eq!(REACTOR_DESTROY_COUNT.load(Relaxed), 2);
    }
}

unsafe fn set_handler() {
    unsafe {
        let reactor = reactor_new(SW_REACTOR_MAXEVENTS as c_int, TYPE_AUTO);
        for (event, fake) in [
            (SW_EVENT_READ, 1usize),
            (SW_EVENT_WRITE, 2),
            (SW_EVENT_ERROR, 3),
        ] {
            // `(ReactorHandler) 0x1`: an address the case only compares, never calls.
            let fake_handler = std::mem::transmute::<usize, Handler>(fake);
            reactor_upstream::set_handler(
                reactor,
                SW_FD_SESSION,
                Events::from_bits(event),
                Some(fake_handler),
            );
            assert!(reactor_upstream::reactor_isset_handler(
                reactor,
                SW_FD_SESSION,
                event
            ));
            assert_eq!(get_handler(reactor, SW_FD_SESSION, event), fake);
        }
        reactor_delete(reactor);
    }
}

/// wait's and write's handler: one "hello world" datagram, then the socket leaves.
fn read_hello(reactor: Running<'_>, event: &mut Event) -> c_int {
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
        let p = unix_socket(true, SOCK_DGRAM);
        assert!(ready(&p));

        assert_eq!(
            swoole_rs_ffi::swoole_event_init(SW_EVENTLOOP_WAIT_EXIT),
            SW_OK
        );
        assert!(!tg_reactor().is_null());
        assert!((*fields(tg())).ready);

        event_api::set_handler(SW_FD_PIPE, Events::READ, Some(read_hello));

        let ret = event_api::event_add(handle(sock(&p, false)), Events::READ);
        assert_eq!(
            event_api::swoole_event_get_socket((*sock(&p, false)).fields.fd),
            sock(&p, false).cast()
        );
        assert!(ret);

        assert_eq!(pair_write(&p, HELLO), HELLO.len() as isize);

        assert_eq!(event_api::swoole_event_wait(), SW_OK);
        assert!(tg_reactor().is_null());
        pair_delete(p);
    }
}

unsafe fn get_missing_socket() {
    unsafe {
        let p = unix_socket(true, SOCK_DGRAM);
        assert!(ready(&p));
        assert_eq!(swoole_rs_ffi::swoole_event_init(0), SW_OK);

        let socket = sock(&p, false);
        let fd = (*socket).fields.fd;

        expect_eq!(event_api::swoole_event_get_socket(fd), null_mut());
        expect_eq!(!get_socket(tg(), fd).is_null(), false);
        expect_eq!(event_num(tg()), 0);
        expect_eq!(event_api::event_add(handle(socket), Events::READ), true);
        expect_eq!(event_api::swoole_event_get_socket(fd), socket.cast());
        expect_eq!(event_api::event_del(handle(socket)), true);
        expect_eq!(swoole_rs_ffi::swoole_event_free(), true);
        pair_delete(p);
    }
}

unsafe fn write_case() {
    unsafe {
        let mut p = unix_socket(true, SOCK_DGRAM);
        assert!(ready(&p));
        socket_pair::set_blocking(&mut p, false);
        let _ = socket_pair::unix_socket_set_buffer_size(&p, 65536);

        assert_eq!(
            swoole_rs_ffi::swoole_event_init(SW_EVENTLOOP_WAIT_EXIT),
            SW_OK
        );
        assert!(!tg_reactor().is_null());

        event_api::set_handler(SW_FD_PIPE, Events::READ, Some(read_hello));
        assert!(event_api::event_add(handle(sock(&p, false)), Events::READ));

        let n = event_api::event_write(handle(sock(&p, true)), HELLO);
        assert_eq!(n, Some(HELLO.len()));

        assert_eq!(event_api::swoole_event_wait(), SW_OK);
        assert!(tg_reactor().is_null());
        pair_delete(p);
    }
}

unsafe fn wait_timeout() {
    assert_eq!(
        swoole_rs_ffi::swoole_event_init(SW_EVENTLOOP_WAIT_EXIT),
        SW_OK
    );
    assert!(!tg_reactor().is_null());

    tg().set_timeout_msec(30);
    let started_at = microtime();
    assert_eq!(swoole_rs_ffi::reactor_wait(tg()), SW_OK);

    let dr = microtime() - started_at;
    assert!(dr >= 0.03, "{dr}");
    assert!(dr < 0.05, "{dr}");

    let _ = swoole_rs_ffi::swoole_event_free();
}

unsafe fn wait_error() {
    unsafe {
        assert_eq!(
            swoole_rs_ffi::swoole_event_init(SW_EVENTLOOP_WAIT_EXIT),
            SW_OK
        );
        assert!(!tg_reactor().is_null());

        // EINVAL: maxevents is less than or equal to zero.
        (*fields(tg())).max_event_num = 0;
        assert_eq!(swoole_rs_ffi::reactor_wait(tg()), SW_ERR);
        assert_eq!(errno(), EINVAL);

        let _ = swoole_rs_ffi::swoole_event_free();
    }
}

unsafe fn writev() {
    unsafe {
        // `network::Socket fake_sock{}`: a record on upstream's stack, never freed.
        let fake_sock = SocketRecord::new().into_raw();

        let p = unix_socket(true, SOCK_DGRAM);
        assert!(ready(&p));

        (*fake_sock).fields.fd = (*sock(&p, true)).fields.fd;

        let _ = swoole_rs_ffi::swoole_event_init(SW_EVENTLOOP_WAIT_EXIT);

        let iov = [IoSlice::new(b"hello "), IoSlice::new(b"world\n")];
        assert_eq!(
            event_api::swoole_event_writev(handle(fake_sock), &iov),
            Some(12)
        );

        let mut buf = [0u8; 32];
        assert_eq!(sock_read(sock(&p, false), &mut buf), 12);
        assert_eq!(&buf[..12], b"hello world\n");

        // `fake_sock.ssl = (SSL *) -1`: a non-null SSL the write refuses before using.
        let stub = std::mem::transmute::<usize, swoole_rs_ffi::SocketSsl>(usize::MAX);
        (*fake_sock).fields.set_ssl(Some(stub));
        assert_eq!(
            event_api::swoole_event_writev(handle(fake_sock), &iov),
            None
        );
        assert_eq!(
            last_error(),
            error(ErrorCode::SW_ERROR_OPERATION_NOT_SUPPORT)
        );

        let _ = event_api::swoole_event_wait();
        pair_delete(p);
    }
}

/// write_2m's handler: fill the thread buffer until DATA_SIZE bytes arrived.
fn read_2m(reactor: Running<'_>, event: &mut Event) -> c_int {
    // SAFETY: the event's socket and the thread buffer, live during dispatch.
    unsafe {
        let tg_buf = swoole_rs_ffi::thread_buffer().cast::<CppString>();
        let socket = ev_socket(event);
        let n = read(
            (*socket).fields.fd,
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
        let mut p = unix_socket(true, SOCK_STREAM);
        assert!(ready(&p));

        assert_eq!(
            swoole_rs_ffi::swoole_event_init(SW_EVENTLOOP_WAIT_EXIT),
            SW_OK
        );
        assert!(!tg_reactor().is_null());

        event_api::set_handler(SW_FD_PIPE, Events::READ, Some(read_2m));

        socket_pair::set_blocking(&mut p, false);
        let _ = socket_pair::unix_socket_set_buffer_size(&p, 65536);

        assert!(event_api::event_add(handle(sock(&p, false)), Events::READ));

        let data = random_data();

        let tg_buf = swoole_rs_ffi::thread_buffer().cast::<CppString>();
        (*tg_buf).length = 0;
        (*tg_buf).offset = 0;

        let socket = sock(&p, true);
        (*socket).fields.output.buffer_size = 2 * 1024 * 1024;

        let n = event_api::event_write(handle(socket), &data);
        assert_eq!(n, Some(data.len()));
        assert!(out_buffer_length(socket) > 1024);

        println!("{}", out_buffer_length(socket));

        assert_eq!(
            event_api::event_write(handle(socket), &data[..256 * 1024]),
            None
        );
        assert_eq!(
            last_error(),
            error(ErrorCode::SW_ERROR_OUTPUT_BUFFER_OVERFLOW)
        );

        // Upstream's length, past its data's end, which the write refuses unread: here
        // bytes of that length (SW_ERR is `None`).
        let oversize = vec![0; (*socket).fields.output.buffer_size as usize + 8192];
        assert_eq!(event_api::event_write(handle(socket), &oversize), None);
        assert_eq!(
            last_error(),
            error(ErrorCode::SW_ERROR_PACKAGE_LENGTH_TOO_LARGE)
        );

        assert_eq!(event_api::swoole_event_wait(), SW_OK);
        assert!(!event_api::swoole_event_is_available());
        // ASSERT_STREQ: the two as C strings.
        assert_eq!(
            CStr::from_ptr((*swoole_rs_ffi::thread_buffer().cast::<CppString>()).str),
            CStr::from_bytes_until_nul(&data).unwrap()
        );
        pair_delete(p);
    }
}

unsafe fn bad_fd() {
    unsafe {
        let _ = swoole_rs_ffi::swoole_event_init(SW_EVENTLOOP_WAIT_EXIT);
        let socket = swoole_rs_ffi::socket_upstream::make_socket_fd(999999, SW_FD_STREAM_CLIENT);
        (*socket).fields.output.nonblock = true;
        let n = event_api::event_write(handle(socket), b"hello world");
        assert_eq!(n, None);
        assert_eq!(last_error(), EBADF);
        let _ = swoole_rs_ffi::swoole_event_free();
        // `socket->move_fd()`.
        (*socket).fields.fd = -1;
        swoole_rs_ffi::upstream::socket_free(Some(OwnedSocket::from_socket(handle(socket))));
    }
}

fn pipe_read(reactor: Running<'_>, event: &mut Event) -> c_int {
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

fn pipe_write(reactor: Running<'_>, event: &mut Event) -> c_int {
    // SAFETY: the event's socket, live during dispatch.
    unsafe {
        let socket = ev_socket(event);
        expect_eq!(sock_write(socket, PKT), PKT.len() as isize);
        del(reactor, socket);
    }
    SW_OK
}

fn count_write(_: Running<'_>, event: &mut Event) -> c_int {
    // SAFETY: the socket's object is reactor_test_func's write_count.
    unsafe { *(*ev_socket(event)).fields.object.cast::<c_int>() += 1 };
    SW_OK
}

/// Upstream's reactor_test_func.
unsafe fn reactor_test_func(reactor: Running<'_>) {
    unsafe {
        let p = pipe_pair(false);
        assert!(ready(&p));

        reactor_upstream::set_handler(
            reactor,
            SW_FD_PIPE,
            Events::from_bits(SW_EVENT_READ),
            Some(pipe_read),
        );
        reactor_upstream::set_handler(
            reactor,
            SW_FD_PIPE,
            Events::from_bits(SW_EVENT_WRITE),
            Some(pipe_write),
        );

        assert_eq!(add(reactor, sock(&p, false), SW_EVENT_READ), SW_OK);
        assert_eq!(add(reactor, sock(&p, true), SW_EVENT_WRITE), SW_OK);

        let unsock = unix_socket(false, SOCK_STREAM);
        assert!(ready(&unsock));

        let write_count: *mut c_int = Box::into_raw(Box::new(0));
        let sock2 = sock(&unsock, false);
        (*sock2).fields.object = write_count.cast();
        (*sock2).fields.fd_type = SW_FD_STREAM;

        reactor_upstream::set_handler(
            reactor,
            SW_FD_STREAM,
            Events::from_bits(SW_EVENT_WRITE),
            Some(count_write),
        );
        assert_eq!(
            add(
                reactor,
                sock2,
                SW_FD_STREAM | SW_EVENT_WRITE | SW_EVENT_ONCE
            ),
            SW_OK
        );

        assert_eq!(*write_count, 0);
        assert_eq!(swoole_rs_ffi::reactor_wait(reactor), SW_OK);
        assert_eq!(*write_count, 1);

        drop(Box::from_raw(write_count));
        pair_delete(unsock);
        pair_delete(p);
    }
}

unsafe fn local_reactor(kind: c_int, body: impl FnOnce(Running<'static>)) {
    unsafe {
        let reactor = reactor_new(1024, kind);
        reactor.set_wait_exit(true);
        body(reactor);
        reactor_delete(reactor);
    }
}

unsafe fn epoll() {
    unsafe { local_reactor(TYPE_EPOLL, |r| reactor_test_func(r)) }
}

unsafe fn poll() {
    unsafe { local_reactor(TYPE_POLL, |r| reactor_test_func(r)) }
}

struct PollSocketState {
    sockets: [Sock; 2],
    dispatched_socket: Sock,
    replacement_socket: Sock,
    source_fd: c_int,
    replacement_count: c_int,
}

fn reuse_read(reactor: Running<'_>, event: &mut Event) -> c_int {
    // SAFETY: the socket's object is the case's PollSocketState.
    unsafe {
        let socket = ev_socket(event);
        let state = &mut *(*socket).fields.object.cast::<PollSocketState>();
        let mut data = [0u8; 1];
        expect_eq!(sock_read(socket, &mut data), 1);
        state.dispatched_socket = socket;

        let other_socket = if socket == state.sockets[0] {
            state.sockets[1]
        } else {
            state.sockets[0]
        };
        expect_eq!(del(reactor, other_socket), SW_OK);
        // `other_socket->move_fd()`.
        let fd = std::mem::replace(&mut (*other_socket).fields.fd, -1);
        expect_eq!(dup2(state.source_fd, fd), fd);

        state.replacement_socket = swoole_rs_ffi::socket_upstream::make_socket_fd(fd, SW_FD_USER);
        (*state.replacement_socket).fields.object = (&raw mut state.replacement_count).cast();
        expect_eq!(add(reactor, state.replacement_socket, SW_EVENT_READ), SW_OK);
    }
    SW_OK
}

fn count_user(_: Running<'_>, event: &mut Event) -> c_int {
    // SAFETY: the socket's object is the state's replacement_count.
    unsafe { *(*ev_socket(event)).fields.object.cast::<c_int>() += 1 };
    SW_OK
}

unsafe fn poll_readiness_after_fd_reuse() {
    unsafe {
        let sockets = [
            unix_socket(false, SOCK_STREAM),
            unix_socket(false, SOCK_STREAM),
        ];
        assert!(ready(&sockets[0]));
        assert!(ready(&sockets[1]));

        let mut pipe_fds = [0 as c_int; 2];
        assert_eq!(pipe(pipe_fds.as_mut_ptr()), 0);

        let reactor = reactor_new(32, TYPE_POLL);
        (*fields(reactor)).once = true;

        let state = Box::into_raw(Box::new(PollSocketState {
            sockets: [sock(&sockets[0], false), sock(&sockets[1], false)],
            dispatched_socket: null_mut(),
            replacement_socket: null_mut(),
            source_fd: pipe_fds[0],
            replacement_count: 0,
        }));
        for &socket in &(*state).sockets {
            (*socket).fields.fd_type = SW_FD_STREAM;
            (*socket).fields.object = state.cast();
        }

        reactor_upstream::set_handler(
            reactor,
            SW_FD_STREAM,
            Events::from_bits(SW_EVENT_READ),
            Some(reuse_read),
        );
        reactor_upstream::set_handler(
            reactor,
            SW_FD_USER,
            Events::from_bits(SW_EVENT_READ),
            Some(count_user),
        );

        assert_eq!(add(reactor, (*state).sockets[0], SW_EVENT_READ), SW_OK);
        assert_eq!(add(reactor, (*state).sockets[1], SW_EVENT_READ), SW_OK);
        assert_eq!(sock_write(sock(&sockets[0], true), b"x"), 1);
        assert_eq!(sock_write(sock(&sockets[1], true), b"x"), 1);
        assert_eq!(swoole_rs_ffi::reactor_wait(reactor), SW_OK);

        assert_eq!((*state).replacement_count, 0);
        assert!(!(*state).dispatched_socket.is_null());
        assert!(!(*state).replacement_socket.is_null());

        // A local Reactor does not defer Socket::free(), so release sockets after the batch.
        assert_eq!(del(reactor, (*state).dispatched_socket), SW_OK);
        assert_eq!(del(reactor, (*state).replacement_socket), SW_OK);
        swoole_rs_ffi::upstream::socket_free(Some(OwnedSocket::from_socket(handle(
            (*state).replacement_socket,
        ))));
        assert_eq!(close(pipe_fds[0]), 0);
        assert_eq!(close(pipe_fds[1]), 0);

        reactor_delete(reactor);
        drop(Box::from_raw(state));
        let [sockets0, sockets1] = sockets;
        pair_delete(sockets1);
        pair_delete(sockets0);
    }
}

/// `network::Socket s{}; s.fd = fd;`: records on upstream's stack, never freed.
fn fake_socket(fd: c_int) -> Sock {
    let mut socket = SocketRecord::new();
    socket.fields.fd = fd;
    socket.into_raw()
}

unsafe fn poll_extra() {
    unsafe {
        let reactor = reactor_new(32, TYPE_POLL);

        let fake_sock1 = fake_socket(12345);
        let fake_sock2 = fake_socket(99999);

        assert_eq!(add(reactor, fake_sock1, SW_EVENT_READ), SW_OK);
        assert_eq!(add(reactor, fake_sock2, SW_EVENT_READ), SW_OK);

        assert_eq!(add(reactor, fake_sock1, SW_EVENT_READ), SW_ERR);

        assert_eq!(event_num(reactor), 2);

        assert_eq!(
            set(
                reactor,
                fake_sock2,
                SW_EVENT_READ | SW_EVENT_WRITE | SW_EVENT_ERROR
            ),
            SW_OK
        );

        assert_eq!(del(reactor, fake_sock2), SW_OK);
        assert_eq!(event_num(reactor), 1);

        let fake_sock3 = fake_socket(88888);

        assert_eq!(
            set(reactor, fake_sock3, SW_EVENT_READ | SW_EVENT_WRITE),
            SW_ERR
        );
        assert_eq!(last_error(), error(ErrorCode::SW_ERROR_SOCKET_NOT_EXISTS));

        assert_eq!(del(reactor, fake_sock3), SW_ERR);
        assert_eq!(last_error(), error(ErrorCode::SW_ERROR_SOCKET_NOT_EXISTS));

        let fake_socks: Vec<Sock> = (0..32).map(|i| fake_socket(i + 1024)).collect();
        for (i, &socket) in fake_socks.iter().enumerate() {
            assert_eq!(
                add(reactor, socket, SW_EVENT_READ),
                if i <= 30 { SW_OK } else { SW_ERR }
            );
        }
        for (i, &socket) in fake_socks.iter().enumerate().rev() {
            assert_eq!(del(reactor, socket), if i <= 30 { SW_OK } else { SW_ERR });
        }
        reactor_delete(reactor);
    }
}

/// The reactor whose end callback [`end_timed_out`] is (end callbacks take no argument).
static END_REACTOR: AtomicPtr<c_void> = AtomicPtr::new(null_mut());

extern "C-unwind" fn end_timed_out() {
    let reactor = END_REACTOR.load(Relaxed);
    // SAFETY: the reactor running its end callbacks.
    assert!(unsafe { (*ReactorPrepared::of(reactor).fields).timed_out });
}

extern "C" fn on_sigio(_: c_int) {}

/// `reactor->set_end_callback(id, fn)`, or `erase_end_callback(id)` without one.
unsafe fn set_end_callback(reactor: Running<'_>, id: c_int, f: Option<extern "C-unwind" fn()>) {
    // SAFETY: the live reactor's state, which nothing else borrows meanwhile.
    let state = unsafe { &mut *reactor.as_ptr().cast::<ReactorState>() };
    swoole_rs_ffi::swr_v1_reactor_set_end(state, id, f);
}

unsafe fn poll_extra2() {
    unsafe {
        let reactor = reactor_new(32, TYPE_POLL);
        (*fields(reactor)).once = true;
        reactor.set_timeout_msec(10);
        END_REACTOR.store(reactor.as_ptr(), Relaxed);
        set_end_callback(reactor, PRIORITY_DEFER_TASK, Some(end_timed_out));
        assert_eq!(swoole_rs_ffi::reactor_wait(reactor), SW_OK);

        let _ = swoole_rs_ffi::signal::swoole_signal_set_action(
            SIGIO,
            on_sigio as extern "C" fn(c_int) as usize,
            0,
            0,
        );

        set_end_callback(reactor, PRIORITY_DEFER_TASK, None);
        reactor.set_timeout_msec(1000);

        let t = std::thread::spawn(|| {
            swoole_rs_ffi::signal_block_all();
            usleep(10000);
            kill(getpid(), SIGIO);
        });
        *__errno_location() = 0;
        assert_eq!(swoole_rs_ffi::reactor_wait(reactor), SW_OK);
        assert_eq!(errno(), EINTR);

        t.join().unwrap();
        reactor_delete(reactor);
    }
}

unsafe fn add_or_update() {
    unsafe {
        let p = unix_socket(true, SOCK_DGRAM);
        assert!(ready(&p));

        assert_eq!(
            swoole_rs_ffi::swoole_event_init(SW_EVENTLOOP_WAIT_EXIT),
            SW_OK
        );
        assert!(!tg_reactor().is_null());

        let socket = sock(&p, false);
        assert!(event_api::add_or_update(handle(socket), Events::READ));
        assert!((*socket).fields.events.bits() & SW_EVENT_READ != 0);

        assert!(event_api::add_or_update(handle(socket), Events::WRITE));
        assert!((*socket).fields.events.bits() & SW_EVENT_READ != 0);
        assert!((*socket).fields.events.bits() & SW_EVENT_WRITE != 0);

        let _ = swoole_rs_ffi::swoole_event_free();
        pair_delete(p);
    }
}

/// `[&count](void *) { count++; }`.
struct Increment(std::rc::Rc<std::cell::Cell<c_int>>);

impl event_api::Deferred for Increment {
    fn run(self) {
        self.0.set(self.0.get() + 1);
    }

    fn dispose(self) {}
}

unsafe fn defer_task() {
    unsafe {
        let _ = swoole_rs_ffi::swoole_event_init(SW_EVENTLOOP_WAIT_EXIT);
        let reactor = tg();
        assert_eq!((*fields(reactor)).max_event_num, SW_REACTOR_MAXEVENTS);

        let count = std::rc::Rc::new(std::cell::Cell::new(0));
        // `reactor->defer(fn)` on the thread's reactor, which `reactor` is.
        assert!(event_api::swoole_event_defer_value(Increment(count.clone())).is_ok());
        let _ = event_api::swoole_event_wait();
        assert_eq!(count.get(), 1);
        let _ = swoole_rs_ffi::swoole_event_free();
    }
}

static EVENT_LOOP_COUNT: AtomicI32 = AtomicI32::new(0);

extern "C-unwind" fn future_task(data: *mut c_void) {
    // SAFETY: the task's data is the case's C string.
    assert_eq!(unsafe { CStr::from_ptr(data.cast()) }, c"hello world");
    EVENT_LOOP_COUNT.fetch_add(1, Relaxed);
}

/// `reactor->future_task` (`before`) or `idle_task` = `{callback, data}`.
unsafe fn set_task(reactor: Running<'_>, before: bool, callback: swoole_rs_ffi::DeferCall) {
    let task = swoole_rs_ffi::ReactorTask {
        callback: Some(callback),
        data: c"hello world".as_ptr().cast_mut().cast(),
    };
    // SAFETY: the live reactor's fields, which nothing else borrows meanwhile.
    let fields = unsafe { &mut *fields(reactor) };
    if before {
        fields.future_task = task;
    } else {
        fields.idle_task = task;
    }
}

unsafe fn cycle() {
    unsafe {
        local_reactor(TYPE_POLL, |reactor| {
            set_task(reactor, true, future_task);
            reactor_test_func(reactor);
            assert!(EVENT_LOOP_COUNT.load(Relaxed) > 0);
        })
    }
}

extern "C-unwind" fn event_idle_callback(data: *mut c_void) {
    // SAFETY: the task's data is the case's C string.
    assert_eq!(unsafe { CStr::from_ptr(data.cast()) }, c"hello world");
}

unsafe fn priority_idle_task() {
    unsafe {
        local_reactor(TYPE_POLL, |reactor| {
            set_task(reactor, false, event_idle_callback);
            reactor_test_func(reactor);
        })
    }
}

extern "C" fn hook_create(data: *mut c_void) {
    // SAFETY: the hook's data is the Reactor being created.
    assert_eq!(TYPE_POLL, unsafe {
        (*ReactorPrepared::of(data).fields).kind
    });
    REACTOR_CREATE_COUNT.fetch_add(1, Relaxed);
}

extern "C" fn hook_destroy(data: *mut c_void) {
    // SAFETY: the hook's data is the Reactor being destroyed.
    assert_eq!(TYPE_POLL, unsafe {
        (*ReactorPrepared::of(data).fields).kind
    });
    REACTOR_DESTROY_COUNT.fetch_add(1, Relaxed);
}

unsafe fn hook() {
    unsafe {
        REACTOR_CREATE_COUNT.store(0, Relaxed);
        REACTOR_DESTROY_COUNT.store(0, Relaxed);

        add_hook(swoole_rs_ffi::HookType::ON_REACTOR_CREATE, hook_create);
        add_hook(swoole_rs_ffi::HookType::ON_REACTOR_DESTROY, hook_destroy);

        local_reactor(TYPE_POLL, |reactor| reactor_test_func(reactor));

        assert_eq!(REACTOR_CREATE_COUNT.load(Relaxed), 1);
        assert_eq!(REACTOR_DESTROY_COUNT.load(Relaxed), 1);
    }
}

unsafe fn set_fd() {
    unsafe {
        let p = unix_socket(true, SOCK_DGRAM);
        for kind in [TYPE_EPOLL, TYPE_POLL] {
            let reactor = reactor_new(1024, kind);
            assert_eq!(add(reactor, sock(&p, false), SW_EVENT_READ), SW_OK);
            assert_eq!(set(reactor, sock(&p, false), SW_EVENT_WRITE), SW_OK);
            reactor_delete(reactor);
        }
        pair_delete(p);
    }
}

fn error_event_handler(reactor: Running<'_>, event: &mut Event) -> c_int {
    // SAFETY: the reactor's ptr is test_error_event's retval.
    unsafe {
        expect_eq!(del(reactor, ev_socket(event)), SW_OK);
        reactor.set_running(false);
        *reactor.object().cast::<c_int>()
    }
}

unsafe fn test_error_event(kind: c_int, mut retval: c_int) {
    unsafe {
        let mut p = pipe_pair(true);
        assert!(ready(&p));

        let reactor = reactor_new(1024, kind);
        swoole_rs_ffi::set_thread_reactor(reactor.as_ptr());

        reactor.set_object((&raw mut retval).cast());

        reactor_upstream::set_handler(
            reactor,
            SW_FD_PIPE,
            Events::from_bits(SW_EVENT_ERROR),
            Some(error_event_handler),
        );

        add(reactor, sock(&p, true), SW_EVENT_ERROR);
        add(reactor, sock(&p, false), SW_EVENT_ERROR);

        socket_pair::close(&mut p, SW_PIPE_CLOSE_WORKER);
        assert_eq!(swoole_rs_ffi::reactor_wait(reactor), SW_OK);
        reactor_delete(reactor);
        swoole_rs_ffi::set_thread_reactor(null_mut());
        pair_delete(p);
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

unsafe fn error_case() {
    unsafe {
        let p = unix_socket(true, SOCK_DGRAM);
        let worker = sock(&p, false);

        swoole_rs_ffi::swoole_set_print_backtrace_on_error(true);

        let reactor = reactor_new(1024, TYPE_EPOLL);
        assert_eq!(add(reactor, worker, SW_EVENT_READ), SW_OK);
        assert_eq!(add(reactor, worker, SW_EVENT_WRITE), SW_ERR);
        assert_eq!(errno(), EEXIST);

        // `network::Socket bad_sock;`: a record on upstream's stack, never freed.
        let bad_sock = SocketRecord::new().into_raw();
        (*bad_sock).fields.removed = true;
        (*bad_sock).fields.fd_type = SW_FD_PIPE;
        (*bad_sock).fields.fd = dup((*worker).fields.fd);
        assert_eq!(del(reactor, bad_sock), SW_ERR);
        assert_eq!(last_error(), error(ErrorCode::SW_ERROR_EVENT_REMOVE_FAILED));

        assert_eq!(add(reactor, bad_sock, SW_EVENT_READ), SW_OK);
        close((*bad_sock).fields.fd);

        #[expect(clippy::eq_op, reason = "upstream's input, base.cpp line 721")]
        let events = SW_EVENT_READ | SW_EVENT_READ;
        assert_eq!(set(reactor, bad_sock, events), SW_ERR);
        assert_eq!(errno(), EBADF);

        assert_eq!(del(reactor, bad_sock), SW_OK);

        reactor_delete(reactor);

        let reactor = reactor_new(1024, TYPE_POLL);
        assert_eq!(add(reactor, worker, SW_EVENT_READ), SW_OK);
        assert_eq!(del(reactor, worker), SW_OK);
        assert_eq!(del(reactor, worker), SW_ERR);
        assert_eq!(last_error(), error(ErrorCode::SW_ERROR_SOCKET_NOT_EXISTS));
        reactor_delete(reactor);
        pair_delete(p);
    }
}

unsafe fn drain_write_buffer() {
    unsafe {
        let mut p = unix_socket(true, SOCK_STREAM);
        assert!(ready(&p));

        assert_eq!(
            swoole_rs_ffi::swoole_event_init(SW_EVENTLOOP_WAIT_EXIT),
            SW_OK
        );

        socket_pair::set_blocking(&mut p, false);
        let _ = socket_pair::unix_socket_set_buffer_size(&p, 65536);

        let data = random_data();

        let wsock = sock(&p, true);

        let n = event_api::event_write(handle(wsock), &data);
        assert_eq!(n, Some(data.len()));
        assert!(out_buffer_length(wsock) > 1024);

        let rsock = sock(&p, false) as usize;
        std::thread::scope(|scope| {
            let t = scope.spawn(|| {
                usleep(10000);
                let rsock = rsock as Sock;
                let mut rbuf = vec![0u8; DATA_SIZE];
                let mut length = 0;
                loop {
                    let _ = handle(rsock).wait_event(1000, Events::READ);
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

            swoole_rs_ffi::reactor_write::reactor_drain_write_buffer(tg(), handle(wsock));

            assert_eq!(event_api::swoole_event_wait(), SW_OK);
            assert!(!event_api::swoole_event_is_available());
            t.join().unwrap();
        });
        pair_delete(p);
    }
}

fn write_pkt(pair: *mut c_void) {
    // SAFETY: the timer's data is handle_fail's UnixSocket.
    unsafe { sock_write(sock(&*pair.cast::<SocketPair>(), true), PKT) };
}

fn fail_read(reactor: Running<'_>, event: &mut Event) -> c_int {
    let mut buffer = [0u8; 16];
    // SAFETY: the event's socket, live during dispatch.
    unsafe {
        let socket = ev_socket(event);
        let n = sock_read(socket, &mut buffer);
        expect_eq!(n, PKT.len() as isize);
        expect_eq!(&buffer[..n.max(0) as usize], PKT);
        expect_eq!(del(reactor, socket), 0);

        expect_eq!(event_num(reactor), 0);
    }
    SW_ERR
}

fn fail_write(reactor: Running<'_>, event: &mut Event) -> c_int {
    // SAFETY: the socket's object is handle_fail's UnixSocket.
    unsafe {
        let socket = ev_socket(event);
        expect_eq!(set(reactor, socket, SW_EVENT_READ), 0);
        let pair = (*socket).fields.object;
        assert!(swoole_rs_ffi::add_fn(10, false, move || write_pkt(pair)).is_some());
    }
    SW_ERR
}

unsafe fn handle_fail() {
    unsafe {
        let p = unix_socket(true, SOCK_DGRAM);
        assert!(ready(&p));

        assert_eq!(
            swoole_rs_ffi::swoole_event_init(SW_EVENTLOOP_WAIT_EXIT),
            SW_OK
        );
        assert!(!tg_reactor().is_null());

        event_api::set_handler(SW_FD_PIPE, Events::READ, Some(fail_read));
        event_api::set_handler(SW_FD_PIPE, Events::WRITE, Some(fail_write));

        let socket = sock(&p, false);
        (*socket).fields.object = ptr::from_ref::<SocketPair>(&p).cast_mut().cast();

        assert!(event_api::event_add(handle(socket), Events::RDWR));

        assert_eq!(event_api::swoole_event_wait(), SW_OK);
        assert!(tg_reactor().is_null());
        pair_delete(p);
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
    ("error", error_case),
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
