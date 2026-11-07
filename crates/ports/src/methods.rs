//! The `ListenPort` methods of `src/server/port.cc` that were one lifecycle run
//! or one readable callback, defined under their upstream (Itanium) symbols when
//! the hybrid links this crate (`upstream-symbols`). port.cc registers its two
//! runners once; each method calls one as its C++ forward did.

use std::ffi::{c_char, c_int, c_void};
use std::ptr;
use std::sync::OnceLock;

use swoole_rs_corebase::log;
use swoole_rs_reactor::{SocketRecord, swr_v1_socket_dealloc, swr_v1_socket_listen};
use swoole_rs_server::port_config::{self, PortConfig};
use swoole_rs_server::view;

use crate::lifecycle::{
    RUN_CLOSE, RUN_CONTEXT_CREATE, RUN_CONTEXT_INIT, RUN_IMPORT, RUN_SNI, RUN_SSL_CREATE,
    RUN_WARN_LISTEN,
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

static NATIVE: OnceLock<(NativeRun, NativeRead)> = OnceLock::new();

/// Registers port.cc's runners, which the methods below call.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_ports_set_native(run: NativeRun, read: NativeRead) {
    // A second registration is the same pair; keeping the first is equivalent.
    NATIVE.get_or_init(|| (run, read));
}

fn native() -> (NativeRun, NativeRead) {
    match NATIVE.get() {
        Some(native) => *native,
        // port.cc registers before any ListenPort exists.
        None => std::process::abort(),
    }
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
        let null = ptr::null_mut();
        // SAFETY: as the caller guarantees.
        unsafe { run(RUN_WARN_LISTEN, port, null, null, null, 0) };
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

/// [`protocol_flags`] for `init_protocol`.
///
/// # Safety
/// `port` is a live `ListenPort`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_ports_flags(port: *mut c_void) -> u32 {
    // SAFETY: as the caller guarantees.
    protocol_flags(unsafe { view::port(port) })
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
    crate::swr_v1_ports_protocol_name(unsafe { swr_v1_ports_flags(port) })
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

/// Each static `readable_callback_*`: its symbol, Rust name and readable kind.
macro_rules! readable_callbacks {
    ($($symbol:literal fn $name:ident = $kind:ident;)*) => {$(
        #[doc = concat!("`", $symbol, "`.")]
        ///
        /// # Safety
        /// The reactor's readable event on a connection of `port`.
        #[cfg_attr(feature = "upstream-symbols", unsafe(export_name = $symbol))]
        pub unsafe extern "C" fn $name(reactor: *mut c_void, port: *mut c_void, event: *mut c_void) -> c_int {
            // SAFETY: as the caller guarantees.
            unsafe { (native().1)(reactor, port, event, $kind) }
        }
    )*};
}

readable_callbacks! {
    "_ZN6swoole10ListenPort21readable_callback_rawEPNS_7ReactorEPS0_PNS_5EventE"
    fn readable_callback_raw = ON_READ_RAW;
    "_ZN6swoole10ListenPort24readable_callback_lengthEPNS_7ReactorEPS0_PNS_5EventE"
    fn readable_callback_length = ON_READ_LENGTH;
    "_ZN6swoole10ListenPort22readable_callback_httpEPNS_7ReactorEPS0_PNS_5EventE"
    fn readable_callback_http = ON_READ_HTTP;
    "_ZN6swoole10ListenPort23readable_callback_redisEPNS_7ReactorEPS0_PNS_5EventE"
    fn readable_callback_redis = ON_READ_REDIS;
    "_ZN6swoole10ListenPort21readable_callback_eofEPNS_7ReactorEPS0_PNS_5EventE"
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

    fn last() -> [usize; 6] {
        *SEEN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .last()
            .unwrap_or(&[0; 6])
    }

    #[test]
    fn methods_pass_what_their_forward_did() {
        swr_v1_ports_set_native(run_fake, read_fake);
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
    fn a_failed_listen_warns_through_native_and_stays_unpublished() {
        swr_v1_ports_set_native(run_fake, read_fake);
        // listen(2) on a datagram socket fails (EOPNOTSUPP).
        let mut port = socket_port(libc::SOCK_DGRAM, false);
        let at = (&raw mut port).cast::<c_void>();
        assert_eq!(unsafe { listen(at) }, -1, "failed");
        let config = unsafe { port_config::of(at) };
        assert!(!config.listening, "not published");
        assert_eq!(config.buffer_high_watermark, 0, "options skipped");
        let warned = SEEN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&[RUN_WARN_LISTEN as usize, at.addr(), 0, 0, 0, 0]);
        assert!(warned, "the host-naming warning runs natively");
        unsafe { close_socket(at) };
        unsafe { port_config::swr_v1_port_config_free(port) };
    }
}
