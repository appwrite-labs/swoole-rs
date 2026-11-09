//! The operations against a recording double of the C++ client.
use super::*;
use std::cell::RefCell;

#[derive(Default)]
struct Mock {
    log: Vec<String>,
    proxy: [c_int; 2],
    connect: Vec<c_int>,
    io: Vec<isize>,
    ssl_connect_ready: bool,
    socket_error: c_int,
    out_buffer: u32,
    /// The callback that releases the client's socket object (as a PHP close would).
    releases: Option<c_int>,
    socket: Option<Box<SocketRecord>>,
    record: Option<Box<core::mem::MaybeUninit<ClientRecord>>>,
    fail_socket: bool,
}

thread_local! {
    static MOCK: RefCell<Mock> = RefCell::new(Mock { proxy: [-1, -1], ..Mock::default() });
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

extern "C" fn make_socket(kind: c_int, fd_type: c_int, flags: c_int) -> *mut SocketRecord {
    log(format!("make {kind} {fd_type} {flags}"));
    if mock(|m| m.fail_socket) {
        set_errno(libc::EMFILE);
        return core::ptr::null_mut();
    }
    let mut socket = Box::new(SocketRecord::new());
    socket.socket_type = kind;
    socket.fd = -1;
    let pointer: *mut SocketRecord = &raw mut *socket;
    mock(|m| m.socket = Some(socket));
    pointer
}
extern "C" fn free_socket(_: *mut c_void, deferred: bool) {
    log(format!("free {deferred}"));
}
extern "C" fn listener(_: *mut c_void, event: c_int, _: *const c_char, length: usize) {
    log(format!("call {event} {length}"));
    if mock(|m| m.releases) == Some(event) {
        mock(|m| m.socket.as_mut().unwrap().object = core::ptr::null_mut());
    }
}
// crates/timer's, which the hybrid links.
#[unsafe(no_mangle)]
extern "C-unwind" fn swr_v1_timer_add_plain_sec(
    timeout: f64,
    _: bool,
    _: extern "C-unwind" fn(*mut c_void),
    _: *mut c_void,
) -> *mut c_void {
    log(format!("timer {timeout}"));
    core::ptr::without_provenance_mut(1)
}
#[unsafe(export_name = "_Z16swoole_timer_delPN6swoole9TimerNodeE")]
extern "C-unwind" fn timer_del(_: *mut c_void) -> bool {
    log("timer del");
    true
}
extern "C" fn kernel_timeout(_: *mut ClientRecord, timeout: f64) {
    log(format!("kernel {timeout}"));
}
extern "C" fn assign(
    _: *mut ClientRecord,
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
extern "C" fn connect_socket(_: *mut ClientRecord, mode: c_int) -> c_int {
    log(format!("connect {mode}"));
    let ret = mock(|m| m.connect.pop()).unwrap_or(0);
    if ret < 0 {
        set_errno(libc::EINPROGRESS);
    }
    ret
}
extern "C" fn io(_: *mut c_void, op: c_int, _: *mut c_char, length: usize, _: c_int) -> isize {
    log(format!("io {op} {length}"));
    mock(|m| m.io.pop()).unwrap_or(length.cast_signed())
}
extern "C" fn sendfile(
    _: *mut c_void,
    _: *const c_char,
    _: i64,
    _: usize,
    is_async: bool,
) -> c_int {
    log(format!("sendfile {is_async}"));
    OK
}
extern "C" fn event(_: *mut ClientRecord, op: c_int, events: c_int) -> c_int {
    log(format!("event {op} {events}"));
    OK
}
extern "C" fn event_write(_: *mut c_void, _: *const c_char, length: usize) -> c_int {
    log(format!("write {length}"));
    if log::last_error() == ERROR_OUTPUT_BUFFER_OVERFLOW {
        ERR
    } else {
        OK
    }
}
extern "C" fn out_buffer_length(_: *mut c_void) -> u32 {
    mock(|m| m.out_buffer)
}
extern "C" fn writable(_: *mut c_void, _: *mut c_void) -> c_int {
    log("writable");
    OK
}
extern "C" fn socket_error(_: *mut ClientRecord, error: *mut c_int) -> c_int {
    // SAFETY: the operation passes a writable int.
    unsafe { *error = mock(|m| m.socket_error) };
    OK
}
extern "C" fn dns_lookup(_: *mut c_void, host: *const c_char, domain: c_int) -> c_int {
    // SAFETY: a NUL-terminated host.
    let host = unsafe { CStr::from_ptr(host) }
        .to_string_lossy()
        .into_owned();
    log(format!("dns {host} {domain}"));
    OK
}
extern "C" fn ssl(_: *mut c_void, op: c_int, arg: c_int) -> c_int {
    log(format!("ssl {op} {arg}"));
    if op == SSL_CONNECT && mock(|m| m.ssl_connect_ready) {
        mock(|m| m.socket.as_mut().unwrap().tls.state = SSL_STATE_READY);
    }
    if op == SSL_HAS_CONTEXT { 1 } else { OK }
}
extern "C" fn proxy_state(_: *mut c_void, proxy: c_int) -> c_int {
    mock(|m| m.proxy[proxy.unsigned_abs() as usize])
}
extern "C" fn set_proxy_state(_: *mut c_void, proxy: c_int, state: c_int) {
    log(format!("proxy {proxy} {state}"));
    mock(|m| m.proxy[proxy.unsigned_abs() as usize] = state);
}
extern "C" fn proxy_route(
    _: *mut c_void,
    proxy: c_int,
    _: *const c_char,
    port: c_int,
    out: *mut c_int,
) -> *const c_char {
    if mock(|m| m.proxy[proxy.unsigned_abs() as usize]) < 0 {
        return core::ptr::null();
    }
    log(format!("route {proxy} {port}"));
    // SAFETY: the operation passes a writable int.
    unsafe { *out = 1080 };
    c"proxy.test".as_ptr()
}
extern "C" fn socks5_negotiate(_: *mut c_void, data: *mut *const c_char) -> isize {
    // SAFETY: the operation passes a writable pointer.
    unsafe { *data = c"\x05".as_ptr() };
    3
}
extern "C" fn socks5_handshake(_: *mut c_void, _: *const c_char, length: usize) -> bool {
    log(format!("socks5 {length}"));
    mock(|m| m.proxy[0] = SOCKS5_READY);
    true
}
extern "C" fn http_pack(_: *mut c_void, data: *mut *const c_char) -> usize {
    // SAFETY: the operation passes a writable pointer.
    unsafe { *data = c"CONNECT".as_ptr() };
    7
}
extern "C" fn http_handshake(_: *mut c_void, shared: bool, length: usize) -> bool {
    log(format!("http {shared} {length}"));
    true
}
extern "C" fn buffer(_: *mut c_void, op: c_int, _: usize, view: *mut BufferView) {
    static mut STORAGE: [c_char; 64] = [0; 64];
    log(format!("buffer {op}"));
    // SAFETY: the operation passes a writable view; the storage is this test thread's.
    unsafe {
        *view = BufferView {
            str: (&raw mut STORAGE).cast(),
            length: 0,
            size: 64,
        }
    };
}
extern "C" fn recv_framed(_: *mut c_void, eof: bool) -> isize {
    log(format!("framed {eof}"));
    0
}

static NATIVE: Native = Native {
    make_socket,
    free_socket,
    kernel_timeout,
    assign,
    connect: connect_socket,
    io,
    sendfile,
    event,
    event_write,
    out_buffer_length,
    writable,
    socket_error,
    dns_lookup,
    ssl,
    proxy_state,
    set_proxy_state,
    proxy_route,
    socks5_negotiate,
    socks5_handshake,
    http_pack,
    http_handshake,
    buffer,
    recv_framed,
};

unsafe fn new_client(kind: c_int, is_async: bool) -> *mut Client {
    let mut record = Box::new(core::mem::MaybeUninit::uninit());
    let pointer = record.as_mut_ptr();
    mock(|m| m.record = Some(record));
    unsafe {
        swr_v1_clients_new(
            core::ptr::without_provenance_mut(8),
            &NATIVE,
            pointer,
            kind,
            is_async,
        )
    }
}

/// A client of `kind`, its construction log dropped, listened to for every event
/// with `callbacks`.
fn client(kind: c_int, is_async: bool, callbacks: bool) -> *mut Client {
    mock(|m| {
        *m = Mock {
            proxy: [-1, -1],
            ..Mock::default()
        }
    });
    log::set_last_error(0);
    // SAFETY: the double stands in for the C++ client.
    let client = unsafe { new_client(kind, is_async) };
    if callbacks {
        // SAFETY: a live state.
        unsafe { swr_v1_clients_listen(client, core::ptr::null_mut(), 0b111_1110, listener) };
    }
    taken();
    client
}

fn free(client: *mut Client) {
    // SAFETY: a live state, released once.
    unsafe { swr_v1_clients_free(client) };
}

fn flag(client: *mut Client, name: u32) -> bool {
    // SAFETY: a live state.
    unsafe { swr_v1_clients_flag(client, name) }
}

fn connect(client: *mut Client, host: &CStr, timeout: f64, flags: c_int) -> c_int {
    // SAFETY: a live state and a NUL-terminated host.
    unsafe { swr_v1_clients_connect(client, host.as_ptr(), 80, timeout, flags) }
}

#[test]
fn construction_makes_a_cloexec_socket_nonblocking_when_async() {
    mock(|m| {
        *m = Mock {
            proxy: [-1, -1],
            ..Mock::default()
        }
    });
    log::set_last_error(0);
    // SAFETY: the double stands in for the C++ client.
    let client = unsafe { new_client(1, true) };
    assert_eq!(taken(), ["make 1 17 12"]);
    assert!(flag(client, ASYNC) && flag(client, STREAM));
    assert_eq!(
        mock(|m| m.socket.as_ref().unwrap().chunk_size),
        SEND_BUFFER_SIZE
    );
    free(client);
    assert_eq!(taken(), ["event 2 0", "buffer 4", "free true"]);
    let client = unsafe { new_client(2, false) };
    assert_eq!(taken(), ["make 2 18 8"]);
    free(client);
}

#[test]
fn async_close_detaches_then_runs_on_close_once() {
    let client = client(1, true, true);
    Handle(client).set(ACTIVE, true);
    // SAFETY: a live state.
    assert_eq!(
        unsafe { swr_v1_clients_close(client) },
        ERR,
        "close(-1) fails"
    );
    assert_eq!(taken(), ["event 2 0", "call 3 0"]);
    assert!(flag(client, CLOSED) && !flag(client, ACTIVE));
    // SAFETY: as above.
    assert_eq!(unsafe { swr_v1_clients_close(client) }, ERR);
    assert_eq!(taken(), Vec::<String>::new());
    free(client);
}

#[test]
fn close_returns_zero_when_on_close_released_the_client() {
    let client = client(1, true, true);
    Handle(client).set(ACTIVE, true);
    mock(|m| m.releases = Some(ON_CLOSE));
    // SAFETY: a live state.
    assert_eq!(unsafe { swr_v1_clients_close(client) }, OK);
    assert_eq!(mock(|m| m.socket.as_ref().unwrap().fd), -1);
    free(client);
}

#[test]
fn shutdown_records_each_direction_once() {
    let client = client(1, false, false);
    // SAFETY: a live state.
    let shutdown = |how| unsafe { swr_v1_clients_shutdown(client, how) };
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
            "buffer 0",
            "assign example.test:80 false",
            "connect 2",
            "timer 2.5",
            "event 0 1024"
        ]
    );
    free(client);
    assert_eq!(
        taken(),
        ["event 2 0", "timer del", "buffer 4", "free true"],
        "inactive: no onClose"
    );
}

#[test]
fn async_connect_waits_for_dns_then_connects_the_resolved_address() {
    let client = client(1, true, true);
    assert_eq!(connect(client, c"dns.test", 1.0, 0), OK);
    assert!(flag(client, WAIT_DNS));
    assert_eq!(
        taken(),
        ["buffer 0", "assign dns.test:80 false", "dns dns.test 2"]
    );
    // SAFETY: a live state and a NUL-terminated address.
    unsafe { swr_v1_clients_on_resolved(client, 0, c"192.0.2.1".as_ptr()) };
    assert!(!flag(client, WAIT_DNS));
    assert_eq!(
        taken(),
        [
            "buffer 0",
            "assign 192.0.2.1:80 false",
            "connect 2",
            "event 0 1024"
        ]
    );
    free(client);
}

#[test]
fn failed_resolution_closes_and_reports_once() {
    let client = client(1, true, true);
    connect(client, c"dns.test", 1.0, 0);
    taken();
    // SAFETY: a live state.
    unsafe { swr_v1_clients_on_resolved(client, 1, c"".as_ptr()) };
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
    mock(|m| m.proxy[0] = SOCKS5_WAIT);
    assert_eq!(connect(client, c"example.test", 3.0, 0), OK);
    assert_eq!(
        taken(),
        [
            "kernel 3",
            "route 0 80",
            "assign proxy.test:1080 true",
            "connect 1",
            "buffer 5",
            "io 2 3",
            "proxy 0 1",
            "io 1 64",
            "socks5 64",
        ]
    );
    assert!(flag(client, ACTIVE));
    assert_eq!(
        Handle(client).get().server_host.borrow().to_bytes(),
        b"proxy.test"
    );
    free(client);
}

#[test]
fn async_send_raises_buffer_full_at_the_high_watermark() {
    let client = client(1, true, true);
    mock(|m| m.out_buffer = 10);
    // SAFETY: a live state.
    unsafe { swr_v1_clients_watermark(client, true).write(8) };
    // SAFETY: a live state and four bytes.
    assert_eq!(
        unsafe { swr_v1_clients_send(client, c"data".as_ptr(), 4, 0) },
        4
    );
    assert_eq!(taken(), ["write 4", "call 5 0"]);
    // Already high: no second call.
    // SAFETY: as above.
    unsafe { swr_v1_clients_send(client, c"data".as_ptr(), 4, 0) };
    assert_eq!(taken(), ["write 4"]);
    log::set_last_error(ERROR_OUTPUT_BUFFER_OVERFLOW);
    // SAFETY: as above.
    assert_eq!(
        unsafe { swr_v1_clients_send(client, c"data".as_ptr(), 4, 0) },
        -1
    );
    free(client);
}

#[test]
fn writable_after_connect_starts_the_socks5_handshake() {
    let client = client(1, true, true);
    mock(|m| m.proxy[0] = SOCKS5_WAIT);
    // SAFETY: a live state.
    unsafe { swr_v1_clients_watermark(client, true).write(100) };
    // SAFETY: as above.
    let result = on_write(Handle(client), core::ptr::null_mut(), core::ptr::null_mut());
    assert_eq!(result, 3);
    assert_eq!(taken(), ["event 1 512", "proxy 0 1", "write 3"]);
    assert!(flag(client, ACTIVE));
    free(client);
}

#[test]
fn writable_after_a_failed_connect_reports_the_error() {
    let client = client(1, true, true);
    mock(|m| m.socket_error = libc::ECONNREFUSED);
    // SAFETY: a live state.
    on_write(Handle(client), core::ptr::null_mut(), core::ptr::null_mut());
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
    // SAFETY: a live state.
    assert_eq!(unsafe { swr_v1_clients_enable_ssl(client) }, OK);
    taken();
    // SAFETY: as above.
    on_write(Handle(client), core::ptr::null_mut(), core::ptr::null_mut());
    assert_eq!(
        mock(|m| m.socket.as_ref().unwrap().tls.state),
        SSL_STATE_WAIT_STREAM
    );
    taken();
    mock(|m| m.ssl_connect_ready = true);
    // SAFETY: as above.
    on_write(Handle(client), core::ptr::null_mut(), core::ptr::null_mut());
    assert_eq!(
        taken(),
        ["ssl 2 0", "ssl 3 0", "ssl 4 0", "ssl 5 0", "call 1 0"]
    );
    free(client);
}

#[test]
fn connect_timeout_with_a_pending_proxy_fails_without_on_close() {
    let client = client(1, true, true);
    Handle(client).set(ACTIVE, true);
    mock(|m| m.proxy[1] = HTTP_PROXY_WAIT);
    // SAFETY: a live state.
    on_timeout(client.cast());
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
    mock(|m| m.io = vec![5]);
    // SAFETY: a live state.
    assert_eq!(on_read(Handle(client)), OK);
    assert_eq!(taken(), ["buffer 1", "io 0 64", "call 2 5"]);
    mock(|m| m.io = vec![0]);
    // SAFETY: as above.
    on_read(Handle(client));
    assert_eq!(taken(), ["buffer 1", "io 0 64", "event 2 0", "call 3 0"]);
    free(client);
}

#[test]
fn datagram_sends_fail_when_short_and_dtls_sends_as_a_stream() {
    let client = client(2, false, false);
    mock(|m| m.io = vec![3]);
    // SAFETY: a live state and four bytes.
    assert_eq!(
        unsafe { swr_v1_clients_send(client, c"data".as_ptr(), 4, 0) },
        -1
    );
    assert_eq!(taken(), ["io 3 4"]);
    // SAFETY: as above.
    assert_eq!(unsafe { swr_v1_clients_enable_ssl(client) }, OK);
    assert_eq!(
        mock(|m| m.socket.as_ref().unwrap().chunk_size),
        SSL_BUFFER_SIZE
    );
    taken();
    // SAFETY: as above.
    assert_eq!(
        unsafe { swr_v1_clients_send(client, c"data".as_ptr(), 4, 0) },
        4
    );
    assert_eq!(taken(), ["io 2 4"]);
    free(client);
}

#[test]
fn public_record_defaults_and_direct_framing_writes_drive_reads() {
    let client = client(1, true, true);
    let fields = Handle(client).get().record;
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
    let fields = Handle(client).get().record;
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
fn failed_socket_leaves_zero_protocol_and_addresses_without_teardown_effects() {
    mock(|m| {
        *m = Mock {
            fail_socket: true,
            ..Mock::default()
        }
    });
    unsafe {
        let client = new_client(1, false);
        assert_eq!(errno(), libc::EMFILE);
        let fields = &*Handle(client).get().record;
        assert!(fields.socket.is_null());
        assert_eq!(fields.remote_addr.kind, 0);
        assert_eq!(fields.protocol.package_length_size, 0);
        assert!(fields.protocol.on_package.is_null());
        taken();
        free(client);
        assert_eq!(taken(), Vec::<String>::new());
    }
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
    mock(|m| m.socket.as_mut().unwrap().output.close_wait = 1);
    assert_eq!(
        on_package(core::ptr::null(), socket, &raw const package),
        ERR
    );
    free(client);
}
