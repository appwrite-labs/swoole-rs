//! The operations against a recording double of the C++ client.
use super::*;
use std::cell::RefCell;
use std::io::Write;
use std::os::fd::{AsRawFd, IntoRawFd};
use swoole_rs_corebase::reactor::{SocketFields, SocketRecord};

#[derive(Default)]
struct Mock {
    log: Vec<String>,
    connect: Vec<bool>,
    connect_async: Vec<Connect>,
    io: Vec<isize>,
    replies: Vec<Vec<u8>>,
    sent: Vec<Vec<u8>>,
    ssl_connect_ready: bool,
    socket_error: c_int,
    /// The callback that releases the client's socket object (as a PHP close would).
    releases: Option<ClientEvent>,
    /// The record `make_socket` allocated; the client releases it.
    socket: Option<core::ptr::NonNull<SocketRecord>>,
    fail_socket: bool,
    /// The last lookup `async_dispatch` took.
    lookup: Option<Lookup>,
    /// The records `free_socket` released, which tests still read: freed with the test.
    freed: Freed,
}

#[derive(Default)]
struct Freed(Vec<swoole_rs_corebase::reactor::OwnedSocket>);

impl Drop for Freed {
    fn drop(&mut self) {
        // `make_socket`'s records, which the client released once.
        self.0
            .drain(..)
            .for_each(swoole_rs_corebase::reactor::OwnedSocket::free);
    }
}

thread_local! {
    static MOCK: RefCell<Mock> = RefCell::new(Mock::default());
}

fn mock<T>(f: impl FnOnce(&mut Mock) -> T) -> T {
    MOCK.with(|m| f(&mut m.borrow_mut()))
}

pub(super) fn log(entry: impl Into<String>) {
    mock(|m| m.log.push(entry.into()));
}

fn taken() -> Vec<String> {
    mock(|m| std::mem::take(&mut m.log))
}

/// The socket record operations' recording doubles.
pub(super) mod ops {
    use super::*;

    pub(in super::super) fn make_socket(
        kind: c_int,
        fd_type: c_int,
        flags: c_int,
    ) -> Option<swoole_rs_corebase::reactor::OwnedSocket> {
        log(format!("make {kind} {fd_type} {flags}"));
        if mock(|m| m.fail_socket) {
            set_errno(libc::EMFILE);
            return None;
        }
        let mut record = SocketRecord::new();
        record.socket_type = kind;
        record.fd = -1;
        let socket = swoole_rs_corebase::reactor::OwnedSocket::new(Box::new(record));
        mock(|m| m.socket = core::ptr::NonNull::new(socket.socket().as_ptr()));
        Some(socket)
    }

    pub(in super::super) fn kernel_timeout(_: Handle<'_>, timeout: f64) {
        log(format!("kernel {timeout}"));
    }
    pub(in super::super) fn assign(_: Handle<'_>, host: &CStr, port: c_int, resolve: bool) -> bool {
        let host = host.to_string_lossy().into_owned();
        log(format!("assign {host}:{port} {resolve}"));
        if host == "dns.test" {
            log::set_last_error(ERROR_BAD_HOST_ADDR);
            return false;
        }
        true
    }
    pub(in super::super) fn connect(_: Handle<'_>, mode: ConnectMode) -> bool {
        log(format!("connect {mode:?}"));
        let connected = mock(|m| m.connect.pop()).unwrap_or(true);
        if !connected {
            set_errno(libc::EINPROGRESS);
        }
        connected
    }
    pub(in super::super) fn connect_async(_: Handle<'_>) -> Connect {
        log("connect Async");
        mock(|m| m.connect_async.pop()).unwrap_or(Connect::Error)
    }
    pub(in super::super) fn event(
        _: Handle<'_>,
        op: EventOp,
        events: swoole_rs_corebase::reactor::Events,
    ) -> bool {
        log(format!("event {op:?} {events}"));
        true
    }
    pub(in super::super) fn socket_error(_: Handle<'_>, error: &mut c_int) -> bool {
        *error = mock(|m| m.socket_error);
        true
    }
}

/// The last socket `make_socket` made.
fn sock() -> &'static mut SocketFields {
    // SAFETY: the test's live record.
    unsafe { mock(|m| m.socket).unwrap().as_mut() }
}

/// `f` over `sw_tg_buffer()`: a 64-byte String per test thread.
pub(super) fn with_thread_buffer(f: &mut dyn FnMut(&mut StringBuffer)) {
    /// Freed with the test's thread.
    struct Shared(StringBuffer);
    impl Drop for Shared {
        fn drop(&mut self) {
            self.0.dispose();
        }
    }
    thread_local! {
        static SHARED: std::cell::RefCell<Shared> = {
            let mut buffer = StringBuffer::EMPTY;
            buffer.alloc(64, None);
            std::cell::RefCell::new(Shared(buffer))
        };
    }
    SHARED.with_borrow_mut(|shared| f(&mut shared.0));
}

// `corebase::tls`'s socket operations.
fn free_socket(socket: Option<swoole_rs_corebase::reactor::OwnedSocket>) {
    log("free");
    mock(|m| m.freed.0.extend(socket));
}
/// `corebase::tls`'s slice-taking transfers.
pub(super) mod net {
    use super::{SocketAddress, io, log, mock};

    fn count(n: isize) -> Option<usize> {
        usize::try_from(n).ok()
    }
    use core::ffi::c_int;
    use swoole_rs_corebase::lent::Spare;
    use swoole_rs_corebase::reactor::Socket;

    pub(in super::super) fn recv_append(
        _: Socket<'_>,
        buffer: &mut swoole_rs_corebase::buffer::string::StringBuffer,
    ) -> Option<usize> {
        if let Some(reply) = mock(|m| m.replies.pop()) {
            log(format!("recv {}", buffer.spare().len()));
            buffer.append(&reply);
            return Some(reply.len());
        }
        let n = count(io("recv", buffer.spare().len()));
        // Zeros, as many as the read reports.
        buffer.fill_within(n.unwrap_or(0), |spare| {
            _ = spare.put(&vec![0; spare.room()]);
        });
        n
    }
    pub(in super::super) fn recv_sync(
        _: Socket<'_>,
        spare: &mut Spare<'_>,
        _: c_int,
    ) -> Option<usize> {
        receive("recv_sync", spare)
    }
    pub(in super::super) fn send_sync(_: Socket<'_>, data: &[u8], _: c_int) -> Option<usize> {
        mock(|m| m.sent.push(data.to_vec()));
        count(io("send_sync", data.len()))
    }
    pub(in super::super) fn recvfrom_sync(
        _: Socket<'_>,
        spare: &mut Spare<'_>,
        _: c_int,
        _: &mut SocketAddress,
    ) -> Option<usize> {
        receive("recvfrom_sync", spare)
    }

    /// The next reply, what fits of it; else zeros, as many as the read reports.
    fn receive(name: &str, spare: &mut Spare<'_>) -> Option<usize> {
        let room = spare.room();
        if let Some(reply) = mock(|m| m.replies.pop()) {
            log(format!("{name} {room}"));
            let fits = reply.get(..room).unwrap_or(&reply);
            return spare.put(fits).map(|copied| copied.len());
        }
        let n = count(io(name, room));
        _ = spare.put(&vec![0; n.unwrap_or(0).min(room)]);
        n
    }
}
pub(super) fn sendfile_sync(_: Socket<'_>, _: &CStr, _: i64, _: usize) -> bool {
    log("sendfile false");
    true
}
pub(super) fn socket_sendfile_async(_: Socket<'_>, _: &CStr, _: i64, _: usize) -> bool {
    log("sendfile true");
    true
}
/// `corebase::reactor`'s, which reads the thread's reactor.
pub(super) fn event_write(_: swoole_rs_corebase::reactor::Heap<'_>, data: &[u8]) -> Option<usize> {
    let length = data.len();
    mock(|m| m.sent.push(data.to_vec()));
    log(format!("write {length}"));
    (log::last_error() != ERROR_OUTPUT_BUFFER_OVERFLOW).then_some(length)
}
/// `corebase::tls`'s `Reactor::_writable_callback`.
pub(super) fn writable_callback(
    _: swoole_rs_corebase::reactor::Running<'_>,
    _: &mut swoole_rs_corebase::reactor::Event,
) -> bool {
    log("writable");
    true
}
/// A socket IO double: logs, then returns the queued result or the whole length.
fn io(name: &str, length: usize) -> isize {
    log(format!("{name} {length}"));
    mock(|m| m.io.pop()).unwrap_or(length.cast_signed())
}
fn listener(notification: Notification) {
    let event = notification.kind();
    let length = match notification {
        Notification::Receive(bytes) => bytes.len(),
        Notification::Error(code) => code as usize,
        _ => 0,
    };
    log(format!("call {event:?} {length}"));
    if mock(|m| m.releases) == Some(event) {
        sock().object = core::ptr::null_mut();
    }
}
/// The connect timer: logs, adds nothing.
#[expect(clippy::unnecessary_wraps, reason = "`corebase::timer`'s signature")]
pub(super) fn timer_add(timeout: f64, _: Handle<'_>) -> Option<swoole_rs_corebase::timer::Held> {
    log(format!("timer {timeout}"));
    Some(swoole_rs_corebase::timer::Held::detached(1))
}
/// `corebase::timer`'s `del_id`: logs, deletes nothing.
pub(super) fn del_id(_: swoole_rs_corebase::timer::Held) {
    log("timer del");
}
/// The async pool submission: logs the request; nothing completes it.
fn async_dispatch(lookup: Lookup, request: &crate::resolver::Request) -> bool {
    mock(|m| m.lookup = Some(lookup));
    let request = request.get();
    let family = request.fields().family;
    log(format!(
        "dns {} {family}",
        String::from_utf8_lossy(&request.name())
    ));
    true
}
// `corebase::tls`'s TLS operations, over its own context.
fn tls_freeze(owner: &TlsContext) -> Option<TlsSettings> {
    log("ssl create");
    super::tls_freeze(owner)
}
pub(super) fn tls_session(_: &TlsSettings, _: Socket<'_>) -> bool {
    log(format!("ssl session {SSL_CLIENT}"));
    true
}
pub(super) fn socket_ssl_connect(_: Socket<'_>) -> bool {
    log("ssl connect");
    if mock(|m| m.ssl_connect_ready) {
        sock().tls.state = SSL_STATE_READY;
    }
    true
}
pub(super) fn socket_ssl_verify(_: Socket<'_>, allow_self_signed: bool) -> bool {
    log(format!("ssl verify {allow_self_signed}"));
    true
}
pub(super) fn socket_ssl_check_host(_: Socket<'_>, _: &CStr) -> bool {
    log("ssl host");
    true
}
fn socket_ssl_close(_: Socket<'_>) {
    log("ssl close");
}
/// `SSL_set_tlsext_host_name`: logged as the `SSL_ctrl` command it is.
pub(super) fn socket_ssl_set_host_name(_: Socket<'_>, _: &CStr) -> bool {
    log("ssl ctrl 55");
    true
}
/// Gives the client a proxy at proxy.test:1080.
fn set_proxy(client: *mut Client, http: bool) {
    super::set_proxy(unsafe { &*client }, http, b"proxy.test", 1080, b"", b"");
}

fn new_client(kind: c_int, is_async: bool) -> *mut Client {
    const OPS: SocketOps = SocketOps {
        dispatch_dns: async_dispatch,
        with_buffer: with_thread_buffer,
        recv_append: net::recv_append,
        recv_sync: net::recv_sync,
        recvfrom_sync: net::recvfrom_sync,
        send_sync: net::send_sync,
        sendfile_sync,
        sendfile_async: socket_sendfile_async,
        free_socket,
        close_tls: socket_ssl_close,
        write: event_write,
        writable: writable_callback,
        make_socket: ops::make_socket,
        kernel_timeout: ops::kernel_timeout,
        assign: ops::assign,
        connect_async: ops::connect_async,
        connect: ops::connect,
        event: ops::event,
        socket_error: ops::socket_error,
        timer_add,
        timer_delete: del_id,
        tls_freeze,
        tls_session,
        tls_connect: socket_ssl_connect,
        tls_set_host_name: socket_ssl_set_host_name,
        tls_verify: socket_ssl_verify,
        tls_check_host: socket_ssl_check_host,
    };
    // The test frees it through [`free`].
    create_with(kind, is_async, &OPS).map_or(core::ptr::null_mut(), |owner| {
        owner.keep();
        owner.client
    })
}

/// A client of `kind`, its construction log dropped, listened to for every event
/// with `callbacks`.
fn client(kind: c_int, is_async: bool, callbacks: bool) -> *mut Client {
    mock(|m| *m = Mock::default());
    log::set_last_error(0);
    let client = new_client(kind, is_async);
    if callbacks {
        // SAFETY: a live state.
        listen(
            unsafe { &*client },
            0b111_1110,
            Listener::new(TrustedAllocator::STD, listener),
        );
    }
    taken();
    client
}

/// The owner [`new_client`] kept the client from, dropped: the test's live client,
/// not used afterwards.
fn free(client: *mut Client) {
    drop(Owner {
        client,
        // SAFETY: the reference `keep` leaked in `new_client`.
        own: Cell::new(Some(unsafe { Rc::from_raw(client) })),
    });
}

/// `Client_onWrite` with a reactor and event the mocks ignore.
fn writable(client: *mut Client) -> bool {
    let mut event = swoole_rs_corebase::reactor::Event::of(0, 0, 0, None);
    // SAFETY: the mocked effects never touch the reactor.
    let reactor = unsafe { Running::new(core::ptr::null_mut()) };
    on_write(Handle::raw(client), reactor, &mut event)
}

fn flag(client: *mut Client, name: Flag) -> bool {
    // SAFETY: a live state.
    unsafe { &*client }.flag(name)
}

const OK: c_int = swoole_rs_corebase::constants::ResultCode::SW_OK as c_int;
const ERR: c_int = swoole_rs_corebase::constants::ResultCode::SW_ERR as c_int;

fn connect(client: *mut Client, host: &CStr, timeout: f64, flags: c_int) -> c_int {
    if super::connect(Handle::raw(client), host, 80, timeout, flags) {
        OK
    } else {
        ERR
    }
}

#[test]
fn construction_makes_a_cloexec_socket_nonblocking_when_async() {
    mock(|m| *m = Mock::default());
    log::set_last_error(0);
    let client = new_client(1, true);
    assert_eq!(taken(), ["make 1 17 12"]);
    assert!(flag(client, Flag::Async) && flag(client, Flag::Stream));
    assert_eq!(sock().chunk_size, SEND_BUFFER_SIZE);
    free(client);
    assert_eq!(taken(), ["event Del 0", "free"]);
    let client = new_client(2, false);
    assert_eq!(taken(), ["make 2 18 8"]);
    free(client);
}

#[test]
fn a_call_measures_with_the_protocol_and_frees_the_client() {
    #[expect(clippy::unnecessary_wraps, reason = "a `GetPackageLength`")]
    fn twice(
        _: &Protocol,
        _: Option<u8>,
        _: Option<std::rc::Rc<dyn std::any::Any>>,
        packet: &mut PacketLength,
    ) -> Option<usize> {
        Some(2 * packet.bytes().len())
    }
    let client = client(1, true, false);
    Handle::raw(client)
        .get()
        .edit_record(|record| record.protocol.get_package_length = Some(twice));
    assert_eq!(package_length(Handle::raw(client), b"abc"), Some(6));
    free(client);
    assert_eq!(taken(), ["event Del 0", "free"]);
}

#[test]
fn async_close_detaches_then_runs_on_close_once() {
    let client = client(1, true, true);
    Handle::raw(client).set(Flag::Active, true);
    assert!(!close(Handle::raw(client)), "close(-1) fails");
    assert_eq!(taken(), ["event Del 0", "call Close 0"]);
    assert!(flag(client, Flag::Closed) && !flag(client, Flag::Active));
    assert!(!close(Handle::raw(client)));
    assert_eq!(taken(), Vec::<String>::new());
    free(client);
}

#[test]
fn close_returns_zero_when_on_close_released_the_client() {
    let client = client(1, true, true);
    Handle::raw(client).set(Flag::Active, true);
    mock(|m| m.releases = Some(ClientEvent::Close));
    assert!(close(Handle::raw(client)));
    assert_eq!(sock().fd, -1);
    free(client);
}

#[test]
fn shutdown_records_each_direction_once() {
    let client = client(1, false, false);
    let shutdown = |how| shutdown(Handle::raw(client), how);
    assert!(!shutdown(99));
    assert_eq!(log::last_error(), libc::EINVAL);
    // The fd is -1, so the system call fails.
    assert!(!shutdown(libc::SHUT_RD));
    assert!(!flag(client, Flag::ShutdownRead));
    Handle::raw(client).set(Flag::ShutdownWrite, true);
    assert!(!shutdown(libc::SHUT_WR));
    Handle::raw(client).set(Flag::Closed, true);
    assert!(!shutdown(libc::SHUT_RDWR));
    free(client);
}

#[test]
fn async_connect_needs_every_callback() {
    let client = client(1, true, false);
    assert_eq!(connect(client, c"example.test", 1.0, 0), ERR);
    assert_eq!(taken(), Vec::<String>::new());
    free(client);
}

#[test]
fn async_connect_in_progress_adds_a_timer_and_write_interest() {
    let client = client(1, true, true);
    mock(|m| m.connect = vec![false]);
    let result = connect(client, c"example.test", 2.5, 0);
    assert_eq!(result, OK);
    assert_eq!(
        taken(),
        [
            "assign example.test:80 false",
            "connect Raw",
            "timer 2.5",
            "event Add 1024"
        ]
    );
    free(client);
    assert_eq!(
        taken(),
        ["event Del 0", "timer del", "free"],
        "inactive: no onClose"
    );
}

#[test]
fn async_connect_waits_for_dns_then_connects_the_resolved_address() {
    let client = client(1, true, true);
    assert_eq!(connect(client, c"dns.test", 1.0, 0), OK);
    assert!(flag(client, Flag::WaitDns));
    assert_eq!(taken(), ["assign dns.test:80 false", "dns dns.test 2"]);
    resolved(Handle::raw(client), 0, c"192.0.2.1");
    assert!(!flag(client, Flag::WaitDns));
    assert_eq!(
        taken(),
        ["assign 192.0.2.1:80 false", "connect Raw", "event Add 1024"]
    );
    free(client);
}

#[test]
fn a_lookup_completes_its_awaiting_client_once() {
    let client = client(1, true, true);
    connect(client, c"dns.test", 1.0, 0);
    taken();
    let lookup = mock(|m| m.lookup).unwrap();
    let request = crate::resolver::Request::new(2, 0, 0, b"dns.test", b"");
    resolve_completed(lookup, request.get(), 1);
    assert!(!flag(client, Flag::WaitDns) && flag(client, Flag::Closed));
    assert_eq!(
        taken(),
        [format!("call Error {ERROR_DNSLOOKUP_RESOLVE_FAILED}")]
    );
    resolve_completed(lookup, request.get(), 1);
    assert_eq!(
        taken(),
        Vec::<String>::new(),
        "a completed lookup reaches no client"
    );
    free(client);
}

#[test]
fn a_freed_client_misses_its_lookup() {
    let client = client(1, true, true);
    connect(client, c"dns.test", 1.0, 0);
    let lookup = mock(|m| m.lookup).unwrap();
    free(client);
    taken();
    let request = crate::resolver::Request::new(2, 0, 0, b"dns.test", b"");
    resolve_completed(lookup, request.get(), 1);
    assert_eq!(taken(), Vec::<String>::new());
}

#[test]
fn failed_resolution_closes_and_reports_once() {
    let client = client(1, true, true);
    connect(client, c"dns.test", 1.0, 0);
    taken();
    resolved(Handle::raw(client), 1, c"");
    assert_eq!(log::last_error(), ERROR_DNSLOOKUP_RESOLVE_FAILED);
    assert!(flag(client, Flag::OnerrorCalled) && flag(client, Flag::Closed));
    assert_eq!(
        taken(),
        [format!("call Error {ERROR_DNSLOOKUP_RESOLVE_FAILED}")],
        "removed: no reactor delete; the error is the length"
    );
    free(client);
}

#[test]
fn sync_nonblocking_connect_reports_ready_and_wait() {
    let client = client(1, false, false);
    mock(|m| m.connect_async = vec![Connect::Ready]);
    assert_eq!(connect(client, c"example.test", -1.0, 1), OK);
    mock(|m| m.connect_async = vec![Connect::Wait]);
    assert_eq!(connect(client, c"example.test", -1.0, 1), ERR);
    assert!(flag(client, Flag::AsyncConnect));
    free(client);
}

#[test]
fn sync_connect_through_socks5_routes_and_handshakes() {
    let client = client(1, false, false);
    set_proxy(client, false);
    assert_eq!(
        Handle::raw(client).proxy_state(Proxy::Socks5),
        Some(SOCKS5_WAIT)
    );
    mock(|m| m.replies = vec![vec![5, 0], vec![5, 0]]);
    assert_eq!(connect(client, c"example.test", 3.0, 0), OK);
    assert_eq!(
        taken(),
        [
            "kernel 3",
            "assign proxy.test:1080 true",
            "connect Sync",
            "send_sync 3",
            "recv_sync 64",
            "send_sync 19",
            "recv_sync 64",
        ]
    );
    assert_eq!(
        mock(|m| m.sent.clone()),
        [
            vec![5, 1, 0],
            b"\x05\x01\x00\x03\x0cexample.test\x00\x50".to_vec()
        ]
    );
    assert!(flag(client, Flag::Active));
    assert_eq!(
        Handle::raw(client).proxy_state(Proxy::Socks5),
        Some(SOCKS5_READY)
    );
    let (target, target_port) = Handle::raw(client)
        .with_proxy(Proxy::Socks5, |proxy| {
            (proxy.string(3), proxy.fields().target_port)
        })
        .unwrap();
    assert_eq!(target, b"example.test");
    assert_eq!(target_port, 80);
    let host = Handle::raw(client).get().server_host.take();
    assert_eq!(
        host.as_deref().map(CStr::to_bytes),
        Some(&b"proxy.test"[..])
    );
    free(client);
}

#[test]
fn async_send_raises_buffer_full_at_the_high_watermark() {
    let client = client(1, true, true);
    // Ten bytes queued on the socket.
    let queue = swoole_rs_corebase::buffer::cpp::new_buffer(1024);
    queue.append(&[0_u8; 10]);
    sock().output.out_buffer = Some(queue);
    // SAFETY: a live state.
    unsafe { &*client }.set_watermark(true, 8);
    assert_eq!(send(Handle::raw(client), b"data", 0), Some(4));
    assert_eq!(taken(), ["write 4", "call BufferFull 0"]);
    // Already high: no second call.
    _ = send(Handle::raw(client), b"data", 0);
    assert_eq!(taken(), ["write 4"]);
    log::set_last_error(ERROR_OUTPUT_BUFFER_OVERFLOW);
    assert_eq!(send(Handle::raw(client), b"data", 0), None);
    free(client);
}

#[test]
fn writable_after_connect_starts_the_socks5_handshake() {
    let client = client(1, true, true);
    set_proxy(client, false);
    // SAFETY: a live state.
    unsafe { &*client }.set_watermark(true, 100);
    assert!(writable(client));
    assert_eq!(taken(), ["event Set 512", "write 3"]);
    assert_eq!(mock(|m| m.sent.clone()), [vec![5, 1, 0]]);
    assert_eq!(
        Handle::raw(client).proxy_state(Proxy::Socks5),
        Some(SOCKS5_HANDSHAKE)
    );
    assert!(flag(client, Flag::Active));
    free(client);
}

#[test]
fn stream_read_hands_the_socks5_handshake_the_bytes_received() {
    let client = client(1, true, true);
    set_proxy(client, false);
    assert!(inet_addr(Handle::raw(client), c"example.test", 80));
    assert!(writable(client));
    Handle::raw(client).input_init();
    taken();
    mock(|m| m.replies = vec![vec![5, 0], vec![5, 0]]);
    assert!(on_read(Handle::raw(client)));
    assert_eq!(taken(), ["recv 65536", "write 19"]);
    assert_eq!(Handle::raw(client).proxy_state(Proxy::Socks5), Some(3));
    assert!(on_read(Handle::raw(client)));
    assert_eq!(taken(), ["recv 65534", "call Connect 0"]);
    assert_eq!(
        mock(|m| m.sent.clone()),
        [
            vec![5, 1, 0],
            b"\x05\x01\x00\x03\x0cexample.test\x00\x50".to_vec()
        ]
    );
    assert_eq!(
        Handle::raw(client).proxy_state(Proxy::Socks5),
        Some(SOCKS5_READY)
    );
    free(client);
}

#[test]
fn writable_after_a_failed_connect_reports_the_error() {
    let client = client(1, true, true);
    mock(|m| m.socket_error = libc::ECONNREFUSED);
    writable(client);
    assert_eq!(log::last_error(), libc::ECONNREFUSED);
    assert_eq!(
        taken(),
        [
            "event Del 0".to_owned(),
            format!("call Error {}", libc::ECONNREFUSED)
        ]
    );
    free(client);
}

#[test]
#[cfg_attr(miri, ignore = "Miri has no OPENSSL_init_ssl")]
fn tls_connect_waits_for_the_stream_then_connects() {
    let client = client(1, true, true);
    assert!(enable_ssl(Handle::raw(client)).is_some());
    taken();
    writable(client);
    assert_eq!(sock().tls.state, SSL_STATE_WAIT_STREAM);
    taken();
    mock(|m| m.ssl_connect_ready = true);
    writable(client);
    assert_eq!(taken(), ["ssl session 2", "ssl connect", "call Connect 0"]);
    free(client);
}

#[test]
fn connect_timeout_with_a_pending_proxy_fails_without_on_close() {
    let client = client(1, true, true);
    Handle::raw(client).set(Flag::Active, true);
    set_proxy(client, true);
    assert_eq!(
        Handle::raw(client).proxy_state(Proxy::Http),
        Some(HTTP_PROXY_WAIT)
    );
    on_timeout(Handle::raw(client));
    assert_eq!(log::last_error(), libc::ETIMEDOUT);
    assert_eq!(
        taken(),
        [
            "event Del 0".to_owned(),
            format!("call Error {}", libc::ETIMEDOUT)
        ]
    );
    free(client);
}

#[test]
fn receive_payload_outlives_the_client_input() {
    let client = client(1, true, true);
    let handle = Handle::raw(client);
    handle.input_init();
    let listener = Listener::new(TrustedAllocator::STD, move |event| {
        let Notification::Receive(payload) = event else {
            unreachable!()
        };
        Handle::raw(client).input_free();
        assert_eq!(payload.view(), b"a\0b");
        log("received");
    });
    listen(handle.get(), ClientEvent::Receive.bit(), listener);
    mock(|m| m.replies.push(b"a\0b".to_vec()));
    assert!(on_read(handle));
    assert_eq!(taken(), ["recv 65536", "received"]);
    free(client);
}

#[test]
fn stream_read_delivers_data_and_closes_at_eof() {
    let client = client(1, true, true);
    Handle::raw(client).set(Flag::Active, true);
    Handle::raw(client).input_init();
    mock(|m| m.io = vec![5]);
    assert!(on_read(Handle::raw(client)));
    assert_eq!(taken(), ["recv 65536", "call Receive 5"]);
    mock(|m| m.io = vec![0]);
    on_read(Handle::raw(client));
    assert_eq!(taken(), ["recv 65536", "event Del 0", "call Close 0"]);
    free(client);
}

#[test]
#[cfg_attr(miri, ignore = "Miri has no sendto")]
fn datagram_sends_fail_with_sendto_and_dtls_sends_as_a_stream() {
    let client = client(2, false, false);
    // The record's descriptor is -1, so sendto(2) fails.
    assert_eq!(send(Handle::raw(client), b"data", 0), None);
    assert_eq!(errno(), libc::EBADF);
    assert!(enable_ssl(Handle::raw(client)).is_some());
    assert_eq!(sock().chunk_size, SSL_BUFFER_SIZE);
    taken();
    assert_eq!(send(Handle::raw(client), b"data", 0), Some(4));
    assert_eq!(taken(), ["send_sync 4"]);
    free(client);
}

#[test]
fn public_record_defaults_and_direct_framing_writes_drive_reads() {
    let client = client(1, true, true);
    let fields = Handle::raw(client).get().framing.record.as_ptr();
    unsafe {
        assert!(!(*fields).http2 && !(*fields).open_eof_check && !(*fields).open_length_check);
        assert_eq!((*fields).remote_addr.kind, 1);
        assert_eq!((*fields).server_addr.len, 0);
        assert_eq!((*fields).protocol.package_length_type, b'N');
        assert_eq!((*fields).protocol.package_length_size, 4);
        assert_eq!((*fields).protocol.package_max_length, 2 * 1024 * 1024);
        // Miri gives each use of a fn item its own address, as codegen may.
        if !cfg!(miri) {
            assert_eq!(
                (*fields).protocol.on_package.map(|f| f as *const ()),
                Some(on_package as *const ())
            );
        }
        // The client owns the record make_socket made.
        let made = mock(|m| m.socket).map(core::ptr::NonNull::as_ptr);
        assert_eq!(Some(Handle::raw(client).socket().as_ptr()), made);
        assert!((*fields).object.is_null() && (*fields).buffer.is_null());
        Handle::raw(client).input_init();
        let (mut peer, reader) = std::os::unix::net::UnixStream::pair().unwrap();
        reader.set_nonblocking(true).unwrap();
        sock().fd = reader.as_raw_fd();
        (*fields).protocol.get_package_length = Some(crate::framing_recv::default_length_func);
        (*fields).open_length_check = true;
        peer.write_all(&4u32.to_be_bytes()).unwrap();
        assert!(on_read(Handle::raw(client)));
        assert!(taken().contains(&"call Receive 4".to_owned()));
        (*fields).open_eof_check = true;
        (*fields).protocol.package_eof[0] = b'\n';
        (*fields).protocol.package_eof_len = 1;
        peer.write_all(b"ok\n").unwrap();
        assert!(on_read(Handle::raw(client)));
        assert!(taken().contains(&"call Receive 3".to_owned()));
        sock().fd = -1;
    }
    free(client);
}

#[test]
fn failed_framed_read_closes_the_live_client() {
    let client = client(1, true, true);
    let handle = Handle::raw(client);
    handle.input_init();
    handle
        .client()
        .edit_record(|record| record.open_length_check = true);
    let (peer, reader) = std::os::unix::net::UnixStream::pair().unwrap();
    sock().fd = reader.into_raw_fd();
    peer.shutdown(std::net::Shutdown::Write).unwrap();
    assert!(!handle.flag(Flag::Closed));
    assert!(on_read(handle));
    assert!(handle.flag(Flag::Closed));
    free(client);
}

#[test]
fn empty_datagram_is_delivered_and_read_failure_is_silent() {
    let client = client(2, true, true);
    let handle = Handle::raw(client);
    mock(|m| m.io = vec![0]);
    assert!(on_datagram(handle));
    let read = format!("recvfrom_sync {BUFFER_SIZE_UDP}");
    assert_eq!(taken(), [&read, "call Receive 0"]);
    mock(|m| m.io = vec![-1]);
    assert!(!on_datagram(handle));
    assert_eq!(taken(), [read]);
    free(client);
}

#[test]
fn datagram_peer_is_a_snapshot() {
    let client = client(2, false, false);
    let fields = Handle::raw(client).get().framing.record.as_ptr();
    unsafe {
        (*fields).remote_addr.len = 17;
        (*fields).remote_addr.kind = 4;
    }
    let mut result = SocketAddress::default();
    assert!(Handle::raw(client).peer_name(&mut result));
    unsafe { (*fields).remote_addr.len = 29 };
    assert_eq!(result.len, 17);
    assert_eq!(result.kind, 4);
    free(client);
}

#[test]
fn a_failed_socket_makes_no_client_and_keeps_its_errno() {
    mock(|m| {
        *m = Mock {
            fail_socket: true,
            ..Mock::default()
        }
    });
    assert!(new_client(1, false).is_null());
    assert_eq!(errno(), libc::EMFILE);
    assert_eq!(taken(), ["make 1 17 8"]);
}

#[test]
fn framed_package_ranges_keep_the_address_and_clip_the_writable_bytes() {
    use swoole_rs_corebase::buffer::string::{StringBuffer, boxed_string};
    let mut empty = StringBuffer::EMPTY;
    let mut package = Package::of(&mut empty, 0, 12);
    assert!(package.as_ptr().is_null());
    assert_eq!(package.bytes(), []);

    let mut data = boxed_string(b"data");
    let base = data.as_ptr();
    for start in [4, 5] {
        let mut package = Package::of(&mut data, start, usize::MAX);
        assert_eq!(package.as_ptr(), base.wrapping_add(start));
        assert_eq!(package.bytes(), []);
    }
    let mut tail = Package::of(&mut data, 2, usize::MAX);
    assert_eq!(tail.as_ptr(), base.wrapping_add(2));
    tail.bytes().copy_from_slice(b"te");
    assert_eq!(data.view(), b"date");
    data.dispose();
}

#[test]
fn framed_packages_reach_on_receive_until_the_socket_waits_to_close() {
    let client = client(1, true, true);
    let socket = Handle::raw(client).socket();
    let mut data = swoole_rs_corebase::buffer::string::boxed_string(b"data");
    let package = Package::of(&mut data, 0, 4);
    assert!(on_package(core::ptr::null(), socket, package));
    assert_eq!(taken(), ["call Receive 4"]);
    sock().output.close_wait = true;
    let package = Package::of(&mut data, 0, 4);
    assert!(!on_package(core::ptr::null(), socket, package));
    data.dispose();
    free(client);
}

#[test]
#[cfg_attr(miri, ignore = "Miri has no OPENSSL_init_ssl")]
fn tls_handshake_names_the_host_and_verifies_the_peer() {
    let client = client(1, false, false);
    assert!(enable_ssl(Handle::raw(client)).is_some());
    assert!(enable_ssl(Handle::raw(client)).is_none());
    let tls = Handle::raw(client).get().tls.get().unwrap().clone();
    let name = c"example.test";
    {
        let mut owner = tls.borrow_configuration();
        assert!(owner.set_text(swoole_rs_corebase::tls::Text::HostName, name.to_bytes()));
        owner.set_value(swoole_rs_corebase::tls::Value::VerifyPeer(true));
        owner.set_value(swoole_rs_corebase::tls::Value::AllowSelfSigned(true));
    }
    mock(|m| m.ssl_connect_ready = true);
    assert!(ssl_handshake(Handle::raw(client)));
    assert_eq!(
        taken(),
        [
            "ssl create",
            "ssl session 2",
            "ssl ctrl 55",
            "ssl connect",
            "ssl verify true",
            "ssl host"
        ]
    );
    free(client);
}

#[test]
fn proxy_states_are_upstreams() {
    // Upstream's include/swoole_proxy.h counts its enums from 0.
    assert_eq!(
        [
            SOCKS5_WAIT,
            SOCKS5_HANDSHAKE,
            SOCKS5_READY,
            HTTP_PROXY_WAIT,
            HTTP_PROXY_READY
        ],
        [0, 1, 4, 0, 2],
        "SW_SOCKS5_STATE_{{WAIT,HANDSHAKE,READY}}, SW_HTTP_PROXY_STATE_{{WAIT,READY}}"
    );
}

#[test]
#[cfg_attr(miri, ignore = "Miri has no OPENSSL_init_ssl")]
fn pending_handshake_keeps_its_hostname_and_verification_snapshot() {
    let client = client(1, false, false);
    let handle = Handle::raw(client);
    assert!(enable_ssl(handle).is_some());
    let owner = handle.get().tls_context().unwrap().clone();
    {
        let mut loan = owner.borrow_configuration();
        assert!(loan.set_text(swoole_rs_corebase::tls::Text::HostName, b"first.test"));
        loan.set_value(swoole_rs_corebase::tls::Value::VerifyPeer(true));
        loan.set_value(swoole_rs_corebase::tls::Value::AllowSelfSigned(true));
    }
    assert!(ssl_handshake(handle));
    assert!(
        owner.fields().context.is_none(),
        "runtime context belongs to the snapshot"
    );
    _ = taken();
    {
        let mut loan = owner.borrow_configuration();
        assert!(loan.set_text(swoole_rs_corebase::tls::Text::HostName, b"later.test"));
        loan.set_value(swoole_rs_corebase::tls::Value::VerifyPeer(false));
        loan.set_value(swoole_rs_corebase::tls::Value::AllowSelfSigned(false));
    }
    mock(|m| m.ssl_connect_ready = true);
    assert!(ssl_handshake(handle));
    assert_eq!(handle.tls_host_name(), c"first.test".to_owned());
    assert_eq!(
        taken(),
        [
            "ssl session 2",
            "ssl ctrl 55",
            "ssl connect",
            "ssl verify true",
            "ssl host"
        ]
    );
    free(client);
}

#[test]
fn the_framed_receive_buffer_stays_usable_across_input_edits() {
    let client = client(1, true, true);
    let handle = Handle::raw(client);
    assert!(!handle.with_framed(|_, _, _| panic!("uninitialized input")));
    handle.input_init();
    handle.input_clear();
    assert!(handle.with_framed(|_, _, buffer| {
        buffer.append(b"partial");
        assert!(handle.with_framed(|_, _, _| panic!("nested read took the buffer")));
        buffer.size() >= CLIENT_BUFFER_SIZE
    }));
    assert!(handle.with_framed(|_, _, buffer| buffer.view() == b"partial"));
    handle.input_clear();
    assert!(handle.with_framed(|framing, _, buffer| {
        assert!(buffer.is_empty());
        buffer.append(b"last");
        free(client);
        assert!(framing.protocol().on_package.is_none());
        buffer.view() == b"last"
    }));
}
