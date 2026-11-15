//! The Servers upstream's `core-tests/src/server/http_server.cpp` builds (`test_base_server`,
//! `test_http_server`, `test_ssl_http`, `abort_connection`'s and `get_package_length`'s),
//! `core-tests/src/protocol/base.cpp` builds, and `protocol/redis.cpp`'s and `server/mqtt.cpp`'s,
//! for main.rs, tests/modules/framing/protocol.rs and tests/modules/protocols/server.rs. They
//! were C++ (fixture.cc) and keep its entry points' names and signatures. A callback slot
//! holds the case's function as its closure context, as fixture.cc's lambdas captured it.

use std::ffi::{CStr, c_char, c_int, c_long, c_void};
use std::mem::{offset_of, transmute};

use super::{
    Address, COMPRESSION_TYPES, Connection, DataHead, LOCATIONS, MODE_BASE, MODE_PROCESS, Port,
    Protocol, RecvData, SW_LOG_WARNING, SW_SOCK_TCP, Server, Slot, TEST_HOST, TEXT_CHROOT,
    TEXT_GROUP, TEXT_USER, ignore_event, ignore_server, last_error, microtime,
    swoole_set_log_level, swr_v1_core_string_set,
};
use crate::{CppString, Event, Hook, Receive};
use swoole_rs_ports::{lifecycle, methods};
use swoole_rs_server::server_methods;

pub(super) const SW_LOG_INFO: c_int = 2;
pub(super) const SW_SOCK_SSL: c_int = 512;
const CLOSE_POLICY_ERROR: c_int = 1008;
// SSLContext's text slots (native/tls/swoole_ssl.h).
pub(super) const CERT_FILE: u32 = 4;
pub(super) const KEY_FILE: u32 = 5;
pub(super) const CLIENT_CERT_FILE: u32 = 7;
const CAFILE: u32 = 9;

/// `swr_v1_tls_owner_fields`.
#[repr(C)]
pub(super) struct TlsFields {
    http: u8,
    http_v2: u8,
    prefer_server_ciphers: u8,
    session_tickets: u8,
    stapling: u8,
    stapling_verify: u8,
    disable_tls_host_name: u8,
    grease: u8,
    verify_depth: u8,
    disable_compress: u8,
    pub(super) verify_peer: u8,
    pub(super) allow_self_signed: u8,
    protocols: u32,
    create_flag: u8,
    context: *mut c_void,
}
const _: () = assert!(
    size_of::<TlsFields>() == 32
        && offset_of!(TlsFields, allow_self_signed) == 11
        && offset_of!(TlsFields, protocols) == 12
        && offset_of!(TlsFields, context) == 24
);

/// `swr_v1_tls_owner_prepared`.
#[repr(C)]
struct TlsView {
    owner: *const c_void,
    fields: *mut TlsFields,
    strings: [*const c_void; 11],
}

/// The head of `TimerNode`, through `destructor` (crates/timer/src/owner.rs).
#[repr(C)]
pub(super) struct TimerNode {
    id: c_long,
    php: bool,
    exec_msec: i64,
    interval: i64,
    exec_count: u64,
    round: u64,
    removed: bool,
    callback: *const c_void,
    pub(super) data: *mut c_void,
    destructor: Option<unsafe extern "C" fn(*mut TimerNode)>,
}
const _: () = assert!(
    offset_of!(TimerNode, exec_msec) == 16
        && offset_of!(TimerNode, removed) == 48
        && offset_of!(TimerNode, destructor) == 72
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
        && size_of::<Address>() == 120
);
pub(super) const SOCKET_INFO: usize = 176;

/// A timer's data: what upstream's lambdas capture.
struct Tick {
    serv: Server,
    tick: Option<extern "C" fn(*mut c_void, c_long) -> bool>,
    session_id: c_long,
}

unsafe extern "C-unwind" {
    fn swr_v1_tls_owner_view(owner: *const c_void) -> TlsView;
    fn swr_v1_core_thread_global() -> *mut c_void;
    #[link_name = "_Z25swoole_get_last_error_msgv"]
    fn swoole_get_last_error_msg() -> *const c_char;
    #[link_name = "_ZN6swoole11make_socketEi8swFdType"]
    pub(super) fn make_socket(fd: c_int, fd_type: c_int) -> *mut SocketHead;
    #[link_name = "_Z16swoole_timer_addlbPFvPN6swoole5TimerEPNS_9TimerNodeEEPv"]
    pub(super) fn swoole_timer_add(
        ms: c_long,
        persistent: bool,
        callback: unsafe extern "C" fn(*mut c_void, *mut TimerNode),
        data: *mut c_void,
    ) -> *mut TimerNode;
    #[link_name = "_Z18swoole_timer_afterlPFvPN6swoole5TimerEPNS_9TimerNodeEEPv"]
    pub(super) fn swoole_timer_after(
        ms: c_long,
        callback: unsafe extern "C" fn(*mut c_void, *mut TimerNode),
        data: *mut c_void,
    ) -> c_long;
    #[link_name = "_Z16swoole_timer_delPN6swoole9TimerNodeE"]
    pub(super) fn swoole_timer_del(node: *mut TimerNode) -> bool;
    /// `SwooleG`, whose first member is `max_sockets`.
    #[link_name = "SwooleG"]
    static mut SWOOLE_G_MAX_SOCKETS: u32;
}

unsafe extern "C" {
    fn getsockname(fd: c_int, addr: *mut c_void, len: *mut u32) -> c_int;
}

// The slots' closures: `ctx` is the case's function.
unsafe extern "C" fn on_hook(ctx: *mut c_void, serv: *mut c_void) {
    // SAFETY: the slot's context is a `Hook`.
    let case = unsafe { transmute::<*mut c_void, Hook>(ctx) };
    case(serv);
}
unsafe extern "C" fn on_worker_hook(ctx: *mut c_void, serv: *mut c_void, _: *mut c_void) {
    // SAFETY: as above.
    let case = unsafe { transmute::<*mut c_void, Hook>(ctx) };
    case(serv);
}
unsafe extern "C" fn on_event(ctx: *mut c_void, serv: *mut c_void, info: *mut c_void) {
    // SAFETY: the slot's context is an `Event`.
    let case = unsafe { transmute::<*mut c_void, Event>(ctx) };
    case(serv, info);
}
unsafe extern "C" fn on_receive(ctx: *mut c_void, serv: *mut c_void, req: *mut c_void) -> c_int {
    // SAFETY: the slot's context is a `Receive`.
    let case = unsafe { transmute::<*mut c_void, Receive>(ctx) };
    case(serv, req)
}

impl Server {
    fn on_start(self, hook: Hook) {
        self.set_closure(Slot::Start, on_hook as *const c_void, hook as *mut c_void);
    }
    fn on_worker_start(self, hook: Hook) {
        self.set_closure(
            Slot::WorkerStart,
            on_worker_hook as *const c_void,
            hook as *mut c_void,
        );
    }
    fn on_event(self, slot: Slot, event: Event) {
        self.set_closure(slot, on_event as *const c_void, event as *mut c_void);
    }
    fn on_receive(self, receive: Receive) {
        self.set_closure(
            Slot::Receive,
            on_receive as *const c_void,
            receive as *mut c_void,
        );
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

/// `SSLContext::fields()`.
pub(super) fn tls_fields(context: *const c_void) -> &'static mut TlsFields {
    // SAFETY: a live context, whose fields live as long as it.
    unsafe { &mut *swr_v1_tls_owner_view(context).fields }
}

/// `SSLContext::set_cert_file` (`CERT_FILE`), `set_key_file` and `set_client_cert_file`,
/// whose results the cases ignore: the warning when the file can't be read.
pub(super) fn set_ssl_path(context: *const c_void, index: u32, file: *const c_char) {
    // SAFETY: a NUL-terminated path.
    let path = unsafe { CStr::from_ptr(file) };
    let which = match index {
        CERT_FILE => swoole_rs_tls::Text::CertFile,
        KEY_FILE => swoole_rs_tls::Text::KeyFile,
        CLIENT_CERT_FILE => swoole_rs_tls::Text::ClientCertFile,
        other => unreachable!("SSL path {other}"),
    };
    // SAFETY: a live context, which only the case's thread configures.
    let context = unsafe { swoole_rs_tls::Lent::new(&*context.cast()) };
    // The owner's setter, which warns itself and ends the process on `bad_alloc`.
    _ = context.set_text(which, path.to_bytes());
}

pub(super) fn log_level(level: c_int) {
    // SAFETY: a plain call.
    unsafe { swoole_set_log_level(level) };
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
    serv.config().worker_num = 1;
    let port = serv.listen_port(SW_SOCK_TCP, 0).settings();
    match framing {
        0 => methods::set_stream_protocol(port),
        1 => methods::set_length_protocol(port, 0, b'l' as c_char, 4),
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
    serv.config().worker_num = 1;
    serv.config().enable_static_handler = true;
    log_level(SW_LOG_WARNING);
    serv.listen_port(SW_SOCK_TCP, 0)
        .config()
        .open_redis_protocol = true;
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
    serv.config().worker_num = 1;
    serv.config().enable_reuse_port = true;
    log_level(SW_LOG_WARNING);
    serv.listen_port(SW_SOCK_TCP, 9501)
        .config()
        .open_mqtt_protocol = true;
    serv.create();
    serv.on_worker_start(on_worker_start);
    serv.on_event(Slot::Close, on_close);
    serv.on_event(Slot::Connect, on_connect);
    serv.on_receive(on_receive);
    serv.0
}

unsafe extern "C" fn tick_free(node: *mut TimerNode) {
    // SAFETY: the node's `Tick`, freed once, with the node.
    drop(unsafe { Box::from_raw((*node).data.cast::<Tick>()) });
}

unsafe extern "C" fn on_tick(_: *mut c_void, node: *mut TimerNode) {
    // SAFETY: the live node of `swr_case_tick`, whose data is its `Tick`.
    let tick = unsafe { &*(*node).data.cast::<Tick>() };
    if !(tick.tick.unwrap())(tick.serv.0, tick.session_id) {
        // SAFETY: as above.
        unsafe { swoole_timer_del(node) };
    }
}

/// swoole_timer_tick(ms): `tick(serv, session_id)` until it returns false.
pub(crate) unsafe fn swr_case_tick(
    serv: *mut c_void,
    ms: c_long,
    tick: extern "C" fn(*mut c_void, c_long) -> bool,
    session_id: c_long,
) {
    let data = Box::into_raw(Box::new(Tick {
        serv: Server(serv),
        tick: Some(tick),
        session_id,
    }));
    // SAFETY: a callback of the timer's signature; the node takes `data`.
    unsafe {
        let node = swoole_timer_add(ms, true, on_tick, data.cast());
        (*node).destructor = Some(tick_free);
    }
}

/// test_base_server's Server, created: null when set_document_root fails.
pub(crate) unsafe fn swr_case_base_server(
    root: *const c_char,
    on_receive: Receive,
    on_close: Event,
    on_worker_start: Hook,
) -> *mut c_void {
    let serv = Server::new(MODE_BASE);
    serv.config().worker_num = 1;
    serv.config().enable_reuse_port = true;
    serv.config().heartbeat_check_interval = 1;
    serv.config().enable_static_handler = true;
    // SAFETY: a NUL-terminated root.
    if !serv.set_document_root(unsafe { CStr::from_ptr(root) }.to_bytes()) {
        return std::ptr::null_mut();
    }
    serv.insert(LOCATIONS, "/examples");
    serv.insert(COMPRESSION_TYPES, "text/html");
    log_level(SW_LOG_WARNING);
    let port = serv.listen_port(SW_SOCK_TCP, 0);
    port.config().open_http_protocol = true;
    port.config().open_websocket_protocol = true;
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
    serv.config().worker_num = u32::try_from(worker_num).unwrap();
    serv.config().dispatch_mode = u8::try_from(dispatch_mode).unwrap();
    serv.config().open_cpu_affinity = true;
    log_level(SW_LOG_WARNING);
    let port = serv.listen_port(
        if ssl {
            SW_SOCK_TCP | SW_SOCK_SSL
        } else {
            SW_SOCK_TCP
        },
        0,
    );
    port.config().open_http_protocol = true;
    port.config().open_websocket_protocol = true;
    port.config().open_tcp_keepalive = true;
    port.config().tcp_fastopen = 1;
    port.config().tcp_defer_accept = 1;
    serv.config().enable_static_handler = true;
    // SAFETY: a NUL-terminated root.
    serv.set_document_root(unsafe { CStr::from_ptr(root) }.to_bytes());
    serv.insert(LOCATIONS, "/examples");
    serv.create();
    serv.set_callback(Slot::Start, ignore_server as *const c_void);
    serv.set_callback(Slot::Connect, ignore_event as *const c_void);
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
    serv.config().worker_num = 1;
    log_level(SW_LOG_INFO);
    let port = serv.listen_port(SW_SOCK_TCP | SW_SOCK_SSL, port_number);
    port.config().open_http_protocol = true;
    set_ssl_path(port.ssl_context(), CERT_FILE, cert);
    set_ssl_path(port.ssl_context(), KEY_FILE, key);
    tls_fields(port.ssl_context()).http = 1;
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
    serv.config().worker_num = 2;
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
    http.config().open_http_protocol = true;
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
    serv.config().worker_num = 1;
    serv.add_port(SW_SOCK_TCP, TEST_HOST, 0)
        .map_or(std::ptr::null_mut(), |port| {
            port.config().open_http_protocol = true;
            port.config().open_http2_protocol = true;
            port.0
        })
}

/// ListenPort::init_protocol, after turning on open_websocket_protocol when `websocket`; the
/// port's Protocol.
pub(crate) unsafe fn swr_case_init_protocol(port: *mut c_void, websocket: bool) -> *const Protocol {
    let port = Port(port);
    if websocket {
        port.config().open_websocket_protocol = true;
    }
    methods::init_protocol(port.settings());
    &raw const port.config().protocol
}

/// get_package_length's fake connection socket on the port's fd, its object a Connection of
/// session 2025. Both leak, as upstream's do.
pub(crate) unsafe fn swr_case_fake_socket(port: *mut c_void, out: *mut *mut c_void) -> *mut c_void {
    let config = Port(port).config();
    // SAFETY: `get_fd()`: the port's socket record, if it has one.
    let fd = unsafe { config.socket.cast::<SocketHead>().as_ref() }.map_or(-1, |socket| socket.fd);
    // SAFETY: zeroed is `new Connection()`'s value.
    let conn = Box::into_raw(Box::new(unsafe { std::mem::zeroed::<Connection>() }));
    // SAFETY: a new record, as `new network::Socket()`, whose fields this sets.
    unsafe {
        let sock = make_socket(fd, 0);
        (*sock).removed = 0;
        (*sock).object = conn.cast();
        (*sock).socket_type = config.type_;
        let info = &mut *sock.cast::<u8>().add(SOCKET_INFO).cast::<Address>();
        info.len = size_of_val(&info.addr) as u32;
        getsockname(fd, (&raw mut info.addr).cast(), &raw mut info.len);
        info.kind = config.type_;
        (*conn).info = *info;
        (*conn).session_id = 2025;
        *out = conn.cast();
        sock.cast()
    }
}

pub(crate) unsafe fn swr_case_connection_status(
    conn: *mut c_void,
    websocket_status: u8,
    http2_stream: u8,
) {
    // SAFETY: the fake socket's live Connection.
    let conn = unsafe { &mut *conn.cast::<Connection>() };
    conn.websocket_status = websocket_status;
    conn.http2_stream = http2_stream;
}

pub(crate) unsafe fn swr_case_create(serv: *mut c_void) -> c_int {
    Server(serv).create()
}

pub(crate) unsafe fn swr_case_last_error(msg: *mut *const c_char) -> c_int {
    // SAFETY: a writable out-parameter; the thread's message.
    unsafe { *msg = swoole_get_last_error_msg() };
    last_error()
}

pub(crate) unsafe fn swr_case_port(serv: *mut c_void) -> c_int {
    Server(serv).get_primary_port().config().port
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

/// `sw_tg_buffer()`: `SwooleTG.buffer_stack`, the thread global's second pointer.
pub(crate) unsafe fn swr_case_tg_buffer() -> *mut CppString {
    // SAFETY: this thread's live global.
    unsafe { *swr_v1_core_thread_global().cast::<*mut CppString>().add(1) }
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

pub(crate) unsafe fn swr_case_data(req: *const c_void) -> *const c_char {
    // SAFETY: a live request.
    unsafe { (*req.cast::<RecvData>()).data }
}

pub(crate) unsafe fn swr_case_websocket_status(serv: *mut c_void, session_id: c_long) -> *mut u8 {
    &raw mut Server(serv)
        .get_connection_by_session_id(session_id)
        .websocket_status
}

pub(crate) unsafe fn swr_case_close_actively(serv: *mut c_void, session_id: c_long) -> bool {
    Server(serv)
        .get_connection_by_session_id(session_id)
        .close_actively
        != 0
}

pub(crate) unsafe fn swr_case_healthy(serv: *mut c_void, session_id: c_long) -> bool {
    let serv = Server(serv);
    // SAFETY: a plain call.
    let now = unsafe { microtime() };
    serv.is_healthy_connection(now, serv.get_connection_by_session_id(session_id))
}

pub(crate) unsafe fn swr_case_idle_worker_num(serv: *mut c_void) -> u32 {
    Server(serv).get_idle_worker_num()
}

pub(crate) unsafe fn swr_case_worker_num(serv: *mut c_void) -> u32 {
    Server(serv).config().worker_num
}

unsafe extern "C" fn close_timeout(_: *mut c_void, node: *mut TimerNode) {
    // SAFETY: `swr_case_close_after`'s `Tick`, which only this callback frees.
    let tick = unsafe { Box::from_raw((*node).data.cast::<Tick>()) };
    let reason = "swoole close";
    // SAFETY: this thread's buffer; the reason is readable for its length.
    unsafe {
        let buffer = swr_case_tg_buffer();
        (*buffer).length = 0;
        (*buffer).offset = 0;
        swoole_rs_ffi::frame_api::pack_close_frame(
            &mut *buffer.cast(),
            CLOSE_POLICY_ERROR,
            reason.as_bytes(),
            0,
        );
        swr_case_send(
            tick.serv.0,
            tick.session_id,
            (*buffer).str,
            (*buffer).length as u32,
        );
    }
}

/// The `/ws/close` handshake's timer: a policy close frame after `ms`.
pub(crate) unsafe fn swr_case_close_after(serv: *mut c_void, session_id: c_long, ms: c_long) {
    let data = Box::into_raw(Box::new(Tick {
        serv: Server(serv),
        tick: None,
        session_id,
    }));
    // SAFETY: a callback of the timer's signature, which takes `data`.
    unsafe { swoole_timer_after(ms, close_timeout, data.cast()) };
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
    tls_fields(context).protocols = u32::try_from(protocols).unwrap();
}

pub(crate) unsafe fn swr_case_ssl_client_ca(serv: *mut c_void, cafile: *const c_char) {
    let context = Server(serv).get_primary_port().ssl_context();
    let fields = tls_fields(context);
    fields.verify_peer = 1;
    fields.allow_self_signed = 1;
    // SAFETY: a live context's slot; a NUL-terminated path.
    let status = unsafe {
        let path = CStr::from_ptr(cafile);
        let slot = swr_v1_tls_owner_view(context).strings[CAFILE as usize];
        swr_v1_core_string_set(slot, path.as_ptr(), path.count_bytes())
    };
    assert_eq!(status, 0, "bad_alloc");
}

pub(crate) unsafe fn swr_case_ssl_init(serv: *mut c_void) -> bool {
    let port = Server(serv).get_primary_port();
    lifecycle::ssl_init(port.settings())
}
