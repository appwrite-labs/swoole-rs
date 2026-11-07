//! The `ListenPort` methods of `src/server/port.cc` that were one lifecycle run
//! or one readable callback, defined under their upstream (Itanium) symbols when
//! the hybrid links this crate (`upstream-symbols`). port.cc registers its two
//! runners once; each method calls one as its C++ forward did.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr;
use std::sync::OnceLock;

use swoole_rs_clients::address::{
    swr_v1_clients_address_assign, swr_v1_clients_address_port, swr_v1_clients_address_type,
};
use swoole_rs_corebase::constants::SW_DATA_EOF_MAXLEN;
use swoole_rs_corebase::net_types::SocketAddress;
use swoole_rs_corebase::{log, swr_v1_clients_gethostbyname};
use swoole_rs_reactor::socket_upstream::make_socket_typed;
use swoole_rs_reactor::{
    SocketRecord, swr_v1_socket_bind, swr_v1_socket_dealloc, swr_v1_socket_get_name,
    swr_v1_socket_listen,
};
#[cfg(not(test))]
use swoole_rs_tls::upstream::socket_free;

/// `Socket::free()` for a socket no loop, timer or buffer has seen: tests link no
/// C++, which `socket_free` reaches under `upstream-symbols`.
#[cfg(test)]
unsafe fn socket_free(socket: *mut SocketRecord) {
    // SAFETY: the caller's live record.
    let fd = unsafe { (*socket).fd };
    // SAFETY: close(2) has no memory preconditions.
    unsafe { libc::close(fd) };
    // SAFETY: Rust allocated the record and nothing uses it after.
    unsafe { swr_v1_socket_dealloc(socket) };
}

/// `SW_FD_STREAM_SERVER` and `SW_FD_DGRAM_SERVER`.
const FD_STREAM_SERVER: c_int = 1;
const FD_DGRAM_SERVER: c_int = 2;
/// `SW_SOCK_NONBLOCK` and `SW_SOCK_CLOEXEC`.
const SOCK_NONBLOCK: c_int = 1 << 2;
const SOCK_CLOEXEC: c_int = 1 << 3;
use swoole_rs_server::port_config::{self, PortConfig};
use swoole_rs_server::view;

use swoole_rs_http::swr_v1_http_request_free;
use swoole_rs_server::ConnectionRecord;

use crate::lifecycle::{
    RUN_CLOSE, RUN_CONTEXT_CREATE, RUN_CONTEXT_INIT, RUN_IMPORT, RUN_SNI, RUN_SSL_CREATE,
    RUN_SSL_INIT,
};
use crate::read::{ON_READ_EOF, ON_READ_HTTP, ON_READ_LENGTH, ON_READ_RAW, ON_READ_REDIS};

/// port.cc's lifecycle runner: kind, port, target context, socket, SSL, and the
/// fd or `allow_empty_cert`.
pub type NativeRun = unsafe extern "C-unwind" fn(
    u32,
    *mut c_void,
    *mut c_void,
    *mut c_void,
    *mut c_void,
    c_int,
) -> i32;
/// port.cc's `port_read`: reactor, port, event and readable kind.
pub type NativeRead =
    unsafe extern "C-unwind" fn(*mut c_void, *mut c_void, *mut c_void, u32) -> i32;
/// A port fact native code owns: [`FACT_REUSE_PORT`] or [`FACT_DTLS`].
pub type NativeFact = unsafe extern "C-unwind" fn(*mut c_void, u32) -> bool;

/// The port's server's `enable_reuse_port`.
pub const FACT_REUSE_PORT: u32 = 0;
/// `ListenPort::is_dtls()`.
pub const FACT_DTLS: u32 = 1;

static NATIVE: OnceLock<(NativeRun, NativeRead, NativeFact)> = OnceLock::new();

/// Registers port.cc's runners, which the methods below call.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_ports_set_native(run: NativeRun, read: NativeRead, fact: NativeFact) {
    // A second registration is the same triple; keeping the first is equivalent.
    NATIVE.get_or_init(|| (run, read, fact));
}

fn native() -> (NativeRun, NativeRead, NativeFact) {
    match NATIVE.get() {
        Some(native) => *native,
        // port.cc registers before any ListenPort exists.
        None => std::process::abort(),
    }
}

/// A native fact of `port`.
///
/// # Safety
/// `port` is a live `ListenPort`.
unsafe fn fact(port: *mut c_void, which: u32) -> bool {
    // SAFETY: as the caller guarantees.
    unsafe { (native().2)(port, which) }
}

/// One lifecycle `kind` through port.cc's runner.
///
/// # Safety
/// `port` is null only for `RUN_SNI`, else a live `ListenPort`; the rest are what `kind` reads.
unsafe fn run(
    kind: u32,
    port: *mut c_void,
    target: *mut c_void,
    sock: *mut c_void,
    ssl: *mut c_void,
    number: c_int,
) -> i32 {
    // SAFETY: as the caller guarantees.
    unsafe { (native().0)(kind, port, target, sock, ssl, number) }
}

/// `ListenPort::ssl_context_init()`.
///
/// # Safety
/// `port` is a live `ListenPort`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole10ListenPort16ssl_context_initEv")
)]
pub unsafe extern "C" fn ssl_context_init(port: *mut c_void) -> bool {
    let null = ptr::null_mut();
    // SAFETY: as the caller guarantees.
    unsafe { run(RUN_CONTEXT_INIT, port, null, null, null, 0) != 0 }
}

/// `ListenPort::ssl_create(sock)`.
///
/// # Safety
/// `port` is a live `ListenPort` and `sock` a socket it accepted.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole10ListenPort10ssl_createEPNS_7network6SocketE")
)]
pub unsafe extern "C" fn ssl_create(port: *mut c_void, sock: *mut c_void) -> bool {
    let null = ptr::null_mut();
    // SAFETY: as the caller guarantees.
    unsafe { run(RUN_SSL_CREATE, port, null, sock, null, 0) != 0 }
}

/// `ListenPort::ssl_context_create(context, allow_empty_cert)`.
///
/// # Safety
/// `port` is a live `ListenPort` and `context` a live `SSLContext`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole10ListenPort18ssl_context_createEPNS_10SSLContextEb")
)]
pub unsafe extern "C" fn ssl_context_create(
    port: *const c_void,
    context: *mut c_void,
    allow_empty_cert: bool,
) -> bool {
    let null = ptr::null_mut();
    let allow = c_int::from(allow_empty_cert);
    // SAFETY: as the caller guarantees.
    unsafe {
        run(
            RUN_CONTEXT_CREATE,
            port.cast_mut(),
            context,
            null,
            null,
            allow,
        ) != 0
    }
}

/// `ListenPort::import(fd)`.
///
/// # Safety
/// `port` is a live `ListenPort`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole10ListenPort6importEi")
)]
pub unsafe extern "C" fn import(port: *mut c_void, fd: c_int) -> bool {
    let null = ptr::null_mut();
    // SAFETY: as the caller guarantees.
    unsafe { run(RUN_IMPORT, port, null, null, null, fd) != 0 }
}

/// `ListenPort::close()`.
///
/// # Safety
/// `port` is a live `ListenPort`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole10ListenPort5closeEv")
)]
pub unsafe extern "C" fn close(port: *mut c_void) {
    let null = ptr::null_mut();
    // SAFETY: as the caller guarantees.
    unsafe { run(RUN_CLOSE, port, null, null, null, 0) };
}

/// `ListenPort::ssl_server_sni_callback(ssl, al, arg)`, OpenSSL's servername callback.
///
/// # Safety
/// `ssl` is the handshake's live `SSL`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole10ListenPort23ssl_server_sni_callbackEP6ssl_stPiPv")
)]
pub unsafe extern "C" fn ssl_server_sni_callback(
    ssl: *mut c_void,
    _al: *mut c_int,
    _arg: *mut c_void,
) -> c_int {
    let null = ptr::null_mut();
    // SAFETY: as the caller guarantees; the SNI run finds the port from `ssl`.
    unsafe { run(RUN_SNI, null, null, null, ssl, 0) }
}

/// The listening socket's options `ListenPort::listen` sets where the platform
/// defines them: (level, option, its name, the value) for each enabled one.
fn listen_options(config: &PortConfig) -> Vec<(c_int, c_int, &'static str, c_int)> {
    let mut options = Vec::new();
    #[cfg(target_os = "linux")]
    if config.tcp_defer_accept != 0 {
        let defer = config.tcp_defer_accept;
        options.push((
            libc::IPPROTO_TCP,
            libc::TCP_DEFER_ACCEPT,
            "TCP_DEFER_ACCEPT",
            defer,
        ));
    }
    if config.tcp_fastopen != 0 {
        let fastopen = config.tcp_fastopen;
        options.push((
            libc::IPPROTO_TCP,
            libc::TCP_FASTOPEN,
            "TCP_FASTOPEN",
            fastopen,
        ));
    }
    if config.open_tcp_keepalive {
        options.push((libc::SOL_SOCKET, libc::SO_KEEPALIVE, "SO_KEEPALIVE", 1));
        #[cfg(target_os = "linux")]
        options.extend([
            (
                libc::IPPROTO_TCP,
                libc::TCP_KEEPIDLE,
                "TCP_KEEPIDLE",
                config.tcp_keepidle,
            ),
            (
                libc::IPPROTO_TCP,
                libc::TCP_KEEPINTVL,
                "TCP_KEEPINTVL",
                config.tcp_keepinterval,
            ),
            (
                libc::IPPROTO_TCP,
                libc::TCP_KEEPCNT,
                "TCP_KEEPCNT",
                config.tcp_keepcount,
            ),
        ]);
        #[cfg(target_os = "linux")]
        if config.tcp_user_timeout > 0 {
            let timeout = config.tcp_user_timeout;
            options.push((
                libc::IPPROTO_TCP,
                libc::TCP_USER_TIMEOUT,
                "TCP_USER_TIMEOUT",
                timeout,
            ));
        }
    }
    options
}

/// `ListenPort::listen()`: listens unless already listening, then sets the
/// socket options and the default high watermark. A failed option only warns.
///
/// # Safety
/// `port` is a live `ListenPort` with a live socket.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole10ListenPort6listenEv")
)]
pub unsafe extern "C" fn listen(port: *mut c_void) -> c_int {
    // SAFETY: as the caller guarantees.
    let config = unsafe { port_config::of(port) };
    let socket = config.socket.cast::<SocketRecord>();
    // SAFETY: the port's live socket, as the caller guarantees.
    if !config.listening && unsafe { swr_v1_socket_listen(socket, c_int::from(config.backlog)) } < 0
    {
        log::sys_warning(
            c"ListenPort::listen",
            format_args!(
                "listen({}:{}, {}) failed",
                host_text(config),
                config.port,
                config.backlog
            ),
        );
        return -1;
    }
    config.listening = true;
    // SAFETY: as above.
    let fd = unsafe { (*socket).fd };
    let size = libc::socklen_t::try_from(size_of::<c_int>()).unwrap_or_default();
    for (level, option, name, value) in listen_options(config) {
        // SAFETY: `value` is a live c_int of `size` bytes.
        if unsafe { libc::setsockopt(fd, level, option, (&raw const value).cast(), size) } != 0 {
            log::sys_warning(
                c"ListenPort::listen",
                format_args!("setsockopt({name}) failed"),
            );
        }
    }
    if config.buffer_high_watermark == 0 {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "upstream's implicit double-to-uint32_t conversion of `socket_buffer_size * 0.8`"
        )]
        let watermark = (f64::from(config.socket_buffer_size) * 0.8) as u32;
        config.buffer_high_watermark = watermark;
    }
    0
}

/// The host as `host.c_str()` prints it.
fn host_text(config: &PortConfig) -> std::borrow::Cow<'_, str> {
    // SAFETY: a slot's view is NUL-terminated and lives while `config` is borrowed.
    unsafe { CStr::from_ptr(config.host().data) }.to_string_lossy()
}

/// An int option set to 1; 0 or -1 as setsockopt(2).
fn set_option(fd: c_int, level: c_int, option: c_int) -> c_int {
    let one: c_int = 1;
    let size = libc::socklen_t::try_from(size_of::<c_int>()).unwrap_or_default();
    // SAFETY: `one` is a live c_int of `size` bytes.
    unsafe { libc::setsockopt(fd, level, option, (&raw const one).cast(), size) }
}

/// `ListenPort::create_socket()`: keeps the port's socket unless the server
/// reuses ports (Linux), else makes one, sets its options, binds it to the
/// port's host and port, and takes the bound port. On failure it sets the last
/// error to errno and frees the new socket, which the port still points at.
///
/// # Safety
/// `port` is a live `ListenPort` of a live server.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole10ListenPort13create_socketEv")
)]
pub unsafe extern "C" fn create_socket(port: *mut c_void) -> c_int {
    // SAFETY: as the caller guarantees.
    let reuse_port = || cfg!(target_os = "linux") && unsafe { fact(port, FACT_REUSE_PORT) };
    // SAFETY: as the caller guarantees.
    let config = unsafe { port_config::of(port) };
    if !config.socket.is_null() {
        if !reuse_port() {
            return 0;
        }
        // SAFETY: the port's live socket, which nothing else uses now.
        unsafe { close_socket(port) };
    }
    let kind = config.r#type;
    let fd_type = if matches!(kind, 2 | 4 | 6) {
        FD_DGRAM_SERVER
    } else {
        FD_STREAM_SERVER
    };
    let socket = make_socket_typed(kind, fd_type, SOCK_NONBLOCK | SOCK_CLOEXEC);
    config.socket = socket.cast();
    if socket.is_null() {
        log::set_last_error(errno());
        return -1;
    }
    // SAFETY: the live record just made.
    let fd = unsafe { (*socket).fd };
    let kqueue = cfg!(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    ));
    // SAFETY: as the caller guarantees.
    if kqueue && unsafe { fact(port, FACT_DTLS) } {
        set_option(fd, libc::SOL_SOCKET, libc::SO_REUSEPORT);
    }
    let fail = || {
        log::set_last_error(errno());
        // SAFETY: the record just made, freed once, on this failure path.
        unsafe { socket_free(socket) };
        -1
    };
    if reuse_port() && set_option(fd, libc::SOL_SOCKET, libc::SO_REUSEPORT) < 0 {
        return fail();
    }
    let host = config.host();
    let mut address = SocketAddress {
        // SAFETY: the sockaddr union is plain data.
        addr: unsafe { std::mem::zeroed() },
        len: 0,
        kind,
    };
    // SAFETY: the union and its length, and the host's NUL-terminated bytes.
    let error = unsafe {
        swr_v1_clients_address_assign(
            (&raw mut address.addr).cast(),
            &raw mut address.len,
            kind,
            host.data,
            host.length,
            config.port,
            Some(swr_v1_clients_gethostbyname),
        )
    };
    if error != 0 {
        log::set_last_error(error);
        // SAFETY: a static NUL-terminated name.
        let name = unsafe { CStr::from_ptr(swr_v1_clients_address_type(kind)) };
        log::warning(
            c"ListenPort::create_socket",
            format_args!(
                "Invalid {} address '{}:{}'",
                name.to_string_lossy(),
                host_text(config),
                config.port
            ),
        );
        return fail();
    }
    if set_option(fd, libc::SOL_SOCKET, libc::SO_REUSEADDR) < 0 {
        log::sys_warning(
            c"ListenPort::create_socket",
            format_args!("setsockopt({fd}, SO_REUSEADDR) failed"),
        );
    }
    // SAFETY: the live record and the address assigned above.
    let bound =
        unsafe { swr_v1_socket_bind(socket, (&raw const address.addr).cast(), address.len) };
    // SAFETY: the live record.
    if bound < 0 || unsafe { swr_v1_socket_get_name(socket) } < 0 {
        return fail();
    }
    // SAFETY: the live record.
    let info = unsafe { &raw const (*socket).info };
    // SAFETY: get_name filled the record's address.
    config.port = unsafe { swr_v1_clients_address_port(info) };
    0
}

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// `ListenPort::close_socket()`: closes and frees the port's socket.
///
/// # Safety
/// `port` is a live `ListenPort` with a live socket nothing else uses.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole10ListenPort12close_socketEv")
)]
pub unsafe extern "C" fn close_socket(port: *mut c_void) {
    // SAFETY: as the caller guarantees.
    let config = unsafe { port_config::of(port) };
    let socket = config.socket.cast::<SocketRecord>();
    // SAFETY: as the caller guarantees.
    let fd = unsafe { (*socket).fd };
    // SAFETY: close(2) has no memory preconditions.
    if unsafe { libc::close(fd) } < 0 {
        log::sys_warning(
            c"ListenPort::close_socket",
            format_args!("close({fd}) failed"),
        );
    }
    // SAFETY: `delete socket`: Rust allocated the record and nothing uses it after.
    unsafe { swr_v1_socket_dealloc(socket) };
    config.socket = ptr::null_mut();
}

/// The `SWR_PORTS_*` protocol flags a port's settings turn on; `SWR_PORTS_DGRAM`
/// too for a datagram port.
fn protocol_flags(config: &PortConfig) -> u32 {
    let datagram = matches!(config.r#type, 2 | 4 | 6);
    [
        (datagram, 1),
        (config.open_eof_check, 4),
        (config.open_length_check, 8),
        (config.open_http_protocol, 16),
        (config.open_http2_protocol, 32),
        (config.open_websocket_protocol, 64),
        (config.open_mqtt_protocol, 128),
        (config.open_redis_protocol, 256),
    ]
    .into_iter()
    .filter(|&(on, _)| on)
    .fold(0, |flags, (_, flag)| flags | flag)
}

/// `ListenPort::get_protocols()`.
///
/// # Safety
/// `port` is a live `ListenPort`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole10ListenPort13get_protocolsEv")
)]
pub unsafe extern "C" fn get_protocols(port: *mut c_void) -> *const c_char {
    // SAFETY: as the caller guarantees.
    crate::swr_v1_ports_protocol_name(protocol_flags(unsafe { view::port(port) }))
}

/// `ListenPort::clear_protocol()`.
///
/// # Safety
/// `port` is a live `ListenPort` whose settings nothing else touches meanwhile.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole10ListenPort14clear_protocolEv")
)]
pub unsafe extern "C" fn clear_protocol(port: *mut c_void) {
    // SAFETY: as the caller guarantees.
    let config = unsafe { port_config::of(port) };
    config.open_eof_check = false;
    config.open_length_check = false;
    config.open_http_protocol = false;
    config.open_websocket_protocol = false;
    config.open_http2_protocol = false;
    config.open_mqtt_protocol = false;
    config.open_redis_protocol = false;
}

/// The C++ functions a port's protocol points at.
#[derive(Clone, Copy)]
enum Callback {
    DispatchTask,
    HttpDispatchFrame,
    WebsocketDispatchFrame,
    DefaultLength,
    HttpLength,
    HttpLengthSize,
    Http2Length,
    WebsocketLength,
}

/// The address of `callback`.
#[cfg(not(test))]
fn callback(callback: Callback) -> *mut c_void {
    use crate::http_routes::exports as http;
    unsafe extern "C" {
        #[link_name = "_ZN6swoole8Protocol19default_length_funcEPKS0_PNS_7network6SocketEPNS_12PacketLengthE"]
        fn default_length();
    }
    match callback {
        Callback::DispatchTask => http::dispatch_task as *mut c_void,
        Callback::HttpDispatchFrame => http::dispatch_frame as *mut c_void,
        Callback::WebsocketDispatchFrame => http::websocket_dispatch_frame as *mut c_void,
        Callback::DefaultLength => default_length as *mut c_void,
        Callback::HttpLength => http::get_package_length as *mut c_void,
        Callback::HttpLengthSize => http::get_package_length_size as *mut c_void,
        Callback::Http2Length => http::http2_length as *mut c_void,
        Callback::WebsocketLength => http::websocket_length as *mut c_void,
    }
}

/// Tests link no C++: each callback is a distinct non-null address.
#[cfg(test)]
fn callback(callback: Callback) -> *mut c_void {
    ptr::without_provenance_mut(0x1000 + callback as usize)
}

/// A readable callback's address.
fn reader(
    function: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> c_int,
) -> *mut c_void {
    function as *mut c_void
}

/// `init_protocol` over the port's settings; `dtls` is `is_dtls()`, asked of a
/// datagram port only.
fn init_protocol_over(config: &mut PortConfig, dtls: impl FnOnce() -> bool) {
    let datagram = matches!(config.r#type, 2 | 4 | 6);
    if datagram && !dtls() {
        return;
    }
    let protocol = &mut config.protocol;
    config.on_read = if config.open_eof_check {
        if u32::from(protocol.package_eof_len) > SW_DATA_EOF_MAXLEN {
            #[expect(clippy::cast_possible_truncation, reason = "SW_DATA_EOF_MAXLEN is 8")]
            let max = SW_DATA_EOF_MAXLEN as u8;
            protocol.package_eof_len = max;
        }
        protocol.on_package = callback(Callback::DispatchTask);
        reader(readable_callback_eof)
    } else if config.open_length_check {
        if protocol.package_length_type != 0 {
            protocol.get_package_length = callback(Callback::DefaultLength);
        }
        protocol.on_package = callback(Callback::DispatchTask);
        reader(readable_callback_length)
    } else if config.open_http_protocol {
        if config.open_http2_protocol && config.open_websocket_protocol {
            protocol.get_package_length = callback(Callback::HttpLength);
            protocol.get_package_length_size = callback(Callback::HttpLengthSize);
            protocol.on_package = callback(Callback::HttpDispatchFrame);
        } else if config.open_http2_protocol {
            protocol.package_length_size = HTTP2_FRAME_HEADER_SIZE;
            protocol.get_package_length = callback(Callback::Http2Length);
            protocol.on_package = callback(Callback::DispatchTask);
        } else if config.open_websocket_protocol {
            protocol.package_length_size = WEBSOCKET_FRAME_HEADER_SIZE;
            protocol.get_package_length = callback(Callback::WebsocketLength);
            protocol.on_package = callback(Callback::WebsocketDispatchFrame);
        }
        protocol.package_length_offset = 0;
        protocol.package_body_offset = 0;
        reader(readable_callback_http)
    } else if config.open_mqtt_protocol {
        // SAFETY: the port's own protocol, borrowed for the call.
        unsafe { swoole_rs_protocols::set_protocol(protocol) };
        protocol.on_package = callback(Callback::DispatchTask);
        reader(readable_callback_length)
    } else if config.open_redis_protocol {
        protocol.on_package = callback(Callback::DispatchTask);
        reader(readable_callback_redis)
    } else {
        reader(readable_callback_raw)
    };
}

/// `SW_HTTP2_FRAME_HEADER_SIZE`.
const HTTP2_FRAME_HEADER_SIZE: u8 = 9;
/// `SW_WEBSOCKET_FRAME_HEADER_SIZE`: header, mask and a 64-bit length.
const WEBSOCKET_FRAME_HEADER_SIZE: u8 = 2 + 4 + 8;

/// `ListenPort::init_protocol()`.
///
/// # Safety
/// `port` is a live `ListenPort` whose settings nothing else touches meanwhile.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole10ListenPort13init_protocolEv")
)]
pub unsafe extern "C" fn init_protocol(port: *mut c_void) {
    // SAFETY: as the caller guarantees.
    let config = unsafe { port_config::of(port) };
    // SAFETY: as the caller guarantees.
    init_protocol_over(config, || unsafe { fact(port, FACT_DTLS) });
}

/// `ListenPort::set_eof_protocol(eof, find_from_right)`, given the string's bytes.
///
/// # Safety
/// `port` is a live `ListenPort` whose settings nothing else touches meanwhile,
/// and `eof` covers `length` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_ports_set_eof(
    port: *mut c_void,
    eof: *const u8,
    length: usize,
    find_from_right: bool,
) {
    // SAFETY: as the caller guarantees.
    let config = unsafe { port_config::of(port) };
    config.open_eof_check = true;
    let protocol = &mut config.protocol;
    protocol.split_by_eof = !find_from_right;
    let length = length.min(protocol.package_eof.len());
    // SAFETY: `eof` covers `length` bytes, and `length` fits the EOF array.
    unsafe { ptr::copy_nonoverlapping(eof, protocol.package_eof.as_mut_ptr(), length) };
    #[expect(clippy::cast_possible_truncation, reason = "at most 8")]
    let length = length as u8;
    protocol.package_eof_len = length;
}

/// `ListenPort::set_length_protocol(length_offset, length_type, body_offset)`.
///
/// # Safety
/// `port` is a live `ListenPort` whose settings nothing else touches meanwhile.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole10ListenPort19set_length_protocolEjcj")
)]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the narrowing stores of the C++ fields"
)]
pub unsafe extern "C" fn set_length_protocol(
    port: *mut c_void,
    length_offset: u32,
    length_type: c_char,
    body_offset: u32,
) {
    // SAFETY: as the caller guarantees.
    let config = unsafe { port_config::of(port) };
    config.open_length_check = true;
    let protocol = &mut config.protocol;
    protocol.package_length_type = length_type as u8;
    protocol.package_length_size = swoole_rs_corebase::swoole_type_size(length_type) as u8;
    protocol.package_length_offset = length_offset as u16;
    protocol.package_body_offset = body_offset as u16;
}

/// `ListenPort::set_stream_protocol()`: length framing as `Stream::set_protocol`
/// sets it, a network-order u32 before the body.
///
/// # Safety
/// `port` is a live `ListenPort` whose settings nothing else touches meanwhile.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole10ListenPort19set_stream_protocolEv")
)]
pub unsafe extern "C" fn set_stream_protocol(port: *mut c_void) {
    // SAFETY: as the caller guarantees.
    let config = unsafe { port_config::of(port) };
    config.open_length_check = true;
    let protocol = &mut config.protocol;
    protocol.get_package_length = callback(Callback::DefaultLength);
    protocol.package_length_type = b'N';
    protocol.package_length_size = 4;
    protocol.package_body_offset = 4;
    protocol.package_length_offset = 0;
}

/// `ListenPort::get_connection_num()`, over `sw_server()`'s event workers.
///
/// # Safety
/// `port` is a live `ListenPort` of the live `sw_server()`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole10ListenPort18get_connection_numEv")
)]
pub unsafe extern "C" fn get_connection_num(port: *mut c_void) -> usize {
    // SAFETY: as the caller guarantees.
    unsafe { view::port_connection_num(swoole_rs_corebase::swr_v1_core_server(), port) }
}

/// `ListenPort::ssl_init()`.
///
/// # Safety
/// `port` is a live `ListenPort` with an SSL context.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZNK6swoole10ListenPort8ssl_initEv")
)]
#[must_use]
pub unsafe extern "C" fn ssl_init(port: *const c_void) -> bool {
    let null = ptr::null_mut();
    // SAFETY: as the caller guarantees; a null target is the port's own context.
    unsafe { run(RUN_SSL_INIT, port.cast_mut(), null, null, null, 0) != 0 }
}

/// `ListenPort::destroy_http_request(conn)`: frees the connection's request, if any.
///
/// # Safety
/// `conn` is a live connection whose `object` is null or from `swr_v1_http_request_new`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole10ListenPort20destroy_http_requestEPNS_10ConnectionE")
)]
pub unsafe extern "C" fn destroy_http_request(_port: *mut c_void, conn: *mut ConnectionRecord) {
    // SAFETY: as the caller guarantees.
    let conn = unsafe { &mut *conn };
    let request = std::mem::replace(&mut conn.object, ptr::null_mut());
    // SAFETY: as the caller guarantees; nothing holds the request after.
    unsafe { swr_v1_http_request_free(request.cast()) };
}

/// Each static `readable_callback_*`: its Rust name and readable kind.
macro_rules! readable_callbacks {
    ($(fn $name:ident = $kind:ident;)*) => {$(
        /// One readable callback kind through port.cc's `port_read`.
        ///
        /// # Safety
        /// The reactor's readable event on a connection of `port`.
        unsafe extern "C" fn $name(reactor: *mut c_void, port: *mut c_void, event: *mut c_void) -> c_int {
            // SAFETY: as the caller guarantees.
            unsafe { (native().1)(reactor, port, event, $kind) }
        }
    )*};
}

readable_callbacks! {
    fn readable_callback_raw = ON_READ_RAW;
    fn readable_callback_length = ON_READ_LENGTH;
    fn readable_callback_http = ON_READ_HTTP;
    fn readable_callback_redis = ON_READ_REDIS;
    fn readable_callback_eof = ON_READ_EOF;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static SEEN: Mutex<Vec<[usize; 6]>> = Mutex::new(Vec::new());

    unsafe extern "C-unwind" fn run_fake(
        kind: u32,
        port: *mut c_void,
        target: *mut c_void,
        sock: *mut c_void,
        ssl: *mut c_void,
        number: c_int,
    ) -> i32 {
        let row = [
            kind as usize,
            port.addr(),
            target.addr(),
            sock.addr(),
            ssl.addr(),
            usize::try_from(number).unwrap_or_default(),
        ];
        SEEN.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(row);
        i32::from(kind != RUN_CLOSE) * 3
    }

    unsafe extern "C-unwind" fn read_fake(
        _: *mut c_void,
        port: *mut c_void,
        _: *mut c_void,
        kind: u32,
    ) -> i32 {
        i32::try_from(port.addr()).unwrap_or_default() * 10
            + i32::try_from(kind).unwrap_or_default()
    }

    thread_local! {
        /// The facts `fact_fake` answers on this thread: bit 0 reuse-port, bit 1 DTLS.
        static FACTS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    }

    unsafe extern "C-unwind" fn fact_fake(_: *mut c_void, which: u32) -> bool {
        FACTS.get() & (1 << which) != 0
    }

    fn last() -> [usize; 6] {
        *SEEN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .last()
            .unwrap_or(&[0; 6])
    }

    #[test]
    fn methods_pass_what_their_forward_did() {
        swr_v1_ports_set_native(run_fake, read_fake, fact_fake);
        let at = |a: usize| ptr::without_provenance_mut::<c_void>(a);
        unsafe {
            assert!(ssl_create(at(1), at(2)), "3 is true");
            assert_eq!(
                last(),
                [RUN_SSL_CREATE as usize, 1, 0, 2, 0, 0],
                "ssl_create row"
            );
            assert!(ssl_context_create(at(1), at(4), true), "context create");
            assert_eq!(
                last(),
                [RUN_CONTEXT_CREATE as usize, 1, 4, 0, 0, 1],
                "allow_empty_cert in number"
            );
            assert!(import(at(1), 9), "import");
            assert_eq!(last()[5], 9, "fd in number");
            close(at(1));
            assert_eq!(last()[0], RUN_CLOSE as usize, "close kind");
            assert_eq!(
                ssl_server_sni_callback(at(5), ptr::null_mut(), at(6)),
                3,
                "sni result"
            );
            assert_eq!(
                last(),
                [RUN_SNI as usize, 0, 0, 0, 5, 0],
                "sni passes only ssl"
            );
            assert_eq!(
                readable_callback_http(at(0), at(2), at(0)),
                20 + i32::try_from(ON_READ_HTTP).unwrap_or_default(),
                "http kind"
            );
            assert_eq!(
                readable_callback_eof(at(0), at(2), at(0)),
                20 + i32::try_from(ON_READ_EOF).unwrap_or_default(),
                "eof kind"
            );
        }
    }

    /// A port whose settings own a fresh `kind` socket on 127.0.0.1, unbound
    /// unless `bind`.
    fn socket_port(kind: c_int, bind: bool) -> *mut PortConfig {
        let config = port_config::swr_v1_port_config_new(1000);
        let socket = swoole_rs_reactor::swr_v1_socket_alloc();
        unsafe {
            socket.write(SocketRecord::new());
            (*socket).fd = libc::socket(libc::AF_INET, kind, 0);
            assert!((*socket).fd >= 0, "socket(2)");
            if bind {
                let mut addr: libc::sockaddr_in = std::mem::zeroed();
                addr.sin_family = libc::sa_family_t::try_from(libc::AF_INET).unwrap();
                addr.sin_addr.s_addr = u32::from_ne_bytes([127, 0, 0, 1]);
                let size = libc::socklen_t::try_from(size_of::<libc::sockaddr_in>()).unwrap();
                assert_eq!(
                    libc::bind((*socket).fd, (&raw const addr).cast(), size),
                    0,
                    "bind(2)"
                );
            }
            (*config).socket = socket.cast();
        }
        config
    }

    fn option(fd: c_int, level: c_int, name: c_int) -> c_int {
        let mut value: c_int = -1;
        let mut size = libc::socklen_t::try_from(size_of::<c_int>()).unwrap();
        let got =
            unsafe { libc::getsockopt(fd, level, name, (&raw mut value).cast(), &raw mut size) };
        assert_eq!(got, 0, "getsockopt(2)");
        value
    }

    #[test]
    fn listen_listens_sets_keepalive_and_the_default_watermark() {
        let mut port = socket_port(libc::SOCK_STREAM, true);
        let at = (&raw mut port).cast::<c_void>();
        let config = unsafe { port_config::of(at) };
        config.open_tcp_keepalive = true;
        config.tcp_user_timeout = 5;
        assert_eq!(unsafe { listen(at) }, 0, "listened");
        let fd = unsafe { view::port_fd(at) };
        assert!(config.listening, "published");
        assert_eq!(
            unsafe { (*config.socket.cast::<SocketRecord>()).listened },
            1,
            "listen(2) succeeded"
        );
        assert_ne!(
            option(fd, libc::SOL_SOCKET, libc::SO_KEEPALIVE),
            0,
            "keepalive"
        );
        #[cfg(target_os = "linux")]
        {
            assert_eq!(
                option(fd, libc::SOL_SOCKET, libc::SO_ACCEPTCONN),
                1,
                "listening socket"
            );
            assert_eq!(
                option(fd, libc::IPPROTO_TCP, libc::TCP_KEEPIDLE),
                3600,
                "keepidle default"
            );
            assert_eq!(
                option(fd, libc::IPPROTO_TCP, libc::TCP_KEEPINTVL),
                60,
                "keepinterval default"
            );
            assert_eq!(
                option(fd, libc::IPPROTO_TCP, libc::TCP_KEEPCNT),
                5,
                "keepcount default"
            );
            assert_eq!(
                option(fd, libc::IPPROTO_TCP, libc::TCP_USER_TIMEOUT),
                5,
                "user timeout"
            );
        }
        assert_eq!(
            config.buffer_high_watermark, 800,
            "socket_buffer_size * 0.8"
        );
        // A second listen keeps a set watermark and does not listen again.
        config.buffer_high_watermark = 7;
        assert_eq!(unsafe { listen(at) }, 0, "already listening");
        assert_eq!(config.buffer_high_watermark, 7, "kept");
        unsafe { close_socket(at) };
        assert!(config.socket.is_null(), "socket released");
        assert_eq!(
            unsafe { libc::fcntl(fd, libc::F_GETFD) },
            -1,
            "descriptor closed"
        );
        unsafe { port_config::swr_v1_port_config_free(port) };
    }

    #[test]
    fn a_failed_listen_warns_and_stays_unpublished() {
        // listen(2) on a datagram socket fails (EOPNOTSUPP).
        let mut port = socket_port(libc::SOCK_DGRAM, false);
        let at = (&raw mut port).cast::<c_void>();
        assert_eq!(unsafe { listen(at) }, -1, "failed");
        let config = unsafe { port_config::of(at) };
        assert!(!config.listening, "not published");
        assert_eq!(config.buffer_high_watermark, 0, "options skipped");
        unsafe { close_socket(at) };
        unsafe { port_config::swr_v1_port_config_free(port) };
    }

    /// A socketless port of `kind` on `host`:`number`, with the native facts `facts`.
    fn unbound_port(kind: c_int, host: &str, number: c_int, facts: u32) -> *mut PortConfig {
        swr_v1_ports_set_native(run_fake, read_fake, fact_fake);
        FACTS.set(facts);
        let config = port_config::swr_v1_port_config_new(1000);
        unsafe {
            (*config).r#type = kind;
            (*config).port = number;
            let set = swoole_rs_corebase::swr_v1_core_string_set(
                &raw const *(*config).host,
                host.as_ptr().cast(),
                host.len(),
            );
            assert_eq!(set, 0, "host set");
        }
        config
    }

    fn record<'a>(config: &PortConfig) -> &'a SocketRecord {
        unsafe { &*config.socket.cast::<SocketRecord>() }
    }

    #[test]
    fn create_socket_binds_the_host_and_takes_the_bound_port() {
        let mut port = unbound_port(1, "127.0.0.1", 0, 0);
        let at = (&raw mut port).cast::<c_void>();
        assert_eq!(unsafe { create_socket(at) }, 0, "created");
        let config = unsafe { port_config::of(at) };
        let socket = record(config);
        assert_eq!(socket.fd_type, FD_STREAM_SERVER, "stream server fd");
        assert_eq!(socket.bound, 1, "bound");
        assert_ne!(config.port, 0, "the kernel's port");
        assert_eq!(
            unsafe { swr_v1_clients_address_port(&raw const socket.info) },
            config.port,
            "getsockname's port"
        );
        assert_ne!(
            option(socket.fd, libc::SOL_SOCKET, libc::SO_REUSEADDR),
            0,
            "SO_REUSEADDR"
        );
        let flags = unsafe { libc::fcntl(socket.fd, libc::F_GETFL) };
        assert_ne!(flags & libc::O_NONBLOCK, 0, "nonblocking");
        let fd_flags = unsafe { libc::fcntl(socket.fd, libc::F_GETFD) };
        assert_ne!(fd_flags & libc::FD_CLOEXEC, 0, "close-on-exec");
        // A port with a socket keeps it unless the server reuses ports.
        let kept = config.socket;
        assert_eq!(unsafe { create_socket(at) }, 0, "kept");
        assert_eq!(config.socket, kept, "same socket");
        unsafe { close_socket(at) };
        unsafe { port_config::swr_v1_port_config_free(port) };
    }

    #[test]
    fn create_socket_makes_a_datagram_server_socket_for_udp() {
        let mut port = unbound_port(2, "127.0.0.1", 0, 0);
        let at = (&raw mut port).cast::<c_void>();
        assert_eq!(unsafe { create_socket(at) }, 0, "created");
        let config = unsafe { port_config::of(at) };
        let socket = record(config);
        assert_eq!(socket.fd_type, FD_DGRAM_SERVER, "datagram server fd");
        assert_eq!(
            option(socket.fd, libc::SOL_SOCKET, libc::SO_TYPE),
            libc::SOCK_DGRAM,
            "datagram"
        );
        unsafe { close_socket(at) };
        unsafe { port_config::swr_v1_port_config_free(port) };
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn create_socket_replaces_the_socket_when_the_server_reuses_ports() {
        let mut port = unbound_port(1, "127.0.0.1", 0, 1 << FACT_REUSE_PORT);
        let at = (&raw mut port).cast::<c_void>();
        assert_eq!(unsafe { create_socket(at) }, 0, "created");
        let config = unsafe { port_config::of(at) };
        let first = record(config).fd;
        assert_ne!(
            option(first, libc::SOL_SOCKET, libc::SO_REUSEPORT),
            0,
            "SO_REUSEPORT"
        );
        // Unmark the old record: only a new bind marks the port's record again.
        unsafe { (*config.socket.cast::<SocketRecord>()).bound = 0 };
        let bound_port = config.port;
        assert_eq!(unsafe { create_socket(at) }, 0, "recreated");
        assert_eq!(record(config).bound, 1, "a new socket, bound");
        assert_eq!(config.port, bound_port, "on the same port");
        assert_ne!(
            option(record(config).fd, libc::SOL_SOCKET, libc::SO_REUSEPORT),
            0,
            "SO_REUSEPORT again"
        );
        unsafe { close_socket(at) };
        unsafe { port_config::swr_v1_port_config_free(port) };
    }

    #[test]
    fn create_socket_fails_on_an_address_it_cannot_assign_or_bind() {
        // A Unix path longer than sun_path: the address is invalid.
        let long = "p".repeat(200);
        let port = unbound_port(5, &long, 0, 0);
        let at = (&raw const port).cast_mut().cast::<c_void>();
        assert_eq!(unsafe { create_socket(at) }, -1, "invalid address");
        unsafe { port_config::swr_v1_port_config_free(port) };
        // An address no local interface has: bind(2) fails.
        let port = unbound_port(1, "203.0.113.1", 0, 0);
        let at = (&raw const port).cast_mut().cast::<c_void>();
        assert_eq!(unsafe { create_socket(at) }, -1, "bind failed");
        assert_eq!(
            log::last_error(),
            libc::EADDRNOTAVAIL,
            "errno as the last error"
        );
        unsafe { port_config::swr_v1_port_config_free(port) };
    }

    /// A fresh port: its settings, and the head pointer `ListenPort` methods take.
    struct Fresh(Box<*mut PortConfig>);

    impl Fresh {
        fn new() -> Self {
            Self(Box::new(port_config::swr_v1_port_config_new(1000)))
        }
        fn at(&mut self) -> *mut c_void {
            (&raw mut *self.0).cast()
        }
        fn config<'a>(&mut self) -> &'a mut PortConfig {
            unsafe { port_config::of(self.at()) }
        }
    }

    impl Drop for Fresh {
        fn drop(&mut self) {
            unsafe { port_config::swr_v1_port_config_free(*self.0) };
        }
    }

    /// The constructor's defaults (was tests/modules/ports/defaults.cc).
    #[test]
    fn a_new_port_has_the_constructor_defaults() {
        let mut port = Fresh::new();
        let config = port.config();
        let p = &config.protocol;
        assert_eq!(config.object_id, 0, "object id");
        assert_eq!(p.package_length_type, b'N', "length type");
        assert_eq!(p.package_length_size, 4, "length size");
        assert_eq!(p.package_length_offset, 0, "length offset");
        assert_eq!(p.package_body_offset, 4, "body offset");
        assert_eq!(
            p.package_max_length,
            2 * 1024 * 1024,
            "SW_INPUT_BUFFER_SIZE"
        );
        assert_eq!(p.package_eof_len, 4, "sizeof(SW_DATA_EOF) - 1");
        assert_eq!(
            &p.package_eof, b"\r\n\r\n\0\0\0\0",
            "SW_DATA_EOF, then zeros"
        );
        assert!(
            p.private_data_2.is_null(),
            "the C++ constructor stores the server"
        );
        assert!(
            !config.open_length_check && !config.open_eof_check,
            "no framing"
        );
        assert!(config.on_read.is_null(), "no reader before init_protocol");
    }

    /// The EOF setter (was tests/modules/ports/settings.cc).
    #[test]
    #[expect(clippy::cast_possible_truncation, reason = "below 256")]
    fn set_eof_protocol_copies_at_most_eight_bytes() {
        for length in [0usize, 1, 2, 7, 8, 9, 16, 1024] {
            for right in [false, true] {
                let mut port = Fresh::new();
                let config = port.config();
                config.protocol.package_eof = *b"xxxxxxxx";
                let value: Vec<u8> = (0..length).map(|i| (i % 256) as u8).collect();
                unsafe { swr_v1_ports_set_eof(port.at(), value.as_ptr(), value.len(), right) };
                let kept = length.min(8);
                let mut eof = *b"xxxxxxxx";
                eof[..kept].copy_from_slice(&value[..kept]);
                let p = &config.protocol;
                assert!(config.open_eof_check, "eof check on");
                assert_eq!(p.split_by_eof, !right, "split unless from the right");
                assert_eq!(usize::from(p.package_eof_len), kept, "length {length}");
                assert_eq!(p.package_eof, eof, "copied bytes, the rest untouched");
                assert!(!config.open_length_check, "length check untouched");
            }
        }
    }

    /// The length setter over every type and boundary offset (was settings.cc).
    #[test]
    #[expect(clippy::cast_possible_wrap, reason = "every C char, as a byte")]
    fn set_length_protocol_narrows_as_the_cpp_fields_do() {
        let edges = [0u32, 1, 255, 65535, 65536, 65537, 0x8000_0000, 0xffff_ffff];
        let mut port = Fresh::new();
        let config = port.config();
        let sentinel = ptr::without_provenance_mut::<c_void>(17);
        config.protocol.get_package_length = sentinel;
        for kind in 0..=255u8 {
            for offset in edges {
                for body in edges {
                    unsafe { set_length_protocol(port.at(), offset, kind as c_char, body) };
                    let p = &config.protocol;
                    let size = swoole_rs_corebase::swoole_type_size(kind as c_char);
                    assert!(config.open_length_check, "length check on");
                    assert_eq!(p.package_length_type, kind, "type");
                    assert_eq!(i32::from(p.package_length_size), size & 0xff, "type size");
                    assert_eq!(
                        u32::from(p.package_length_offset),
                        offset & 0xffff,
                        "u16 offset"
                    );
                    assert_eq!(u32::from(p.package_body_offset), body & 0xffff, "u16 body");
                    assert_eq!(p.get_package_length, sentinel, "length function untouched");
                }
            }
        }
    }

    /// The stream setter after any length and EOF settings (was settings.cc).
    #[test]
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "settings.cc's byte-wrapping iteration values"
    )]
    fn set_stream_protocol_sets_network_u32_framing() {
        let mut port = Fresh::new();
        let config = port.config();
        for iteration in 0..256u32 {
            config.protocol.get_package_length = ptr::without_provenance_mut(17);
            let kind = iteration as u8 as c_char;
            unsafe { set_length_protocol(port.at(), iteration * 257, kind, u32::MAX - iteration) };
            let eof = vec![iteration as u8; (iteration % 17) as usize];
            unsafe { swr_v1_ports_set_eof(port.at(), eof.as_ptr(), eof.len(), iteration % 2 == 1) };
            let eof_before = (
                config.protocol.split_by_eof,
                config.protocol.package_eof_len,
                config.protocol.package_eof,
            );
            unsafe { set_stream_protocol(port.at()) };
            let p = &config.protocol;
            assert!(
                config.open_length_check && config.open_eof_check,
                "both checks on"
            );
            assert_eq!(
                p.get_package_length,
                callback(Callback::DefaultLength),
                "default length"
            );
            assert_eq!(p.package_length_type, b'N', "network u32");
            assert_eq!(p.package_length_size, 4, "four bytes");
            assert_eq!(p.package_length_offset, 0, "at the start");
            assert_eq!(p.package_body_offset, 4, "body after it");
            assert_eq!(
                (p.split_by_eof, p.package_eof_len, p.package_eof),
                eof_before,
                "EOF untouched"
            );
        }
    }

    #[test]
    fn init_protocol_takes_upstreams_branches() {
        use Callback::{
            DefaultLength, DispatchTask as Task, Http2Length, HttpDispatchFrame, HttpLength,
            HttpLengthSize, WebsocketDispatchFrame, WebsocketLength,
        };
        const EOF: u8 = 1;
        const LENGTH: u8 = 2;
        const HTTP: u8 = 4;
        const HTTP2: u8 = 8;
        const WS: u8 = 16;
        const MQTT: u8 = 32;
        const REDIS: u8 = 64;
        const UNTYPED: u8 = 128;
        let mqtt = swoole_rs_protocols::get_package_length as *mut c_void;
        let address = |c: Option<Callback>| c.map_or(ptr::null_mut(), callback);
        // Settings; reader; package, length and length-size callbacks; length size, offsets.
        #[rustfmt::skip]
        let branches = [
            (EOF, reader(readable_callback_eof), Some(Task), None, None, (4, 0, 4)),
            (EOF | LENGTH, reader(readable_callback_eof), Some(Task), None, None, (4, 0, 4)),
            (LENGTH, reader(readable_callback_length), Some(Task), Some(DefaultLength), None, (4, 0, 4)),
            (LENGTH | UNTYPED, reader(readable_callback_length), Some(Task), None, None, (4, 0, 4)),
            (HTTP, reader(readable_callback_http), None, None, None, (4, 0, 0)),
            (HTTP | HTTP2, reader(readable_callback_http), Some(Task), Some(Http2Length), None, (9, 0, 0)),
            (HTTP | WS, reader(readable_callback_http), Some(WebsocketDispatchFrame), Some(WebsocketLength), None,
             (14, 0, 0)),
            (HTTP | HTTP2 | WS, reader(readable_callback_http), Some(HttpDispatchFrame), Some(HttpLength),
             Some(HttpLengthSize), (4, 0, 0)),
            (MQTT, reader(readable_callback_length), Some(Task), None, None, (4, 1, 0)),
            (REDIS, reader(readable_callback_redis), Some(Task), None, None, (4, 0, 4)),
            (0, reader(readable_callback_raw), None, None, None, (4, 0, 4)),
        ];
        for (flags, on_read, package, length, length_size, framing) in branches {
            let mut port = Fresh::new();
            let config = port.config();
            config.open_eof_check = flags & EOF != 0;
            config.open_length_check = flags & LENGTH != 0;
            config.open_http_protocol = flags & HTTP != 0;
            config.open_http2_protocol = flags & HTTP2 != 0;
            config.open_websocket_protocol = flags & WS != 0;
            config.open_mqtt_protocol = flags & MQTT != 0;
            config.open_redis_protocol = flags & REDIS != 0;
            if flags & UNTYPED != 0 {
                config.protocol.package_length_type = 0;
            }
            init_protocol_over(config, || panic!("a stream port asks no DTLS fact"));
            let p = &config.protocol;
            let length = if flags == MQTT { mqtt } else { address(length) };
            assert_eq!(config.on_read, on_read, "{flags}: reader");
            assert_eq!(p.on_package, address(package), "{flags}: package callback");
            assert_eq!(p.get_package_length, length, "{flags}: length function");
            assert_eq!(
                p.get_package_length_size,
                address(length_size),
                "{flags}: size function"
            );
            let actual = (
                p.package_length_size,
                p.package_length_offset,
                p.package_body_offset,
            );
            assert_eq!(actual, framing, "{flags}: framing");
        }
    }

    #[test]
    fn init_protocol_clamps_the_eof_and_skips_plain_datagrams() {
        for (eof_len, kept) in [(0, 0), (8, 8), (9, 8), (255, 8)] {
            let mut port = Fresh::new();
            let config = port.config();
            config.open_eof_check = true;
            config.protocol.package_eof_len = eof_len;
            init_protocol_over(config, || panic!("a stream port asks no DTLS fact"));
            assert_eq!(config.protocol.package_eof_len, kept, "SW_DATA_EOF_MAXLEN");
        }
        for (kind, dtls, initialized) in [
            (2, false, false),
            (4, false, false),
            (6, false, false),
            (2, true, true),
        ] {
            let mut port = Fresh::new();
            let config = port.config();
            config.r#type = kind;
            init_protocol_over(config, || dtls);
            assert_eq!(
                !config.on_read.is_null(),
                initialized,
                "type {kind}, dtls {dtls}"
            );
        }
    }

    #[test]
    fn destroy_http_request_frees_and_clears() {
        unsafe extern "C" fn no_effects(
            _: u32,
            _: *mut c_void,
            _: *const u8,
            _: u64,
            _: u64,
        ) -> u64 {
            0
        }
        // SAFETY: an all-zero record is a closed connection with no object.
        let mut conn: ConnectionRecord = unsafe { std::mem::zeroed() };
        conn.object = swoole_rs_http::swr_v1_http_request_new(no_effects).cast();
        unsafe { destroy_http_request(ptr::null_mut(), &raw mut conn) };
        assert!(conn.object.is_null(), "the request is freed and cleared");
        unsafe { destroy_http_request(ptr::null_mut(), &raw mut conn) };
        assert!(conn.object.is_null(), "no request: nothing to do");
    }
}
