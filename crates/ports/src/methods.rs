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
use swoole_rs_corebase::net_types::SocketAddress;
use swoole_rs_corebase::{log, swr_v1_clients_gethostbyname};
use swoole_rs_reactor::socket_upstream::make_socket_typed;
use swoole_rs_reactor::{
    SocketRecord, swr_v1_socket_bind, swr_v1_socket_dealloc, swr_v1_socket_get_name,
    swr_v1_socket_listen,
};
use swoole_rs_tls::upstream::socket_free;

/// `SW_FD_STREAM_SERVER` and `SW_FD_DGRAM_SERVER`.
const FD_STREAM_SERVER: c_int = 1;
const FD_DGRAM_SERVER: c_int = 2;
/// `SW_SOCK_NONBLOCK` and `SW_SOCK_CLOEXEC`.
const SOCK_NONBLOCK: c_int = 1 << 2;
const SOCK_CLOEXEC: c_int = 1 << 3;
use swoole_rs_server::port_config::{self, PortConfig};
use swoole_rs_server::view;

use crate::lifecycle::{
    RUN_CLOSE, RUN_CONTEXT_CREATE, RUN_CONTEXT_INIT, RUN_IMPORT, RUN_SNI, RUN_SSL_CREATE,
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
        assert_eq!(unsafe { create_socket(at) }, 0, "recreated");
        assert_eq!(
            unsafe { libc::fcntl(first, libc::F_GETFD) },
            -1,
            "the old socket closed"
        );
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
}
