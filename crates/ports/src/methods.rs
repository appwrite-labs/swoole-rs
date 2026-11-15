//! The `ListenPort` methods of `src/server/port.cc` other than its TLS, import
//! and close ([`crate::lifecycle`]). The readable callbacks answer their effects
//! in [`crate::read_native`].

use std::ffi::{CStr, c_char, c_int};
use std::ptr;
use swoole_rs_corebase::CCast;

use swoole_rs_clients::address::{assign, swr_v1_clients_address_port, type_name};
use swoole_rs_corebase::constants::SW_DATA_EOF_MAXLEN;
use swoole_rs_corebase::log;
use swoole_rs_corebase::net_types::{
    GetPackageLength, GetPackageLengthSize, OnPackage, SocketAddress,
};
use swoole_rs_reactor::socket_upstream::make_typed;
use swoole_rs_reactor::{Running, SocketFields, SocketRecord};
#[cfg(not(test))]
pub(crate) use swoole_rs_tls::upstream::socket_free;

#[cfg(test)]
pub(crate) use tests::socket_free;

/// `SW_FD_STREAM_SERVER` and `SW_FD_DGRAM_SERVER`.
pub(crate) const FD_STREAM_SERVER: c_int = 1;
pub(crate) const FD_DGRAM_SERVER: c_int = 2;
/// `SW_SOCK_NONBLOCK` and `SW_SOCK_CLOEXEC`.
const SOCK_NONBLOCK: c_int = swoole_rs_corebase::constants::SocketFlag::SW_SOCK_NONBLOCK as c_int;
const SOCK_CLOEXEC: c_int = swoole_rs_corebase::constants::SocketFlag::SW_SOCK_CLOEXEC as c_int;
use swoole_rs_server::port_config::{PortConfig, ReadEvent, Reader};
use swoole_rs_server::view;

use swoole_rs_http_ffi::request_free;

use crate::read::Readable;

/// The listening socket's options `ListenPort::listen` sets where the platform
/// defines them: (level, option, its name, the value) for each enabled one.
fn listen_options(config: &PortConfig) -> Vec<(c_int, c_int, &'static str, c_int)> {
    let mut options = Vec::new();
    #[cfg(target_os = "linux")]
    if config.tcp_defer_accept.get() != 0 {
        let defer = config.tcp_defer_accept.get();
        options.push((
            libc::IPPROTO_TCP,
            libc::TCP_DEFER_ACCEPT,
            "TCP_DEFER_ACCEPT",
            defer,
        ));
    }
    if config.tcp_fastopen.get() != 0 {
        let fastopen = config.tcp_fastopen.get();
        options.push((
            libc::IPPROTO_TCP,
            libc::TCP_FASTOPEN,
            "TCP_FASTOPEN",
            fastopen,
        ));
    }
    if config.open_tcp_keepalive.get() {
        options.push((libc::SOL_SOCKET, libc::SO_KEEPALIVE, "SO_KEEPALIVE", 1));
        #[cfg(target_os = "linux")]
        options.extend([
            (
                libc::IPPROTO_TCP,
                libc::TCP_KEEPIDLE,
                "TCP_KEEPIDLE",
                config.tcp_keepidle.get(),
            ),
            (
                libc::IPPROTO_TCP,
                libc::TCP_KEEPINTVL,
                "TCP_KEEPINTVL",
                config.tcp_keepinterval.get(),
            ),
            (
                libc::IPPROTO_TCP,
                libc::TCP_KEEPCNT,
                "TCP_KEEPCNT",
                config.tcp_keepcount.get(),
            ),
        ]);
        #[cfg(target_os = "linux")]
        if config.tcp_user_timeout.get() > 0 {
            let timeout = config.tcp_user_timeout.get();
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
/// socket options and the default high watermark. A failed option only warns;
/// false when there is no socket or listen(2) failed.
pub(crate) fn listen(config: &PortConfig) -> bool {
    let Some(socket) = config.listening_socket() else {
        return false;
    };
    let backlog = c_int::from(config.backlog.get());
    if !config.listening.get() && !socket.edit(|s| s.listen(backlog)) {
        log::sys_warning(
            c"ListenPort::listen",
            format_args!(
                "listen({}:{}, {}) failed",
                host_text(config),
                config.port.get(),
                config.backlog.get()
            ),
        );
        return false;
    }
    config.listening.set(true);
    let fd = socket.fd();
    for (level, option, name, value) in listen_options(config) {
        if swoole_rs_corebase::sys::setsockopt(fd, level, option, &value) != 0 {
            log::sys_warning(
                c"ListenPort::listen",
                format_args!("setsockopt({name}) failed"),
            );
        }
    }
    if config.buffer_high_watermark.get() == 0 {
        let watermark = (f64::from(config.socket_buffer_size.get()) * 0.8).c_cast::<u32>();
        config.buffer_high_watermark.set(watermark);
    }
    true
}

/// The host as `host.c_str()` prints it.
fn host_text(config: &PortConfig) -> std::borrow::Cow<'_, str> {
    config.host_c_str().to_string_lossy()
}

/// An int option set to 1; 0 or -1 as setsockopt(2).
fn set_option(fd: c_int, level: c_int, option: c_int) -> c_int {
    swoole_rs_corebase::sys::setsockopt::<c_int>(fd, level, option, &1)
}

/// `ListenPort::create_socket()`: keeps the port's socket unless the server
/// reuses ports (Linux), else makes one, sets its options, binds it to the
/// port's host and port, and takes the bound port. On failure it sets the last
/// error to errno and frees the new socket, which the port still points at,
/// and returns false.
pub(crate) fn create_socket(config: &PortConfig, reuse_port: bool) -> bool {
    let reuse_port = cfg!(target_os = "linux") && reuse_port;
    if !config.socket_ptr().is_null() {
        if !reuse_port {
            return true;
        }
        close_socket(config);
    }
    let kind = config.r#type;
    let fd_type = if matches!(kind, 2 | 4 | 6) {
        FD_DGRAM_SERVER
    } else {
        FD_STREAM_SERVER
    };
    let made = make_typed(kind, fd_type, SOCK_NONBLOCK | SOCK_CLOEXEC);
    // The record just made, which only this function holds until the port takes it.
    let Some(socket) = made.map(SocketRecord::into_socket) else {
        log::set_last_error(errno());
        return false;
    };
    let made = socket.as_ptr();
    let fd = socket.fd();
    let kqueue = cfg!(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    ));
    if kqueue && config.is_dtls() {
        set_option(fd, libc::SOL_SOCKET, libc::SO_REUSEPORT);
    }
    let fail = || {
        log::set_last_error(errno());
        // SAFETY: the record just made, freed once, on this failure path.
        unsafe { socket_free(made) };
        false
    };
    if reuse_port && set_option(fd, libc::SOL_SOCKET, libc::SO_REUSEPORT) < 0 {
        return fail();
    }
    let mut address = SocketAddress {
        addr: swoole_rs_corebase::sys::zeroed(),
        len: 0,
        kind,
    };
    let error = assign(
        &mut address.addr,
        &mut address.len,
        kind,
        config.host.bytes_with_nul(),
        config.port.get(),
        true,
    );
    if error != 0 {
        log::set_last_error(error);
        let name = type_name(kind);
        log::warning(
            c"ListenPort::create_socket",
            format_args!(
                "Invalid {} address '{}:{}'",
                name.to_string_lossy(),
                host_text(config),
                config.port.get()
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
    if !socket.edit(|s| s.bind(&address)) || !socket.edit(SocketFields::get_name) {
        return fail();
    }
    let info = socket.read(|s| s.info);
    config.port.set(swr_v1_clients_address_port(&info));
    // Only a bound record: each failure above frees its own, which the port's
    // delete would otherwise free again.
    config.set_listening_socket(socket);
    true
}

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// `ListenPort::close_socket()`: closes and frees the port's socket.
pub(crate) fn close_socket(config: &PortConfig) {
    // Taken first, so nothing finds the record once it is freed.
    let socket = config.take_socket_ptr();
    if socket.is_null() {
        return;
    }
    // SAFETY: `delete socket`: the port's live socket, which Rust boxed
    // (`SocketRecord::into_raw`) and nothing else uses, taken back to drop.
    let socket = unsafe { Box::from_raw(socket) };
    let fd = socket.fd;
    if swoole_rs_corebase::sys::close(fd) < 0 {
        log::sys_warning(
            c"ListenPort::close_socket",
            format_args!("close({fd}) failed"),
        );
    }
    drop(socket);
}

/// `ListenPort::get_protocols()`: a datagram port, else the first protocol its
/// settings turn on, in upstream's order.
pub(crate) fn get_protocols(config: &PortConfig) -> &'static CStr {
    if matches!(config.r#type, 2 | 4 | 6) {
        c"dgram"
    } else if config.open_eof_check.get() {
        c"eof"
    } else if config.open_length_check.get() {
        c"length"
    } else if config.open_http_protocol.get() {
        let http2 = config.open_http2_protocol.get();
        match (http2, config.open_websocket_protocol.get()) {
            (true, true) => c"http|http2|websocket",
            (true, false) => c"http|http2",
            (false, true) => c"http|websocket",
            (false, false) => c"http",
        }
    } else if config.open_mqtt_protocol.get() {
        c"mqtt"
    } else if config.open_redis_protocol.get() {
        c"redis"
    } else {
        c"raw"
    }
}

/// `ListenPort::clear_protocol()`.
pub fn clear_protocol(config: &PortConfig) {
    config.open_eof_check.set(false);
    config.open_length_check.set(false);
    config.open_http_protocol.set(false);
    config.open_websocket_protocol.set(false);
    config.open_http2_protocol.set(false);
    config.open_mqtt_protocol.set(false);
    config.open_redis_protocol.set(false);
}

/// The functions a port's protocol points at.
struct Callbacks {
    dispatch_task: OnPackage,
    http_dispatch_frame: OnPackage,
    websocket_dispatch_frame: OnPackage,
    default_length: GetPackageLength,
    http_length: GetPackageLength,
    http_length_size: GetPackageLengthSize,
    http2_length: GetPackageLength,
    websocket_length: GetPackageLength,
}

#[cfg(not(test))]
const CALLBACKS: Callbacks = {
    use crate::http_routes::exports as http;
    Callbacks {
        dispatch_task: http::dispatch_task,
        http_dispatch_frame: http::dispatch_frame,
        websocket_dispatch_frame: http::websocket_dispatch_frame,
        default_length: crate::framing_recv::default_length_func,
        http_length: http::get_package_length,
        http_length_size: http::get_package_length_size,
        http2_length: http::http2_length,
        websocket_length: swoole_rs_websocket_ffi::frame_api::get_package_length,
    }
};

/// Tests link no C++: each callback is a distinct stub.
#[cfg(test)]
const CALLBACKS: Callbacks = {
    use tests::{length, package};
    Callbacks {
        dispatch_task: package::<0>,
        http_dispatch_frame: package::<1>,
        websocket_dispatch_frame: package::<2>,
        default_length: length::<0>,
        http_length: length::<1>,
        http_length_size: tests::length_size,
        http2_length: length::<2>,
        websocket_length: length::<3>,
    }
};

/// `init_protocol` over the port's settings; `dtls` is `is_dtls()`, asked of a
/// datagram port only.
fn init_protocol_over(config: &PortConfig, dtls: impl FnOnce() -> bool) {
    let datagram = matches!(config.r#type, 2 | 4 | 6);
    if datagram && !dtls() {
        return;
    }
    let on_read = config
        .protocol
        .update(|protocol| init_protocol_callbacks(config, protocol));
    config.on_read.set(Some(on_read));
}

/// The readable callback of the port's settings, whose protocol callbacks and
/// framing it stores in `protocol`.
fn init_protocol_callbacks(
    config: &PortConfig,
    protocol: &mut swoole_rs_corebase::net_types::Protocol,
) -> Reader {
    let c = &CALLBACKS;
    if config.open_eof_check.get() {
        if u32::from(protocol.package_eof_len) > SW_DATA_EOF_MAXLEN {
            #[expect(clippy::cast_possible_truncation, reason = "SW_DATA_EOF_MAXLEN is 8")]
            let max = SW_DATA_EOF_MAXLEN as u8;
            protocol.package_eof_len = max;
        }
        protocol.on_package = Some(c.dispatch_task);
        readable_callback_eof
    } else if config.open_length_check.get() {
        if protocol.package_length_type != 0 {
            protocol.get_package_length = Some(c.default_length);
        }
        protocol.on_package = Some(c.dispatch_task);
        readable_callback_length
    } else if config.open_http_protocol.get() {
        if config.open_http2_protocol.get() && config.open_websocket_protocol.get() {
            protocol.get_package_length = Some(c.http_length);
            protocol.get_package_length_size = Some(c.http_length_size);
            protocol.on_package = Some(c.http_dispatch_frame);
        } else if config.open_http2_protocol.get() {
            protocol.package_length_size = HTTP2_FRAME_HEADER_SIZE;
            protocol.get_package_length = Some(c.http2_length);
            protocol.on_package = Some(c.dispatch_task);
        } else if config.open_websocket_protocol.get() {
            protocol.package_length_size = WEBSOCKET_FRAME_HEADER_SIZE;
            protocol.get_package_length = Some(c.websocket_length);
            protocol.on_package = Some(c.websocket_dispatch_frame);
        }
        protocol.package_length_offset = 0;
        protocol.package_body_offset = 0;
        readable_callback_http
    } else if config.open_mqtt_protocol.get() {
        swoole_rs_protocols_ffi::set_protocol(protocol);
        protocol.on_package = Some(c.dispatch_task);
        readable_callback_length
    } else if config.open_redis_protocol.get() {
        protocol.on_package = Some(c.dispatch_task);
        readable_callback_redis
    } else {
        readable_callback_raw
    }
}

/// `SW_HTTP2_FRAME_HEADER_SIZE`.
const HTTP2_FRAME_HEADER_SIZE: u8 = 9;
/// `SW_WEBSOCKET_FRAME_HEADER_SIZE`: header, mask and a 64-bit length.
const WEBSOCKET_FRAME_HEADER_SIZE: u8 = 2 + 4 + 8;

/// `ListenPort::init_protocol()`, over the port's settings.
pub fn init_protocol(config: &PortConfig) {
    init_protocol_over(config, || config.is_dtls());
}

/// `ListenPort::set_eof_protocol(eof, find_from_right)`, given the string's bytes: at
/// most the first eight.
pub fn set_eof_protocol(config: &mut PortConfig, eof: &[u8], find_from_right: bool) {
    config.open_eof_check.set(true);
    let protocol = config.protocol.get_mut();
    protocol.split_by_eof = !find_from_right;
    let length = eof.len().min(protocol.package_eof.len());
    for (slot, &byte) in protocol.package_eof.iter_mut().zip(eof) {
        *slot = byte;
    }
    #[expect(clippy::cast_possible_truncation, reason = "at most 8")]
    let length = length as u8;
    protocol.package_eof_len = length;
}

/// `ListenPort::set_length_protocol(length_offset, length_type, body_offset)`.
pub fn set_length_protocol(
    config: &mut PortConfig,
    length_offset: u32,
    length_type: c_char,
    body_offset: u32,
) {
    config.open_length_check.set(true);
    let protocol = config.protocol.get_mut();
    protocol.package_length_type = length_type.to_ne_bytes()[0];
    protocol.package_length_size = swoole_rs_corebase::swoole_type_size(length_type).c_cast::<u8>();
    protocol.package_length_offset = length_offset.c_cast::<u16>();
    protocol.package_body_offset = body_offset.c_cast::<u16>();
}

/// `ListenPort::set_stream_protocol()`: length framing as `Stream::set_protocol`
/// sets it, a network-order u32 before the body.
pub fn set_stream_protocol(config: &mut PortConfig) {
    config.open_length_check.set(true);
    let protocol = config.protocol.get_mut();
    protocol.get_package_length = Some(CALLBACKS.default_length);
    protocol.package_length_type = b'N';
    protocol.package_length_size = 4;
    protocol.package_body_offset = 4;
    protocol.package_length_offset = 0;
}

/// `ListenPort::destroy_http_request(conn)`: frees the connection's request, if any.
pub(crate) fn destroy_http_request(conn: swoole_rs_server::Connection<'_>) {
    let request = conn.object();
    conn.set_object(ptr::null_mut());
    let request = ptr::NonNull::new(request.cast::<swoole_rs_http_ffi::HttpRequest>());
    // SAFETY: its `object` is null or `request_new`'s box; nothing holds it after.
    request_free(request.map(|request| unsafe { Box::from_raw(request.as_ptr()) }));
}

#[cfg(not(test))]
use crate::read_native::on_read;

#[cfg(test)]
use tests::on_read;

/// Each static `readable_callback_*`: its Rust name and readable kind.
macro_rules! readable_callbacks {
    ($(fn $name:ident = $kind:ident;)*) => {$(
        /// One readable callback kind.
        fn $name(reactor: Running<'_>, port: view::Port<'_>, read: ReadEvent<'_>) -> c_int {
            on_read(reactor, port, read, Readable::$kind)
        }
    )*};
}

readable_callbacks! {
    fn readable_callback_raw = Raw;
    fn readable_callback_length = Length;
    fn readable_callback_http = Http;
    fn readable_callback_redis = Redis;
    fn readable_callback_eof = Eof;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lifecycle::{context_create, ssl_init};
    use std::ffi::c_void;
    use swoole_rs_server::ConnectionRecord;

    /// The server's port settings, and the two raw steps these tests take on a port
    /// they made themselves.
    mod port_config {
        use std::ffi::c_void;
        pub(super) use swoole_rs_server::port_config::*;

        /// The settings of a port, which is their box.
        pub(super) unsafe fn of<'a>(port: *mut c_void) -> &'a mut PortConfig {
            unsafe { &mut *port.cast::<PortConfig>() }
        }

        /// Frees `swr_v1_port_config_new`'s box.
        pub(super) unsafe fn swr_v1_port_config_free(config: *mut PortConfig) {
            drop(unsafe { Box::from_raw(config) });
        }
    }

    /// The port at `at`, as the server passes one of its own.
    fn port_at<'a>(at: *mut c_void) -> view::Port<'a> {
        unsafe { view::Port::from_raw(at) }.expect("a port")
    }
    // The `ListenPort` methods over a port's address, as the tests name one.
    fn listen(at: *mut c_void) -> c_int {
        if super::listen(port_at(at).config()) {
            0
        } else {
            -1
        }
    }
    fn create_socket(at: *mut c_void) -> c_int {
        let config = port_at(at).config();
        let serv = config.protocol.get().private_data_2;
        let reuse_port = !serv.is_null()
            && unsafe { view::Server::from_raw(serv) }
                .config()
                .enable_reuse_port
                .get();
        if super::create_socket(config, reuse_port) {
            0
        } else {
            -1
        }
    }
    fn close_socket(at: *mut c_void) {
        super::close_socket(port_at(at).config());
    }
    fn close(at: *mut c_void) {
        crate::lifecycle::close(port_at(at).config());
    }
    fn ssl_context_init(at: *mut c_void) -> bool {
        crate::lifecycle::ssl_context_init(unsafe { port_config::of(at) })
    }
    use swoole_rs_corebase::net_types::{PacketLength, Protocol};

    /// `Socket::free()` for a socket no loop, timer or buffer has seen: tests link no
    /// C++, which `socket_free` reaches under `upstream-symbols`.
    pub(crate) unsafe fn socket_free(socket: *mut SocketRecord) {
        // SAFETY: the caller's live record.
        let fd = unsafe { (*socket).fields.fd };
        swoole_rs_corebase::sys::close(fd);
        // SAFETY: Rust allocated the record and nothing uses it after.
        unsafe { SocketRecord::free(socket) };
    }

    /// A readable callback's address.
    fn reader(function: Reader) -> *mut c_void {
        function as *mut c_void
    }

    /// Tests link no C++: the readable kind stands in for the read.
    pub(super) fn on_read(
        _: Running<'_>,
        _: view::Port<'_>,
        _: ReadEvent<'_>,
        kind: Readable,
    ) -> c_int {
        kind as c_int
    }

    /// Stub callbacks, each body distinct so that no two share an address.
    pub(super) unsafe extern "C-unwind" fn package<const N: u8>(
        _: *const Protocol,
        _: *mut c_void,
        _: *const c_void,
    ) -> c_int {
        N.into()
    }
    pub(super) unsafe extern "C" fn length<const N: u8>(
        _: *const Protocol,
        _: *mut c_void,
        _: &mut PacketLength,
    ) -> isize {
        N.into()
    }
    pub(super) unsafe extern "C" fn length_size(_: *mut c_void) -> u8 {
        0
    }

    /// A length function's address, 0 for none.
    fn address(f: Option<GetPackageLength>) -> usize {
        f.map_or(0, |f| (f as *const ()).addr())
    }

    #[test]
    fn context_init_sets_upstreams_defaults_and_close_releases_it() {
        let mut fresh = Fresh::new();
        let at = fresh.at();
        let config = unsafe { port_config::of(at) };
        config.ssl = true;
        assert!(ssl_context_init(at), "a stream port's context");
        let context = config.ssl_context.get().expect("published");
        let fields = unsafe { swoole_rs_tls::owner_fields(&context) };
        assert_eq!(
            (
                fields.prefer_server_ciphers,
                fields.session_tickets,
                fields.stapling,
                fields.stapling_verify
            ),
            (true, false, true, true),
            "flags"
        );
        assert!(!config.is_dtls(), "not DTLS");
        // No files: ssl_init refuses unless SNI contexts exist.
        assert!(!ssl_init(config), "no certificate");
        close(at);
        assert!(config.ssl_context.get().is_none(), "released");
        assert!(
            unsafe { swoole_rs_tls::owner_fields(&context) }.prefer_server_ciphers,
            "a reader's context stays alive after the port closes"
        );
        config.r#type = 2;
        assert!(ssl_context_init(at), "a datagram port's context");
        assert!(config.is_dtls(), "DTLS");
        crate::lifecycle::tls_free(unsafe { &mut *fresh.0 });
        assert!(
            config.ssl_context.get().is_none(),
            "the destructor releases what close left"
        );
    }

    // macOS answers SO_ACCEPTCONN with ENOPROTOOPT, so import refuses every socket there.
    #[cfg(target_os = "linux")]
    #[test]
    fn import_adopts_a_listening_socket_and_refuses_others() {
        let mut fresh = Fresh::new();
        let at = fresh.at();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let bound = listener.local_addr().unwrap().port();
        let fd = std::os::fd::IntoRawFd::into_raw_fd(listener);
        assert!(
            crate::lifecycle::import(unsafe { port_config::of(at) }, fd),
            "listening"
        );
        let config = unsafe { port_config::of(at) };
        let socket = record(config);
        assert_eq!(
            (socket.fd, socket.fd_type),
            (fd, FD_STREAM_SERVER),
            "server socket"
        );
        assert_eq!(
            (config.r#type, config.port.get()),
            (1, c_int::from(bound)),
            "type and port"
        );
        assert_eq!(host_text(config), "127.0.0.1", "host");
        assert!(config.listening.get(), "listening");
        close(at);
        assert!(config.socket_ptr().is_null(), "freed");
        let stream = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
        assert!(
            !crate::lifecycle::import(unsafe { port_config::of(at) }, stream),
            "not listening"
        );
        assert!(
            config.socket_ptr().is_null(),
            "the refused record freed, not kept"
        );
        unsafe { libc::close(stream) };
    }

    /// A port whose settings own a fresh `kind` socket on 127.0.0.1, unbound
    /// unless `bind`.
    fn socket_port(kind: c_int, bind: bool) -> *mut PortConfig {
        let config = port_config::swr_v1_port_config_new(1000);
        let socket = SocketRecord::new().into_raw();
        unsafe {
            (*socket).fields.fd = libc::socket(libc::AF_INET, kind, 0);
            assert!((*socket).fields.fd >= 0, "socket(2)");
            if bind {
                let mut addr: libc::sockaddr_in = std::mem::zeroed();
                addr.sin_family = libc::sa_family_t::try_from(libc::AF_INET).unwrap();
                addr.sin_addr.s_addr = u32::from_ne_bytes([127, 0, 0, 1]);
                let size = libc::socklen_t::try_from(size_of::<libc::sockaddr_in>()).unwrap();
                assert_eq!(
                    libc::bind((*socket).fields.fd, (&raw const addr).cast(), size),
                    0,
                    "bind(2)"
                );
            }
            (*config).set_listening_socket(swoole_rs_reactor::Socket::from_ptr(socket).unwrap());
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

    /// The local address `fd` is bound to, or `None` when it names no socket.
    fn local_addr(fd: c_int) -> Option<(u16, u32)> {
        let mut addr: libc::sockaddr_in = unsafe { std::mem::zeroed() };
        let mut size = libc::socklen_t::try_from(size_of::<libc::sockaddr_in>()).unwrap();
        let got = unsafe { libc::getsockname(fd, (&raw mut addr).cast(), &raw mut size) };
        (got == 0).then_some((addr.sin_port, addr.sin_addr.s_addr))
    }

    #[test]
    fn listen_listens_sets_keepalive_and_the_default_watermark() {
        let port = socket_port(libc::SOCK_STREAM, true);
        let at = port.cast::<c_void>();
        let config = unsafe { port_config::of(at) };
        config.open_tcp_keepalive.set(true);
        config.tcp_user_timeout.set(5);
        assert_eq!(listen(at), 0, "listened");
        let fd = unsafe { view::Port::from_raw(at) }.map_or(-1, view::Port::fd);
        assert!(config.listening.get(), "published");
        assert!(
            unsafe {
                (*config.socket_ptr().cast::<SocketRecord>())
                    .fields
                    .listened
            },
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
            config.buffer_high_watermark.get(),
            800,
            "socket_buffer_size * 0.8"
        );
        // A second listen keeps a set watermark and does not listen again.
        config.buffer_high_watermark.set(7);
        assert_eq!(listen(at), 0, "already listening");
        assert_eq!(config.buffer_high_watermark.get(), 7, "kept");
        // Parallel tests may reuse the number once it is closed, so check that
        // it no longer names this socket: a duplicate keeps the ephemeral port
        // bound, and so unique, until the check is done.
        let bound = local_addr(fd);
        assert!(bound.is_some(), "bound");
        let held = unsafe { libc::dup(fd) };
        assert!(held >= 0, "dup(2)");
        close_socket(at);
        assert!(config.socket_ptr().is_null(), "socket released");
        assert_ne!(local_addr(fd), bound, "descriptor closed");
        unsafe { libc::close(held) };
        unsafe { port_config::swr_v1_port_config_free(port) };
    }

    #[test]
    fn a_failed_listen_warns_and_stays_unpublished() {
        // listen(2) on a datagram socket fails (EOPNOTSUPP).
        let port = socket_port(libc::SOCK_DGRAM, false);
        let at = port.cast::<c_void>();
        assert_eq!(listen(at), -1, "failed");
        let config = unsafe { port_config::of(at) };
        assert!(!config.listening.get(), "not published");
        assert_eq!(config.buffer_high_watermark.get(), 0, "options skipped");
        close_socket(at);
        unsafe { port_config::swr_v1_port_config_free(port) };
    }

    /// A socketless port of `kind` on `host`:`number`, of a server that reuses
    /// ports when `reuse_port` (a `Server` head over a leaked config).
    fn unbound_port(kind: c_int, host: &str, number: c_int, reuse_port: bool) -> *mut PortConfig {
        let config = port_config::swr_v1_port_config_new(1000);
        if reuse_port {
            let server = Box::leak(Box::<swoole_rs_server::ServerConfig>::default());
            server.enable_reuse_port.set(true);
            let head: &mut [*mut c_void; 2] =
                Box::leak(Box::new([ptr::null_mut(), (&raw mut *server).cast()]));
            unsafe { (*config).protocol.get_mut().private_data_2 = head.as_mut_ptr().cast() };
        }
        unsafe {
            (*config).r#type = kind;
            (*config).port.set(number);
            let set = swoole_rs_corebase::swr_v1_core_string_set(
                &raw const *(*config).host,
                host.as_ptr().cast(),
                host.len(),
            );
            assert_eq!(set, 0, "host set");
        }
        config
    }

    fn record<'a>(config: &PortConfig) -> &'a SocketFields {
        unsafe { &*config.socket_ptr().cast::<SocketRecord>() }
    }

    #[test]
    fn create_socket_binds_the_host_and_takes_the_bound_port() {
        let port = unbound_port(1, "127.0.0.1", 0, false);
        let at = port.cast::<c_void>();
        assert_eq!(create_socket(at), 0, "created");
        let config = unsafe { port_config::of(at) };
        let socket = record(config);
        assert_eq!(socket.fd_type, FD_STREAM_SERVER, "stream server fd");
        assert!(socket.bound, "bound");
        assert_ne!(config.port.get(), 0, "the kernel's port");
        assert_eq!(
            swr_v1_clients_address_port(&socket.info),
            config.port.get(),
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
        let kept = config.socket_ptr();
        assert_eq!(create_socket(at), 0, "kept");
        assert_eq!(config.socket_ptr(), kept, "same socket");
        close_socket(at);
        unsafe { port_config::swr_v1_port_config_free(port) };
    }

    #[test]
    fn create_socket_makes_a_datagram_server_socket_for_udp() {
        let port = unbound_port(2, "127.0.0.1", 0, false);
        let at = port.cast::<c_void>();
        assert_eq!(create_socket(at), 0, "created");
        let config = unsafe { port_config::of(at) };
        let socket = record(config);
        assert_eq!(socket.fd_type, FD_DGRAM_SERVER, "datagram server fd");
        assert_eq!(
            option(socket.fd, libc::SOL_SOCKET, libc::SO_TYPE),
            libc::SOCK_DGRAM,
            "datagram"
        );
        close_socket(at);
        unsafe { port_config::swr_v1_port_config_free(port) };
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn create_socket_replaces_the_socket_when_the_server_reuses_ports() {
        let port = unbound_port(1, "127.0.0.1", 0, true);
        let at = port.cast::<c_void>();
        assert_eq!(create_socket(at), 0, "created");
        let config = unsafe { port_config::of(at) };
        let first = record(config).fd;
        assert_ne!(
            option(first, libc::SOL_SOCKET, libc::SO_REUSEPORT),
            0,
            "SO_REUSEPORT"
        );
        // Unmark the old record: only a new bind marks the port's record again.
        unsafe { (*config.socket_ptr().cast::<SocketRecord>()).fields.bound = false };
        let bound_port = config.port.get();
        assert_eq!(create_socket(at), 0, "recreated");
        assert!(record(config).bound, "a new socket, bound");
        assert_eq!(config.port.get(), bound_port, "on the same port");
        assert_ne!(
            option(record(config).fd, libc::SOL_SOCKET, libc::SO_REUSEPORT),
            0,
            "SO_REUSEPORT again"
        );
        close_socket(at);
        unsafe { port_config::swr_v1_port_config_free(port) };
    }

    #[test]
    fn create_socket_fails_on_an_address_it_cannot_assign_or_bind() {
        // A Unix path longer than sun_path: the address is invalid.
        let long = "p".repeat(200);
        let port = unbound_port(5, &long, 0, false);
        let at = port.cast::<c_void>();
        assert_eq!(create_socket(at), -1, "invalid address");
        assert!(unsafe { (*port).socket_ptr() }.is_null(), "freed, not kept");
        unsafe { port_config::swr_v1_port_config_free(port) };
        // An address no local interface has: bind(2) fails.
        let port = unbound_port(1, "203.0.113.1", 0, false);
        let at = port.cast::<c_void>();
        assert_eq!(create_socket(at), -1, "bind failed");
        assert_eq!(
            log::last_error(),
            libc::EADDRNOTAVAIL,
            "errno as the last error"
        );
        assert!(unsafe { (*port).socket_ptr() }.is_null(), "freed, not kept");
        unsafe { port_config::swr_v1_port_config_free(port) };
    }

    /// A fresh port: its settings, which `ListenPort` methods take.
    struct Fresh(*mut PortConfig);

    impl Fresh {
        fn new() -> Self {
            Self(port_config::swr_v1_port_config_new(1000))
        }
        fn at(&mut self) -> *mut c_void {
            self.0.cast()
        }
        fn config<'a>(&mut self) -> &'a mut PortConfig {
            unsafe { port_config::of(self.at()) }
        }
    }

    impl Drop for Fresh {
        fn drop(&mut self) {
            unsafe { port_config::swr_v1_port_config_free(self.0) };
        }
    }

    /// The constructor's defaults (was tests/modules/ports/defaults.cc).
    #[test]
    fn a_new_port_has_the_constructor_defaults() {
        let mut port = Fresh::new();
        let config = port.config();
        let p = config.protocol.get();
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
            !config.open_length_check.get() && !config.open_eof_check.get(),
            "no framing"
        );
        assert!(
            config.on_read.address().is_null(),
            "no reader before init_protocol"
        );
    }

    /// The EOF setter (was tests/modules/ports/settings.cc).
    #[test]
    #[expect(clippy::cast_possible_truncation, reason = "below 256")]
    fn set_eof_protocol_copies_at_most_eight_bytes() {
        for length in [0usize, 1, 2, 7, 8, 9, 16, 1024] {
            for right in [false, true] {
                let mut port = Fresh::new();
                let config = port.config();
                config.protocol.get_mut().package_eof = *b"xxxxxxxx";
                let value: Vec<u8> = (0..length).map(|i| (i % 256) as u8).collect();
                set_eof_protocol(config, &value, right);
                let kept = length.min(8);
                let mut eof = *b"xxxxxxxx";
                eof[..kept].copy_from_slice(&value[..kept]);
                let p = config.protocol.get();
                assert!(config.open_eof_check.get(), "eof check on");
                assert_eq!(p.split_by_eof, !right, "split unless from the right");
                assert_eq!(usize::from(p.package_eof_len), kept, "length {length}");
                assert_eq!(p.package_eof, eof, "copied bytes, the rest untouched");
                assert!(!config.open_length_check.get(), "length check untouched");
            }
        }
    }

    /// The length setter over every type and boundary offset (was settings.cc).
    #[test]
    fn set_length_protocol_narrows_as_the_cpp_fields_do() {
        let edges = [0u32, 1, 255, 65535, 65536, 65537, 0x8000_0000, 0xffff_ffff];
        let mut port = Fresh::new();
        let config = port.config();
        let sentinel = (length::<17> as *const ()).addr();
        config.protocol.get_mut().get_package_length = Some(length::<17>);
        for kind in 0..=255u8 {
            for offset in edges {
                for body in edges {
                    let c_kind = c_char::from_ne_bytes([kind]);
                    set_length_protocol(config, offset, c_kind, body);
                    let p = config.protocol.get();
                    let size = swoole_rs_corebase::swoole_type_size(c_kind);
                    assert!(config.open_length_check.get(), "length check on");
                    assert_eq!(p.package_length_type, kind, "type");
                    assert_eq!(i32::from(p.package_length_size), size & 0xff, "type size");
                    assert_eq!(
                        u32::from(p.package_length_offset),
                        offset & 0xffff,
                        "u16 offset"
                    );
                    assert_eq!(u32::from(p.package_body_offset), body & 0xffff, "u16 body");
                    assert_eq!(
                        address(p.get_package_length),
                        sentinel,
                        "length function untouched"
                    );
                }
            }
        }
    }

    /// The stream setter after any length and EOF settings (was settings.cc).
    #[test]
    fn set_stream_protocol_sets_network_u32_framing() {
        let mut port = Fresh::new();
        let config = port.config();
        for iteration in 0..256u32 {
            config.protocol.get_mut().get_package_length = Some(length::<17>);
            let kind = c_char::from_ne_bytes([iteration.c_cast::<u8>()]);
            set_length_protocol(config, iteration * 257, kind, u32::MAX - iteration);
            let eof = vec![iteration.c_cast::<u8>(); (iteration % 17) as usize];
            set_eof_protocol(config, &eof, iteration % 2 == 1);
            let eof_before = (
                config.protocol.get_mut().split_by_eof,
                config.protocol.get_mut().package_eof_len,
                config.protocol.get_mut().package_eof,
            );
            set_stream_protocol(config);
            let p = config.protocol.get();
            assert!(
                config.open_length_check.get() && config.open_eof_check.get(),
                "both checks on"
            );
            assert_eq!(
                address(p.get_package_length),
                address(Some(CALLBACKS.default_length)),
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
        const EOF: u8 = 1;
        const LENGTH: u8 = 2;
        const HTTP: u8 = 4;
        const HTTP2: u8 = 8;
        const WS: u8 = 16;
        const MQTT: u8 = 32;
        const REDIS: u8 = 64;
        const UNTYPED: u8 = 128;
        let c = &CALLBACKS;
        let task = Some(c.dispatch_task);
        let package = |f: Option<OnPackage>| f.map_or(0, |f| (f as *const ()).addr());
        let size = |f: Option<GetPackageLengthSize>| f.map_or(0, |f| (f as *const ()).addr());
        let mqtt: GetPackageLength = swoole_rs_protocols_ffi::get_package_length;
        // Settings; reader; package, length and length-size callbacks; length size, offsets.
        #[rustfmt::skip]
        let branches = [
            (EOF, reader(readable_callback_eof), task, None, None, (4, 0, 4)),
            (EOF | LENGTH, reader(readable_callback_eof), task, None, None, (4, 0, 4)),
            (LENGTH, reader(readable_callback_length), task, Some(c.default_length), None, (4, 0, 4)),
            (LENGTH | UNTYPED, reader(readable_callback_length), task, None, None, (4, 0, 4)),
            (HTTP, reader(readable_callback_http), None, None, None, (4, 0, 0)),
            (HTTP | HTTP2, reader(readable_callback_http), task, Some(c.http2_length), None, (9, 0, 0)),
            (HTTP | WS, reader(readable_callback_http), Some(c.websocket_dispatch_frame), Some(c.websocket_length),
             None, (14, 0, 0)),
            (HTTP | HTTP2 | WS, reader(readable_callback_http), Some(c.http_dispatch_frame), Some(c.http_length),
             Some(c.http_length_size), (4, 0, 0)),
            (MQTT, reader(readable_callback_length), task, Some(mqtt), None, (4, 1, 0)),
            (REDIS, reader(readable_callback_redis), task, None, None, (4, 0, 4)),
            (0, reader(readable_callback_raw), None, None, None, (4, 0, 4)),
        ];
        for (flags, on_read, on_package, length, length_size, framing) in branches {
            let mut port = Fresh::new();
            let config = port.config();
            config.open_eof_check.set(flags & EOF != 0);
            config.open_length_check.set(flags & LENGTH != 0);
            config.open_http_protocol.set(flags & HTTP != 0);
            config.open_http2_protocol.set(flags & HTTP2 != 0);
            config.open_websocket_protocol.set(flags & WS != 0);
            config.open_mqtt_protocol.set(flags & MQTT != 0);
            config.open_redis_protocol.set(flags & REDIS != 0);
            if flags & UNTYPED != 0 {
                config.protocol.get_mut().package_length_type = 0;
            }
            init_protocol_over(config, || panic!("a stream port asks no DTLS fact"));
            let p = config.protocol.get();
            assert_eq!(config.on_read.address(), on_read, "{flags}: reader");
            assert_eq!(
                package(p.on_package),
                package(on_package),
                "{flags}: package callback"
            );
            assert_eq!(
                address(p.get_package_length),
                address(length),
                "{flags}: length function"
            );
            assert_eq!(
                size(p.get_package_length_size),
                size(length_size),
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
            config.open_eof_check.set(true);
            config.protocol.get_mut().package_eof_len = eof_len;
            init_protocol_over(config, || panic!("a stream port asks no DTLS fact"));
            assert_eq!(
                config.protocol.get_mut().package_eof_len,
                kept,
                "SW_DATA_EOF_MAXLEN"
            );
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
                !config.on_read.address().is_null(),
                initialized,
                "type {kind}, dtls {dtls}"
            );
        }
    }

    #[test]
    fn destroy_http_request_frees_and_clears() {
        // SAFETY: an all-zero record is a closed connection with no object.
        let mut conn: ConnectionRecord = unsafe { std::mem::zeroed() };
        // `request_new`'s boxed request.
        let request = swoole_rs_http_ffi::HttpRequest::new(None);
        conn.object = Box::into_raw(Box::new(request)).cast();
        destroy_http_request((&mut conn).into());
        assert!(conn.object.is_null(), "the request is freed and cleared");
        destroy_http_request((&mut conn).into());
        assert!(conn.object.is_null(), "no request: nothing to do");
    }

    /// Upstream's `server_port.import`. Linux, as core-tests: macOS answers
    /// `SO_ACCEPTCONN` with `ENOPROTOOPT`, not the `EINVAL` of a socket that isn't listening.
    #[cfg(target_os = "linux")]
    #[test]
    fn server_port_import() {
        log::swr_v1_core_set_log_host(crate::read::tests::record);
        let mut port = Fresh::new();
        assert!(!crate::lifecycle::import(port.config(), libc::STDIN_FILENO));
        assert_eq!(log::last_error(), libc::ENOTSOCK);

        let sock = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
        assert!(!crate::lifecycle::import(port.config(), sock));
        assert_eq!(log::last_error(), libc::EINVAL);
        unsafe { libc::close(sock) };
    }

    /// Upstream's `server_port.create`. `Server::add_port` creates the socket, then the
    /// case does again. `cert_file` is written into its slot, as upstream assigns the
    /// string, without the setter's file check.
    #[test]
    fn server_port_create() {
        log::swr_v1_core_set_log_host(crate::read::tests::record);
        let port = unbound_port(1, "127.0.0.1", 0, true);
        let at = port.cast::<c_void>();
        assert_eq!(create_socket(at), 0, "add_port");
        assert_eq!(0, create_socket(at));

        let config = unsafe { port_config::of(at) };
        config.open_eof_check.set(true);
        config.protocol.get_mut().package_eof_len = u8::try_from(SW_DATA_EOF_MAXLEN + 10).unwrap();
        init_protocol(config);
        assert_eq!(c"eof", get_protocols(config));
        assert_eq!(
            u32::from(config.protocol.get_mut().package_eof_len),
            SW_DATA_EOF_MAXLEN
        );

        assert!(ssl_context_init(at));
        let context = config.ssl_context.get().unwrap();
        assert!(!context_create(config, &context, false));
        assert_eq!(log::last_error(), 509, "SW_ERROR_WRONG_OPERATION");
        let cert_file = swoole_rs_tls::swr_v1_tls_owner_view(&context).strings[4];
        let set = |text: &str| unsafe {
            swoole_rs_corebase::swr_v1_core_string_set(cert_file, text.as_ptr().cast(), text.len())
        };
        assert_eq!(set("server.crt"), 0, "cert_file = \"server.crt\"");
        assert!(!context_create(config, &context, true));
        assert_eq!(set(""), 0, "cert_file.clear()");
        assert!(context_create(config, &context, true));

        close(at);
        unsafe { port_config::swr_v1_port_config_free(port) };
    }

    /// Upstream's `server_port.dgram`.
    #[test]
    fn server_port_dgram() {
        let port = unbound_port(2, "127.0.0.1", 0, true);
        let at = port.cast::<c_void>();
        assert_eq!(create_socket(at), 0, "add_port");
        assert_eq!(c"dgram", get_protocols(unsafe { port_config::of(at) }));
        close_socket(at);
        unsafe { port_config::swr_v1_port_config_free(port) };
    }

    /// Upstream's `server.protocols` (server/server.cpp). Its base-mode `Server` only
    /// holds the port, so the port stands alone: `add_port` is `create_socket`.
    #[test]
    fn server_protocols() {
        let port = unbound_port(1, "127.0.0.1", 0, false);
        let at = port.cast::<c_void>();
        assert_eq!(create_socket(at), 0, "add_port");
        let config = unsafe { port_config::of(at) };

        config.open_eof_check.set(true);
        assert_eq!(get_protocols(config), c"eof");
        config.open_eof_check.set(false);

        config.open_length_check.set(true);
        assert_eq!(get_protocols(config), c"length");
        config.open_length_check.set(false);

        config.open_http_protocol.set(true);
        assert_eq!(get_protocols(config), c"http");
        config.open_http_protocol.set(false);

        config.open_http_protocol.set(true);
        config.open_http2_protocol.set(true);
        config.open_websocket_protocol.set(true);
        assert_eq!(get_protocols(config), c"http|http2|websocket");
        config.open_http2_protocol.set(false);
        config.open_websocket_protocol.set(false);
        config.open_http_protocol.set(false);

        config.open_http_protocol.set(true);
        config.open_http2_protocol.set(true);
        assert_eq!(get_protocols(config), c"http|http2");
        config.open_http2_protocol.set(false);
        config.open_http_protocol.set(false);

        config.open_http_protocol.set(true);
        config.open_websocket_protocol.set(true);
        assert_eq!(get_protocols(config), c"http|websocket");
        config.open_websocket_protocol.set(false);
        config.open_http_protocol.set(false);

        config.open_mqtt_protocol.set(true);
        assert_eq!(get_protocols(config), c"mqtt");
        config.open_mqtt_protocol.set(false);

        config.open_redis_protocol.set(true);
        assert_eq!(get_protocols(config), c"redis");
        config.open_redis_protocol.set(false);

        clear_protocol(config);
        assert!(!config.open_eof_check.get());
        assert!(!config.open_length_check.get());
        assert!(!config.open_http_protocol.get());
        assert!(!config.open_websocket_protocol.get());
        assert!(!config.open_http2_protocol.get());
        assert!(!config.open_mqtt_protocol.get());
        assert!(!config.open_redis_protocol.get());
        assert_eq!(get_protocols(config), c"raw");

        close_socket(at);
        unsafe { port_config::swr_v1_port_config_free(port) };
    }
}
