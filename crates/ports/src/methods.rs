//! The `ListenPort` methods of `src/server/port.cc` that were one lifecycle run
//! or one readable callback, defined under their upstream (Itanium) symbols when
//! the hybrid links this crate (`upstream-symbols`). port.cc registers its two
//! runners once; each method calls one as its C++ forward did.

use std::ffi::{c_char, c_int, c_void};
use std::ptr;
use std::sync::OnceLock;

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
}
