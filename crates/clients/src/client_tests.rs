//! The operations against a recording double of the C++ client.
use super::*;
use std::cell::RefCell;
use swoole_rs_buffer::cpp::{CppBuffer, swr_cpp_buffer_append, swr_cpp_buffer_new};

#[derive(Default)]
struct Mock {
    log: Vec<String>,
    connect: Vec<c_int>,
    io: Vec<isize>,
    ssl_connect_ready: bool,
    socket_error: c_int,
    /// The callback that releases the client's socket object (as a PHP close would).
    releases: Option<c_int>,
    /// The record `make_socket` allocated; the client releases it.
    socket: Option<core::ptr::NonNull<SocketRecord>>,
    fail_socket: bool,
}

thread_local! {
    static MOCK: RefCell<Mock> = RefCell::new(Mock::default());
}

fn mock<T>(f: impl FnOnce(&mut Mock) -> T) -> T {
    MOCK.with(|m| f(&mut m.borrow_mut()))
}

fn log(entry: impl Into<String>) {
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
    ) -> *mut SocketRecord {
        log(format!("make {kind} {fd_type} {flags}"));
        if mock(|m| m.fail_socket) {
            set_errno(libc::EMFILE);
            return core::ptr::null_mut();
        }
        let mut record = SocketRecord::new();
        record.socket_type = kind;
        record.fd = -1;
        let socket = record.into_raw();
        mock(|m| m.socket = core::ptr::NonNull::new(socket));
        socket
    }

    pub(in super::super) fn kernel_timeout(_: &ClientRecord, timeout: f64) {
        log(format!("kernel {timeout}"));
    }
    pub(in super::super) fn assign(
        _: &mut ClientRecord,
        host: *const c_char,
        port: c_int,
        resolve: bool,
    ) -> bool {
        // SAFETY: the operations pass NUL-terminated hosts.
        let host = unsafe { CStr::from_ptr(host) }
            .to_string_lossy()
            .into_owned();
        log(format!("assign {host}:{port} {resolve}"));
        if host == "dns.test" {
            log::set_last_error(ERROR_BAD_HOST_ADDR);
            return false;
        }
        true
    }
    pub(in super::super) fn connect(_: &ClientRecord, mode: c_int) -> c_int {
        log(format!("connect {mode}"));
        let ret = mock(|m| m.connect.pop()).unwrap_or(0);
        if ret < 0 {
            set_errno(libc::EINPROGRESS);
        }
        ret
    }
    pub(in super::super) fn event(_: &ClientRecord, op: c_int, events: c_int) -> c_int {
        log(format!("event {op} {events}"));
        OK
    }
    pub(in super::super) fn socket_error(_: &ClientRecord, error: &mut c_int) -> c_int {
        *error = mock(|m| m.socket_error);
        OK
    }
}

/// The last socket `make_socket` made.
fn sock() -> &'static mut SocketRecord {
    // SAFETY: the test's live record.
    unsafe { mock(|m| m.socket).unwrap().as_mut() }
}

/// `sw_tg_buffer()`: a 64-byte String per test thread.
pub(super) fn thread_buffer() -> *mut c_void {
    thread_local! {
        static SHARED: *mut CppString = {
            let mut buffer = Box::new(CppString::EMPTY);
            buffer.alloc(64, core::ptr::null());
            Box::into_raw(buffer)
        };
    }
    SHARED.with(|shared| shared.cast())
}

// crates/tls's socket operations.
pub(super) unsafe fn socket_free(_: *mut SocketRecord) {
    log("free");
}
pub(super) unsafe fn socket_recv(
    _: *mut SocketRecord,
    _: *mut c_void,
    length: usize,
    _: c_int,
) -> isize {
    io("recv", length)
}
pub(super) unsafe fn socket_recv_sync(
    _: *mut SocketRecord,
    _: *mut c_void,
    length: usize,
    _: c_int,
) -> isize {
    io("recv_sync", length)
}
pub(super) unsafe fn socket_send_sync(
    _: *mut SocketRecord,
    _: *const c_void,
    length: usize,
    _: c_int,
) -> isize {
    io("send_sync", length)
}
pub(super) unsafe fn socket_recvfrom_sync_address(
    _: *mut SocketRecord,
    _: *mut c_void,
    length: usize,
    _: c_int,
    _: *mut SocketAddress,
) -> isize {
    io("recvfrom_sync", length)
}
pub(super) unsafe fn socket_sendfile_sync(
    _: *mut SocketRecord,
    _: *const c_char,
    _: i64,
    _: usize,
) -> c_int {
    log("sendfile false");
    OK
}
pub(super) unsafe fn socket_sendfile_async(
    _: *mut SocketRecord,
    _: *const c_char,
    _: i64,
    _: usize,
) -> c_int {
    log("sendfile true");
    OK
}
/// crates/reactor's, which reads the thread's reactor.
pub(super) unsafe extern "C-unwind" fn swoole_event_write(
    _: *mut SocketRecord,
    _: *const c_void,
    length: usize,
) -> isize {
    log(format!("write {length}"));
    if log::last_error() == ERROR_OUTPUT_BUFFER_OVERFLOW {
        ERR as isize
    } else {
        OK as isize
    }
}
/// crates/tls's `Reactor::_writable_callback`.
pub(super) unsafe fn writable_callback(_: *mut c_void, _: *mut swoole_rs_reactor::Event) -> c_int {
    log("writable");
    OK
}
/// The framed receives, as crates/ports registers them.
const FRAMING: Framing = Framing {
    length: |_, _, _| {
        log("framed false");
        0
    },
    eof: |_, _, _| {
        log("framed true");
        0
    },
};

/// A socket IO double: logs, then returns the queued result or the whole length.
fn io(name: &str, length: usize) -> isize {
    log(format!("{name} {length}"));
    mock(|m| m.io.pop()).unwrap_or(length.cast_signed())
}
extern "C" fn listener(_: *mut c_void, event: c_int, _: *const c_char, length: usize) {
    log(format!("call {event} {length}"));
    if mock(|m| m.releases) == Some(event) {
        sock().object = core::ptr::null_mut();
    }
}
/// crates/timer's `swr_v1_timer_add_plain_sec`: logs, adds nothing.
pub(super) unsafe fn swr_v1_timer_add_plain_sec(
    timeout: f64,
    _: bool,
    _: unsafe extern "C-unwind" fn(*mut c_void),
    _: *mut c_void,
) -> *mut c_void {
    log(format!("timer {timeout}"));
    core::ptr::without_provenance_mut(1)
}
/// crates/timer's `swoole_timer_del`: logs, deletes nothing.
pub(super) unsafe fn swoole_timer_del(_: *mut c_void) -> bool {
    log("timer del");
    true
}
/// crates/hooks' `async::dispatch`: logs the request; nothing completes it.
pub(super) unsafe extern "C" fn async_dispatch(event: *const c_void) -> *mut c_void {
    // SAFETY: a complete event over a live request.
    let view = swoole_rs_corebase::swr_v1_core_resolver_view(unsafe {
        &*(*event.cast::<AsyncEvent>()).data
    });
    // SAFETY: the request's name, live with it.
    let name = unsafe { swoole_rs_corebase::swr_v1_core_string_view(view.name) };
    // SAFETY: `length` bytes at `data`.
    let name = unsafe { core::slice::from_raw_parts(name.data.cast::<u8>(), name.length) };
    // SAFETY: the request's fields, live with it.
    let family = unsafe { (*view.fields).family };
    log(format!("dns {} {family}", String::from_utf8_lossy(name)));
    core::ptr::without_provenance_mut(8)
}
// crates/tls's TLS operations, over its own context.
pub(super) unsafe fn ssl_context_create(owner: *const OwnedTlsContext) -> bool {
    log("ssl create");
    // SAFETY: the client's live owner.
    unsafe { swoole_rs_tls::ssl_context_create(owner) }
}
pub(super) unsafe fn socket_ssl_create(
    _: *mut SocketRecord,
    _: *const OwnedTlsContext,
    flags: c_int,
) -> c_int {
    log(format!("ssl session {flags}"));
    OK
}
pub(super) fn socket_ssl_connect(_: Socket<'_>) -> c_int {
    log("ssl connect");
    if mock(|m| m.ssl_connect_ready) {
        sock().tls.state = SSL_STATE_READY;
    }
    OK
}
pub(super) fn socket_ssl_verify(_: &SocketRecord, allow_self_signed: bool) -> bool {
    log(format!("ssl verify {allow_self_signed}"));
    true
}
pub(super) fn socket_ssl_check_host(_: &SocketRecord, _: &CStr) -> bool {
    log("ssl host");
    true
}
pub(super) fn socket_ssl_close(_: Socket<'_>) {
    log("ssl close");
}
// crates/protocols-ffi's and OpenSSL's, which the hybrid links.
#[unsafe(export_name = "SSL_ctrl")]
extern "C" fn ssl_ctrl(
    _: *mut c_void,
    cmd: c_int,
    _: libc::c_long,
    _: *mut c_void,
) -> libc::c_long {
    log(format!("ssl ctrl {cmd}"));
    1
}
#[unsafe(export_name = "_ZN6swoole11Socks5Proxy5admitEmm")]
extern "C" fn socks5_admit(user: usize, password: usize) -> c_int {
    if user != 0 && password != 0 { 2 } else { 1 }
}
#[unsafe(export_name = "_ZN6swoole11Socks5Proxy22pack_negotiate_requestEv")]
extern "C" fn socks5_pack_negotiate(_: *const *mut c_void) -> isize {
    3
}
/// Any reply makes the proxy ready, with nothing more to send.
#[unsafe(export_name = "_ZN6swoole11Socks5Proxy14handshake_stepEPKcm")]
extern "C" fn socks5_handshake_step(
    proxy: *const *mut c_void,
    _: *const c_char,
    length: usize,
) -> isize {
    log(format!("socks5 {length}"));
    // SAFETY: the client's live SOCKS5 owner.
    unsafe { (*(*(*proxy).cast::<ProxyState>()).fields.get()).state = 4 };
    0
}

/// Gives the client a proxy at proxy.test:1080.
fn set_proxy(client: *mut Client, http: bool) {
    let host = c"proxy.test";
    // SAFETY: a live state and texts of their lengths.
    unsafe {
        swr_v1_clients_proxy(
            client,
            http,
            host.as_ptr(),
            host.count_bytes(),
            1080,
            c"".as_ptr(),
            0,
            c"".as_ptr(),
            0,
        );
    }
}

fn new_client(kind: c_int, is_async: bool) -> *mut Client {
    swr_v1_clients_new(kind, is_async)
}

/// A client of `kind`, its construction log dropped, listened to for every event
/// with `callbacks`.
fn client(kind: c_int, is_async: bool, callbacks: bool) -> *mut Client {
    mock(|m| *m = Mock::default());
    set_framing(FRAMING);
    log::set_last_error(0);
    let client = new_client(kind, is_async);
    if callbacks {
        // SAFETY: a live state.
        swr_v1_clients_listen(
            unsafe { &*client },
            core::ptr::null_mut(),
            0b111_1110,
            listener,
        );
    }
    taken();
    client
}

fn free(client: *mut Client) {
    super::free(Handle(client));
}

/// `Client_onWrite` with a reactor and event the mocks ignore.
fn writable(client: *mut Client) -> c_int {
    let mut event = swoole_rs_reactor::Event {
        fd: 0,
        reactor_id: 0,
        fd_type: 0,
        socket: core::ptr::null_mut(),
    };
    // SAFETY: the mocked effects never touch the reactor.
    let reactor = unsafe { Running::new(core::ptr::null_mut()) };
    on_write(Handle(client), reactor, &mut event)
}

fn flag(client: *mut Client, name: u32) -> bool {
    // SAFETY: a live state.
    swr_v1_clients_flag(unsafe { &*client }, name)
}

fn connect(client: *mut Client, host: &CStr, timeout: f64, flags: c_int) -> c_int {
    super::connect(Handle(client), host.as_ptr(), 80, timeout, flags)
}

#[test]
fn construction_makes_a_cloexec_socket_nonblocking_when_async() {
    mock(|m| *m = Mock::default());
    log::set_last_error(0);
    let client = new_client(1, true);
    assert_eq!(taken(), ["make 1 17 12"]);
    assert!(flag(client, ASYNC) && flag(client, STREAM));
    assert_eq!(sock().chunk_size, SEND_BUFFER_SIZE);
    free(client);
    assert_eq!(taken(), ["event 2 0", "free"]);
    let client = new_client(2, false);
    assert_eq!(taken(), ["make 2 18 8"]);
    free(client);
}

#[test]
fn a_call_measures_with_the_protocol_and_frees_the_client() {
    unsafe extern "C" fn twice(
        _: *const Protocol,
        _: *mut SocketRecord,
        packet: *const PacketLength,
    ) -> isize {
        // SAFETY: the call's packet.
        2 * isize::try_from(unsafe { (*packet).buf_size }).unwrap()
    }
    let client = client(1, true, false);
    Handle(client).fields().protocol.get_package_length = (twice as LengthFunc) as *mut c_void;
    let mut call = Call::new(client, Op::PackageLength);
    call.buffer = c"abc".as_ptr().cast_mut();
    call.length = 3;
    // SAFETY: a live client and three bytes.
    unsafe { swr_v1_clients_run((&raw mut call).cast()) };
    assert_eq!(call.result, 6);
    let mut call = Call::new(client, Op::Free);
    // SAFETY: the live client, not used afterwards.
    unsafe { swr_v1_clients_run((&raw mut call).cast()) };
    assert_eq!(taken(), ["event 2 0", "free"]);
}

#[test]
fn async_close_detaches_then_runs_on_close_once() {
    let client = client(1, true, true);
    Handle(client).set(ACTIVE, true);
    assert_eq!(close(Handle(client)), ERR, "close(-1) fails");
    assert_eq!(taken(), ["event 2 0", "call 3 0"]);
    assert!(flag(client, CLOSED) && !flag(client, ACTIVE));
    assert_eq!(close(Handle(client)), ERR);
    assert_eq!(taken(), Vec::<String>::new());
    free(client);
}

#[test]
fn close_returns_zero_when_on_close_released_the_client() {
    let client = client(1, true, true);
    Handle(client).set(ACTIVE, true);
    mock(|m| m.releases = Some(ON_CLOSE));
    assert_eq!(close(Handle(client)), OK);
    assert_eq!(sock().fd, -1);
    free(client);
}

#[test]
fn shutdown_records_each_direction_once() {
    let client = client(1, false, false);
    let shutdown = |how| shutdown(Handle(client), how);
    assert_eq!(shutdown(99), ERR);
    assert_eq!(log::last_error(), libc::EINVAL);
    // The fd is -1, so the system call fails.
    assert_eq!(shutdown(libc::SHUT_RD), ERR);
    assert!(!flag(client, SHUTDOWN_READ));
    Handle(client).set(SHUTDOWN_WRITE, true);
    assert_eq!(shutdown(libc::SHUT_WR), ERR);
    Handle(client).set(CLOSED, true);
    assert_eq!(shutdown(libc::SHUT_RDWR), ERR);
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
    mock(|m| m.connect = vec![-1]);
    let result = connect(client, c"example.test", 2.5, 0);
    assert_eq!(result, OK);
    assert_eq!(
        taken(),
        [
            "assign example.test:80 false",
            "connect 2",
            "timer 2.5",
            "event 0 1024"
        ]
    );
    free(client);
    assert_eq!(
        taken(),
        ["event 2 0", "timer del", "free"],
        "inactive: no onClose"
    );
}

#[test]
fn async_connect_waits_for_dns_then_connects_the_resolved_address() {
    let client = client(1, true, true);
    assert_eq!(connect(client, c"dns.test", 1.0, 0), OK);
    assert!(flag(client, WAIT_DNS));
    assert_eq!(taken(), ["assign dns.test:80 false", "dns dns.test 2"]);
    resolved(Handle(client), 0, c"192.0.2.1");
    assert!(!flag(client, WAIT_DNS));
    assert_eq!(
        taken(),
        ["assign 192.0.2.1:80 false", "connect 2", "event 0 1024"]
    );
    free(client);
}

#[test]
fn failed_resolution_closes_and_reports_once() {
    let client = client(1, true, true);
    connect(client, c"dns.test", 1.0, 0);
    taken();
    resolved(Handle(client), 1, c"");
    assert_eq!(log::last_error(), ERROR_DNSLOOKUP_RESOLVE_FAILED);
    assert!(flag(client, ONERROR_CALLED) && flag(client, CLOSED));
    assert_eq!(
        taken(),
        [format!("call 4 {ERROR_DNSLOOKUP_RESOLVE_FAILED}")],
        "removed: no reactor delete; the error is the length"
    );
    free(client);
}

#[test]
fn sync_nonblocking_connect_reports_ready_and_wait() {
    let client = client(1, false, false);
    mock(|m| m.connect = vec![SW_READY]);
    assert_eq!(connect(client, c"example.test", -1.0, 1), OK);
    mock(|m| m.connect = vec![SW_WAIT]);
    assert_eq!(connect(client, c"example.test", -1.0, 1), ERR);
    assert!(flag(client, ASYNC_CONNECT));
    free(client);
}

#[test]
fn sync_connect_through_socks5_routes_and_handshakes() {
    let client = client(1, false, false);
    set_proxy(client, false);
    assert_eq!(Handle(client).proxy_state(Proxy::Socks5), SOCKS5_WAIT);
    assert_eq!(connect(client, c"example.test", 3.0, 0), OK);
    assert_eq!(
        taken(),
        [
            "kernel 3",
            "assign proxy.test:1080 true",
            "connect 1",
            "send_sync 3",
            "recv_sync 64",
            "socks5 64",
        ]
    );
    assert!(flag(client, ACTIVE));
    assert_eq!(Handle(client).proxy_state(Proxy::Socks5), SOCKS5_READY);
    let proxy = Handle(client).proxy(Proxy::Socks5).unwrap();
    // SAFETY: the client's live proxy.
    unsafe {
        assert_eq!(CStr::from_ptr(proxy.string(3).data), c"example.test");
        assert_eq!((*proxy.fields.get()).target_port, 80);
    }
    assert_eq!(
        Handle(client).get().server_host.borrow().to_bytes(),
        b"proxy.test"
    );
    free(client);
}

#[test]
fn async_send_raises_buffer_full_at_the_high_watermark() {
    let client = client(1, true, true);
    // Ten bytes queued on the socket.
    let mut storage = Box::new(core::mem::MaybeUninit::<CppBuffer>::uninit());
    swr_cpp_buffer_new(&mut storage, 1024);
    let queue = Box::into_raw(storage).cast::<CppBuffer>();
    unsafe { swr_cpp_buffer_append(&*queue, [0_u8; 10].as_ptr(), 10) };
    sock().output.out_buffer = queue.cast();
    // SAFETY: a live state.
    unsafe { swr_v1_clients_watermark(&*client, true).write(8) };
    assert_eq!(send(Handle(client), c"data".as_ptr(), 4, 0), 4);
    assert_eq!(taken(), ["write 4", "call 5 0"]);
    // Already high: no second call.
    send(Handle(client), c"data".as_ptr(), 4, 0);
    assert_eq!(taken(), ["write 4"]);
    log::set_last_error(ERROR_OUTPUT_BUFFER_OVERFLOW);
    assert_eq!(send(Handle(client), c"data".as_ptr(), 4, 0), -1);
    free(client);
}

#[test]
fn writable_after_connect_starts_the_socks5_handshake() {
    let client = client(1, true, true);
    set_proxy(client, false);
    // SAFETY: a live state.
    unsafe { swr_v1_clients_watermark(&*client, true).write(100) };
    let result = writable(client);
    assert_eq!(result, 3);
    assert_eq!(taken(), ["event 1 512", "write 3"]);
    assert_eq!(Handle(client).proxy_state(Proxy::Socks5), SOCKS5_HANDSHAKE);
    assert!(flag(client, ACTIVE));
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
            "event 2 0".to_owned(),
            format!("call 4 {}", libc::ECONNREFUSED)
        ]
    );
    free(client);
}

#[test]
fn tls_connect_waits_for_the_stream_then_connects() {
    let client = client(1, true, true);
    assert_eq!(enable_ssl(Handle(client)), OK);
    taken();
    writable(client);
    assert_eq!(sock().tls.state, SSL_STATE_WAIT_STREAM);
    taken();
    mock(|m| m.ssl_connect_ready = true);
    writable(client);
    assert_eq!(taken(), ["ssl session 2", "ssl connect", "call 1 0"]);
    free(client);
}

#[test]
fn connect_timeout_with_a_pending_proxy_fails_without_on_close() {
    let client = client(1, true, true);
    Handle(client).set(ACTIVE, true);
    set_proxy(client, true);
    assert_eq!(Handle(client).proxy_state(Proxy::Http), HTTP_PROXY_WAIT);
    unsafe { on_timeout(client.cast()) };
    assert_eq!(log::last_error(), libc::ETIMEDOUT);
    assert_eq!(
        taken(),
        [
            "event 2 0".to_owned(),
            format!("call 4 {}", libc::ETIMEDOUT)
        ]
    );
    free(client);
}

#[test]
fn stream_read_delivers_data_and_closes_at_eof() {
    let client = client(1, true, true);
    Handle(client).set(ACTIVE, true);
    Handle(client).input_init();
    mock(|m| m.io = vec![5]);
    assert_eq!(on_read(Handle(client)), OK);
    assert_eq!(taken(), ["recv 65536", "call 2 5"]);
    mock(|m| m.io = vec![0]);
    on_read(Handle(client));
    assert_eq!(taken(), ["recv 65536", "event 2 0", "call 3 0"]);
    free(client);
}

#[test]
fn datagram_sends_fail_with_sendto_and_dtls_sends_as_a_stream() {
    let client = client(2, false, false);
    // The record's descriptor is -1, so sendto(2) fails.
    assert_eq!(send(Handle(client), c"data".as_ptr(), 4, 0), -1);
    assert_eq!(errno(), libc::EBADF);
    assert_eq!(enable_ssl(Handle(client)), OK);
    assert_eq!(sock().chunk_size, SSL_BUFFER_SIZE);
    taken();
    assert_eq!(send(Handle(client), c"data".as_ptr(), 4, 0), 4);
    assert_eq!(taken(), ["send_sync 4"]);
    free(client);
}

#[test]
fn public_record_defaults_and_direct_framing_writes_drive_reads() {
    let client = client(1, true, true);
    let fields = client.cast::<ClientRecord>();
    unsafe {
        assert!(!(*fields).http2 && !(*fields).open_eof_check && !(*fields).open_length_check);
        assert_eq!((*fields).remote_addr.kind, 1);
        assert_eq!((*fields).server_addr.len, 0);
        assert_eq!((*fields).protocol.package_length_type, b'N');
        assert_eq!((*fields).protocol.package_length_size, 4);
        assert_eq!((*fields).protocol.package_max_length, 2 * 1024 * 1024);
        assert_eq!((*fields).protocol.on_package, on_package as *mut c_void);
        assert_eq!((*fields).socket, Handle(client).socket_ptr());
        assert!((*fields).object.is_null() && (*fields).buffer.is_null());
        Handle(client).input_init();
        (*fields).open_length_check = true;
        assert_eq!(on_read(Handle(client)), OK);
        assert!(taken().contains(&"framed false".to_owned()));
        (*fields).open_eof_check = true;
        assert_eq!(on_read(Handle(client)), OK);
        assert!(taken().contains(&"framed true".to_owned()));
    }
    free(client);
}

#[test]
fn datagram_peer_snapshot_is_independent_and_may_target_its_source() {
    let client = client(2, false, false);
    let fields = client.cast::<ClientRecord>();
    unsafe {
        (*fields).remote_addr.len = 17;
        (*fields).remote_addr.kind = 4;
        let mut result = core::mem::MaybeUninit::uninit();
        assert_eq!(
            swr_v1_clients_get_peer_name(client, result.as_mut_ptr()),
            OK
        );
        let result = result.assume_init();
        (*fields).remote_addr.len = 29;
        assert_eq!(result.len, 17);
        assert_eq!(result.kind, 4);
        assert_eq!(
            swr_v1_clients_get_peer_name(client, &raw mut (*fields).remote_addr),
            OK
        );
        assert_eq!((*fields).remote_addr.len, 29);
    }
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
fn framed_packages_reach_on_receive_until_the_socket_waits_to_close() {
    let client = client(1, true, true);
    let socket = Handle(client).socket_ptr();
    let mut package = RecvData {
        info: swoole_rs_corebase::DataHead::default(),
        data: c"data".as_ptr(),
    };
    package.info.len = 4;
    assert_eq!(
        on_package(core::ptr::null(), socket, &raw const package),
        OK
    );
    assert_eq!(taken(), ["call 2 4"]);
    sock().output.close_wait = 1;
    assert_eq!(
        on_package(core::ptr::null(), socket, &raw const package),
        ERR
    );
    free(client);
}

#[test]
fn tls_handshake_names_the_host_and_verifies_the_peer() {
    let client = client(1, false, false);
    assert_eq!(enable_ssl(Handle(client)), OK);
    assert_eq!(enable_ssl(Handle(client)), ERR);
    let tls = Handle(client).tls();
    let name = c"example.test";
    // SAFETY: the double's live slot and fields.
    unsafe {
        let slot = tls.strings[TLS_HOST_NAME].cast();
        assert_eq!(
            swoole_rs_corebase::swr_v1_core_string_set(slot, name.as_ptr(), name.count_bytes()),
            0
        );
        (*tls.fields).verify_peer = 1;
        (*tls.fields).allow_self_signed = 1;
    }
    mock(|m| m.ssl_connect_ready = true);
    assert_eq!(ssl_handshake(Handle(client)), OK);
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
