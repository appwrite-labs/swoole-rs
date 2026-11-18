//! The Servers upstream's `core-tests/src/server/http_server.cpp` builds (`test_base_server`,
//! `test_http_server`, `test_ssl_http`, `abort_connection`'s and `get_package_length`'s),
//! `core-tests/src/protocol/base.cpp` builds, and `protocol/redis.cpp`'s and `server/mqtt.cpp`'s,
//! for main.rs, tests/modules/framing/protocol.rs and tests/modules/protocols/server.rs. They
//! were C++ (fixture.cc) and keep its entry points' names and signatures. Where fixture.cc's
//! lambdas captured the case's function, the slot's fn reads it from a static the case set.

use std::ffi::{CStr, c_char, c_int, c_long, c_void};
use std::mem::offset_of;
use std::ptr;
use std::sync::Mutex;

use super::{
    COMPRESSION_TYPES, Callback, Connection, DataHead, LOCATIONS, MODE_BASE, MODE_PROCESS, Port,
    Protocol, RecvData, SW_LOG_WARNING, SW_SOCK_TCP, Server, Slot, TEST_HOST, TEXT_CHROOT,
    TEXT_GROUP, TEXT_USER, Worker, ignore_event, ignore_server, last_error, microtime,
    swoole_set_log_level,
};
use crate::{Event, Hook, Receive, StringBuffer};
use swoole_rs_ports::{lifecycle, methods};
use swoole_rs_server::{ConnectionRecord, server_methods, view};

pub(super) const SW_LOG_INFO: c_int = 2;
pub(super) const SW_SOCK_SSL: c_int = 512;
const CLOSE_POLICY_ERROR: c_int = 1008;
// SSLContext's text slots (native/tls/swoole_ssl.h).
pub(super) const CERT_FILE: u32 = 4;
pub(super) const KEY_FILE: u32 = 5;
pub(super) const CLIENT_CERT_FILE: u32 = 7;
const CAFILE: u32 = 9;

/// The typed TLS configuration; the original layout witness still checks its fields.
pub(super) type TlsFields = swoole_rs_corebase::tls::OwnedTlsFields;
const _: () = assert!(
    size_of::<TlsFields>() == 32
        && offset_of!(TlsFields, allow_self_signed) == 11
        && offset_of!(TlsFields, protocols) == 12
        && offset_of!(TlsFields, context) == 24
);

/// The head of `network::Socket`, through `object` (native/reactor/swoole_rs_socket.h); `info`
/// is at [`SOCKET_INFO`].
#[repr(C)]
pub(super) struct SocketHead {
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
    output_flags: [u8; 4],
    object: *mut c_void,
}
const _: () = assert!(
    offset_of!(SocketHead, removed) == 18
        && offset_of!(SocketHead, chunk_size) == 36
        && offset_of!(SocketHead, object) == 64
);
// `sockaddr_un` is shorter on the BSDs, so the address is Linux's size only there.
#[cfg(target_os = "linux")]
const _: () = assert!(size_of::<super::Address>() == 120);
pub(super) const SOCKET_INFO: usize = 176;

unsafe extern "C-unwind" {
    /// `SwooleG`, whose first member is `max_sockets`.
    #[link_name = "SwooleG"]
    static mut SWOOLE_G_MAX_SOCKETS: u32;
}

unsafe extern "C" {
    fn getsockname(fd: c_int, addr: *mut c_void, len: *mut u32) -> c_int;
}

// The case's functions, which the slots' fns below run: fixture.cc's lambdas captured them.
static START: Mutex<Option<Hook>> = Mutex::new(None);
static WORKER_START: Mutex<Option<Hook>> = Mutex::new(None);
static CONNECT: Mutex<Option<Event>> = Mutex::new(None);
static CLOSE: Mutex<Option<Event>> = Mutex::new(None);
static RECEIVE: Mutex<Option<Receive>> = Mutex::new(None);

fn case<F: Copy>(slot: &Mutex<Option<F>>) -> F {
    slot.lock().unwrap().expect("the case set it")
}

fn on_hook(serv: view::Server<'_>) {
    case(&START)(serv.raw());
}
fn on_worker_hook(serv: view::Server<'_>, _: Worker<'_>) {
    case(&WORKER_START)(serv.raw());
}
fn on_connect(serv: view::Server<'_>, info: &DataHead) {
    case(&CONNECT)(serv.raw(), ptr::from_ref(info).cast());
}
fn on_close(serv: view::Server<'_>, info: &DataHead) {
    case(&CLOSE)(serv.raw(), ptr::from_ref(info).cast());
}
fn on_receive(serv: view::Server<'_>, req: &mut RecvData<'_>) -> c_int {
    case(&RECEIVE)(serv.raw(), ptr::from_ref(req).cast())
}

impl Server {
    fn on_start(self, hook: Hook) {
        *START.lock().unwrap() = Some(hook);
        self.set_callback(Callback::Start(on_hook));
    }
    fn on_worker_start(self, hook: Hook) {
        *WORKER_START.lock().unwrap() = Some(hook);
        self.set_callback(Callback::WorkerStart(on_worker_hook));
    }
    fn on_event(self, slot: Slot, event: Event) {
        match slot {
            Slot::Connect => {
                *CONNECT.lock().unwrap() = Some(event);
                self.set_callback(Callback::Connect(on_connect));
            }
            Slot::Close => {
                *CLOSE.lock().unwrap() = Some(event);
                self.set_callback(Callback::Close(on_close));
            }
            _ => unreachable!("the cases' events are onConnect and onClose"),
        }
    }
    fn on_receive(self, receive: Receive) {
        *RECEIVE.lock().unwrap() = Some(receive);
        self.set_callback(Callback::Receive(status_callback!(on_receive)));
    }

    /// fixture.cc's `listen_port`: the port, or exit 2.
    fn listen_port(self, kind: c_int, port: c_int) -> Port {
        self.add_port(kind, TEST_HOST, port).unwrap_or_else(|| {
            eprintln!("listen failed, [error={}]", last_error());
            std::process::exit(2)
        })
    }
}

impl Port {
    /// `config()->ssl_context`.
    pub(super) fn ssl_context(self) -> *const c_void {
        self.settings()
            .ssl_context
            .get()
            .map_or(std::ptr::null(), |context| context.as_ptr().cast())
    }
}

/// Edits the TLS configuration only while its exclusive loan is held.
pub(super) fn edit_tls(context: *const c_void, edit: impl FnOnce(&mut TlsFields)) {
    // SAFETY: the fixture's port keeps this context live.
    let owner = unsafe { &*context.cast::<swoole_rs_corebase::tls::OwnedTlsContext>() };
    owner.borrow_configuration().edit(edit);
}

/// `SSLContext::set_cert_file` (`CERT_FILE`), `set_key_file` and `set_client_cert_file`,
/// whose results the cases ignore: the warning when the file can't be read.
pub(super) fn set_ssl_path(context: *const c_void, index: u32, file: *const c_char) {
    // SAFETY: a NUL-terminated path.
    let path = unsafe { CStr::from_ptr(file) };
    let which = match index {
        CERT_FILE => swoole_rs_corebase::tls::Text::CertFile,
        KEY_FILE => swoole_rs_corebase::tls::Text::KeyFile,
        CLIENT_CERT_FILE => swoole_rs_corebase::tls::Text::ClientCertFile,
        other => unreachable!("SSL path {other}"),
    };
    // SAFETY: a live context, which only the case's thread configures.
    let context = unsafe { &*context.cast::<swoole_rs_corebase::tls::OwnedTlsContext>() };
    let mut context = context.borrow_configuration();
    // The owner's setter, which warns itself and ends the process on `bad_alloc`.
    _ = context.set_text(which, path.to_bytes());
}

pub(super) fn log_level(level: c_int) {
    swoole_set_log_level(level);
}

fn str_of<'a>(text: *const c_char) -> &'a str {
    // SAFETY: a NUL-terminated string.
    unsafe { CStr::from_ptr(text) }.to_str().unwrap()
}

/// core-tests/src/protocol/base.cpp's Server, created: one worker in base mode, its TCP port
/// framed by set_stream_protocol (0), set_length_protocol(0, 'l', 4) (1) or
/// set_eof_protocol("\r\n", false) (2).
pub(crate) unsafe fn swr_case_protocol_server(
    framing: c_int,
    on_worker_start: Hook,
    on_receive: Receive,
) -> *mut c_void {
    let serv = Server::new(MODE_BASE);
    serv.config().worker_num.set(1);
    let port = serv.listen_port(SW_SOCK_TCP, 0).settings();
    match framing {
        0 => methods::set_stream_protocol(port),
        1 => methods::set_length_protocol(port, 0, c_char::from_ne_bytes(*b"l"), 4),
        _ => methods::set_eof_protocol(port, b"\r\n", false),
    }
    serv.create();
    serv.on_worker_start(on_worker_start);
    serv.on_receive(on_receive);
    serv.0
}

/// core-tests/src/protocol/redis.cpp's redis.server Server, created: one worker in base mode,
/// its TCP port on the redis protocol.
pub(crate) unsafe fn swr_case_redis_server(
    on_worker_start: Hook,
    on_receive: Receive,
) -> *mut c_void {
    let serv = Server::new(MODE_BASE);
    serv.config().worker_num.set(1);
    serv.config().enable_static_handler.set(true);
    log_level(SW_LOG_WARNING);
    serv.listen_port(SW_SOCK_TCP, 0)
        .config()
        .open_redis_protocol
        .set(true);
    serv.create();
    serv.on_worker_start(on_worker_start);
    serv.on_receive(on_receive);
    serv.0
}

/// core-tests/src/server/mqtt.cpp's test_mqtt_server, created: one worker in base mode, port
/// 9501 on the mqtt protocol.
pub(crate) unsafe fn swr_case_mqtt_server(
    on_worker_start: Hook,
    on_connect: Event,
    on_close: Event,
    on_receive: Receive,
) -> *mut c_void {
    let serv = Server::new(MODE_BASE);
    serv.config().worker_num.set(1);
    serv.config().enable_reuse_port.set(true);
    log_level(SW_LOG_WARNING);
    serv.listen_port(SW_SOCK_TCP, 9501)
        .config()
        .open_mqtt_protocol
        .set(true);
    serv.create();
    serv.on_worker_start(on_worker_start);
    serv.on_event(Slot::Close, on_close);
    serv.on_event(Slot::Connect, on_connect);
    serv.on_receive(on_receive);
    serv.0
}

/// Schedule a one-shot owned capture; dropping the timer also drops the capture.
pub(super) fn after_once(ms: c_long, body: impl FnOnce() + 'static) {
    let body = std::cell::Cell::new(Some(body));
    _ = swoole_rs_corebase::timer::add_owned(ms, false, move || {
        if let Some(body) = body.take() {
            body();
        }
    });
}

/// A callback that receives its issued timer ID, including when it cancels itself.
pub(super) fn schedule(
    ms: c_long,
    persistent: bool,
    body: impl Fn(swoole_rs_corebase::timer::Held) + 'static,
) -> Option<swoole_rs_corebase::timer::Held> {
    let slot = std::rc::Rc::new(std::cell::Cell::new(None));
    let issued = std::rc::Rc::clone(&slot);
    let held = swoole_rs_corebase::timer::add_owned(ms, persistent, move || {
        body(
            issued
                .get()
                .expect("the timer was published before selecting it"),
        );
    });
    slot.set(held);
    held
}

/// swoole_timer_tick(ms): `tick(serv, session_id)` until it returns false.
pub(crate) unsafe fn swr_case_tick(
    serv: *mut c_void,
    ms: c_long,
    tick: extern "C" fn(*mut c_void, c_long) -> bool,
    session_id: c_long,
) {
    _ = schedule(ms, true, move |held| {
        if !tick(serv, session_id) {
            swoole_rs_corebase::timer::del_id(held);
        }
    });
}

/// test_base_server's Server, created: null when set_document_root fails.
pub(crate) unsafe fn swr_case_base_server(
    root: *const c_char,
    on_receive: Receive,
    on_close: Event,
    on_worker_start: Hook,
) -> *mut c_void {
    let serv = Server::new(MODE_BASE);
    serv.config().worker_num.set(1);
    serv.config().enable_reuse_port.set(true);
    serv.config().heartbeat_check_interval.set(1);
    serv.config().enable_static_handler.set(true);
    // SAFETY: a NUL-terminated root.
    if !serv.set_document_root(unsafe { CStr::from_ptr(root) }.to_bytes()) {
        return std::ptr::null_mut();
    }
    serv.insert(LOCATIONS, "/examples");
    serv.insert(COMPRESSION_TYPES, "text/html");
    log_level(SW_LOG_WARNING);
    let port = serv.listen_port(SW_SOCK_TCP, 0);
    port.config().open_http_protocol.set(true);
    port.config().open_websocket_protocol.set(true);
    serv.create();
    serv.on_worker_start(on_worker_start);
    serv.on_event(Slot::Close, on_close);
    serv.on_receive(on_receive);
    serv.0
}

/// test_http_server's Server, created.
pub(crate) unsafe fn swr_case_http_server(
    dispatch_mode: c_int,
    ssl: bool,
    worker_num: c_int,
    mode: c_int,
    root: *const c_char,
    on_receive: Receive,
    on_close: Event,
) -> *mut c_void {
    let serv = Server::new(mode);
    serv.set_text(TEXT_USER, "root");
    serv.set_text(TEXT_GROUP, "root");
    serv.set_text(TEXT_CHROOT, "/");
    serv.config()
        .worker_num
        .set(u32::try_from(worker_num).unwrap());
    serv.config()
        .dispatch_mode
        .set(swoole_rs_server::DispatchMode(
            u8::try_from(dispatch_mode).unwrap(),
        ));
    serv.config().open_cpu_affinity.set(true);
    log_level(SW_LOG_WARNING);
    let port = serv.listen_port(
        if ssl {
            SW_SOCK_TCP | SW_SOCK_SSL
        } else {
            SW_SOCK_TCP
        },
        0,
    );
    port.config().open_http_protocol.set(true);
    port.config().open_websocket_protocol.set(true);
    port.config().open_tcp_keepalive.set(true);
    port.config().tcp_fastopen.set(1);
    port.config().tcp_defer_accept.set(1);
    serv.config().enable_static_handler.set(true);
    // SAFETY: a NUL-terminated root.
    serv.set_document_root(unsafe { CStr::from_ptr(root) }.to_bytes());
    serv.insert(LOCATIONS, "/examples");
    serv.create();
    serv.set_callback(Callback::Start(ignore_server));
    serv.set_callback(Callback::Connect(ignore_event));
    serv.on_event(Slot::Close, on_close);
    serv.on_receive(on_receive);
    serv.0
}

/// test_ssl_http's Server on an SSL port: SW_OK from create, or its error.
#[expect(
    clippy::too_many_arguments,
    reason = "fixture.cc's entry point, as main.rs calls it"
)]
pub(crate) unsafe fn swr_case_ssl_server(
    out: *mut *mut c_void,
    mode: c_int,
    port_number: c_int,
    cert: *const c_char,
    key: *const c_char,
    on_start: Hook,
    on_worker_start: Hook,
    on_receive: Receive,
) -> c_int {
    let serv = Server::new(mode);
    // SAFETY: a writable out-parameter.
    unsafe { *out = serv.0 };
    serv.config().worker_num.set(1);
    log_level(SW_LOG_INFO);
    let port = serv.listen_port(SW_SOCK_TCP | SW_SOCK_SSL, port_number);
    port.config().open_http_protocol.set(true);
    set_ssl_path(port.ssl_context(), CERT_FILE, cert);
    set_ssl_path(port.ssl_context(), KEY_FILE, key);
    edit_tls(port.ssl_context(), |fields| fields.http = true);
    let _ = lifecycle::ssl_init(port.settings());
    let created = serv.create();
    serv.on_start(on_start);
    serv.on_worker_start(on_worker_start);
    serv.on_receive(on_receive);
    created
}

/// abort_connection's Server, created: two workers in process mode, SwooleG.max_sockets set
/// to `max_sockets` (the old value in *saved) before set_max_connection(1), then an HTTP port
/// and a TCP port (ports[0], ports[1]; not created when either is null).
pub(crate) unsafe fn swr_case_abort_server(
    max_sockets: u32,
    saved: *mut u32,
    ports: *mut [*mut c_void; 2],
    on_start: Hook,
    on_receive: Receive,
) -> *mut c_void {
    let serv = Server::new(MODE_PROCESS);
    serv.config().worker_num.set(2);
    // SAFETY: SwooleG, which only this thread uses here; writable out-parameters.
    unsafe {
        *saved = SWOOLE_G_MAX_SOCKETS;
        SWOOLE_G_MAX_SOCKETS = max_sockets;
    }
    server_methods::set_max_connection(serv.view(), 1);
    log_level(SW_LOG_WARNING);
    // SAFETY: a writable out-parameter.
    let ports = unsafe { &mut *ports };
    let Some(http) = serv.add_port(SW_SOCK_TCP, TEST_HOST, 0) else {
        return serv.0;
    };
    ports[0] = http.0;
    http.config().open_http_protocol.set(true);
    let Some(tcp) = serv.add_port(SW_SOCK_TCP, TEST_HOST, 0) else {
        return serv.0;
    };
    ports[1] = tcp.0;
    serv.create();
    serv.on_start(on_start);
    serv.on_receive(on_receive);
    serv.0
}

pub(crate) unsafe fn swr_case_set_max_sockets(value: u32) {
    // SAFETY: SwooleG, which only this thread uses here.
    unsafe { SWOOLE_G_MAX_SOCKETS = value };
}

/// get_package_length's port: one worker in base mode, the TCP port speaking HTTP and HTTP/2;
/// the Server not created. Null when add_port fails.
pub(crate) unsafe fn swr_case_length_port(out: *mut *mut c_void) -> *mut c_void {
    let serv = Server::new(MODE_BASE);
    // SAFETY: a writable out-parameter.
    unsafe { *out = serv.0 };
    serv.config().worker_num.set(1);
    serv.add_port(SW_SOCK_TCP, TEST_HOST, 0)
        .map_or(std::ptr::null_mut(), |port| {
            port.config().open_http_protocol.set(true);
            port.config().open_http2_protocol.set(true);
            port.0
        })
}

/// ListenPort::init_protocol, after turning on open_websocket_protocol when `websocket`; the
/// port's Protocol.
pub(crate) unsafe fn swr_case_init_protocol(port: *mut c_void, websocket: bool) -> *const Protocol {
    let port = Port(port);
    if websocket {
        port.config().open_websocket_protocol.set(true);
    }
    methods::init_protocol(port.settings());
    (&raw const port.config().protocol).cast()
}

/// A standalone session with the listen address and ID used in the error assertion.
pub(crate) unsafe fn length_connection(port: *mut c_void) -> ConnectionRecord {
    let port = Port(port);
    let mut conn = ConnectionRecord::default();
    let info = conn.info.get_mut();
    info.len = size_of_val(&info.addr) as u32;
    // SAFETY: info owns the writable address storage and its length.
    unsafe {
        getsockname(
            port.get_fd(),
            (&raw mut info.addr).cast(),
            &raw mut info.len,
        )
    };
    info.kind = port.config().r#type;
    conn.session_id.set(2025);
    conn
}

pub(crate) unsafe fn swr_case_create(serv: *mut c_void) -> c_int {
    Server(serv).create()
}

pub(crate) unsafe fn swr_case_port(serv: *mut c_void) -> c_int {
    Server(serv).get_primary_port().config().port.get()
}

pub(crate) unsafe fn swr_case_start(serv: *mut c_void) -> c_int {
    Server(serv).start()
}

pub(crate) unsafe fn swr_case_shutdown(serv: *mut c_void) -> bool {
    server_methods::shutdown(Server(serv).view())
}

pub(crate) unsafe fn swr_case_master_pid(serv: *mut c_void) -> c_int {
    Server(serv).get_master_pid()
}

pub(crate) unsafe fn swr_case_send(
    serv: *mut c_void,
    session_id: c_long,
    data: *const c_char,
    length: u32,
) -> bool {
    // SAFETY: `data` is readable for `length` bytes.
    Server(serv).send(session_id, unsafe {
        std::slice::from_raw_parts(data.cast(), length as usize)
    })
}

pub(crate) unsafe fn swr_case_close(serv: *mut c_void, session_id: c_long) -> bool {
    server_methods::close(Server(serv).view(), session_id, false)
}

pub(crate) unsafe fn swr_case_sendfile(
    serv: *mut c_void,
    session_id: c_long,
    file: *const c_char,
) -> bool {
    Server(serv).sendfile(session_id, str_of(file))
}

/// `sw_tg_buffer()`: `SwooleTG.buffer_stack`.
pub(crate) fn swr_case_tg_buffer() -> *mut StringBuffer {
    swoole_rs_corebase::thread_buffer().cast()
}

/// The request header fields a callback reads.
pub(crate) unsafe fn swr_case_info(
    info: *const c_void,
    reactor_id: *mut c_int,
    len: *mut u32,
    ext_flags: *mut u16,
) -> c_long {
    // SAFETY: a live header; writable out-parameters.
    unsafe {
        let info = &*info.cast::<DataHead>();
        *reactor_id = c_int::from(info.reactor_id);
        *len = info.len;
        *ext_flags = info.ext_flags;
        info.fd
    }
}

pub(crate) unsafe fn swr_case_connection(
    serv: *mut c_void,
    session_id: c_long,
) -> Connection<'static> {
    Server(serv).get_connection_by_session_id(session_id)
}

pub(crate) unsafe fn swr_case_close_actively(serv: *mut c_void, session_id: c_long) -> bool {
    Server(serv)
        .get_connection_by_session_id(session_id)
        .close_actively()
}

pub(crate) unsafe fn swr_case_healthy(serv: *mut c_void, session_id: c_long) -> bool {
    let serv = Server(serv);
    let now = microtime();
    serv.is_healthy_connection(now, serv.get_connection_by_session_id(session_id))
}

pub(crate) unsafe fn swr_case_idle_worker_num(serv: *mut c_void) -> u32 {
    Server(serv).get_idle_worker_num()
}

pub(crate) unsafe fn swr_case_worker_num(serv: *mut c_void) -> u32 {
    Server(serv).config().worker_num.get()
}

fn close_timeout(serv: Server, session_id: c_long) {
    let reason = "swoole close";
    // SAFETY: this thread's buffer; the reason is readable for its length.
    unsafe {
        let buffer = swr_case_tg_buffer();
        (*buffer).clear();
        swoole_rs_ports::frame_api::pack_close_frame(
            &mut *buffer,
            CLOSE_POLICY_ERROR,
            reason.as_bytes(),
            0,
        );
        swr_case_send(
            serv.0,
            session_id,
            (*buffer).as_ptr().cast(),
            (*buffer).len() as u32,
        );
    }
}

/// The `/ws/close` handshake's timer: a policy close frame after `ms`.
pub(crate) unsafe fn swr_case_close_after(serv: *mut c_void, session_id: c_long, ms: c_long) {
    let serv = Server(serv);
    after_once(ms, move || close_timeout(serv, session_id));
}

// The primary port's TLS settings.
pub(crate) unsafe fn swr_case_ssl_files(
    serv: *mut c_void,
    cert: *const c_char,
    key: *const c_char,
) {
    let context = Server(serv).get_primary_port().ssl_context();
    set_ssl_path(context, CERT_FILE, cert);
    set_ssl_path(context, KEY_FILE, key);
}

pub(crate) unsafe fn swr_case_ssl_sni(
    serv: *mut c_void,
    name: *const c_char,
    cert: *const c_char,
    key: *const c_char,
) {
    let port = Server(serv).get_primary_port();
    let config = port.settings();
    let context = lifecycle::context_dup(config);
    set_ssl_path(context.as_ptr().cast(), CERT_FILE, cert);
    set_ssl_path(context.as_ptr().cast(), KEY_FILE, key);
    // SAFETY: a NUL-terminated name.
    let name = unsafe { CStr::from_ptr(name) };
    lifecycle::sni_add(config, name.to_bytes(), context);
}

/// `set_ssl_protocols`; the cases' ports are TCP, so the DTLS bit stays.
pub(crate) unsafe fn swr_case_ssl_protocols(serv: *mut c_void, protocols: c_long) {
    let context = Server(serv).get_primary_port().ssl_context();
    edit_tls(context, |fields| {
        fields.protocols = u32::try_from(protocols).unwrap();
    });
}

pub(crate) unsafe fn swr_case_ssl_client_ca(serv: *mut c_void, cafile: *const c_char) {
    let context = Server(serv).get_primary_port().ssl_context();
    edit_tls(context, |fields| {
        fields.verify_peer = true;
        fields.allow_self_signed = true;
    });
    // SAFETY: a live context; a NUL-terminated path.
    let status = unsafe {
        let path = CStr::from_ptr(cafile);
        let owner = &*context.cast::<swoole_rs_corebase::tls::OwnedTlsContext>();
        owner
            .borrow_configuration()
            .set_string(CAFILE as usize, path.to_bytes())
    };
    assert!(status, "bad_alloc");
}

pub(crate) unsafe fn swr_case_ssl_init(serv: *mut c_void) -> bool {
    let port = Server(serv).get_primary_port();
    lifecycle::ssl_init(port.settings())
}
