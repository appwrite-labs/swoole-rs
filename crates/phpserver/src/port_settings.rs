//! `Swoole\Server\Port::set()`'s `ListenPort` and TLS options: each lookup,
//! conversion and bound, in the order upstream reads them, written into the
//! port's Rust-owned settings (crates/server's `PortConfig`), TLS owner and SNI
//! contexts (crates/ports).
use crate::settings::{Settings, Step, Stop, checked};
use std::ffi::{CStr, c_char, c_int, c_void};
use swoole_rs_corebase::{constants::SW_IPC_BUFFER_SIZE, swoole_type_size};
use swoole_rs_ports::lifecycle_native::{context_dup, sni_add};
use swoole_rs_server::port_config::PortConfig;
use swoole_rs_tls::{self as tls, OwnedTlsContext, Text, Value};
use swoole_rs_zend::{E_ERROR, E_WARNING, Message, sys::IS_ARRAY};

/// `SW_TIMER_MIN_SEC`.
const TIMER_MIN_SEC: f64 = 0.001;
/// `SW_SSL_DTLS`.
const SSL_DTLS: i64 = 1 << 7;

/// What `set()` reads from its settings array, each value converted as
/// upstream converts it; `None` is a key that is missing or null.
pub(crate) trait Source {
    fn long(&self, key: &CStr) -> Result<Option<i64>, Stop>;
    /// `php_swoole_parse_to_size`.
    fn size(&self, key: &CStr) -> Result<Option<i64>, Stop>;
    fn truth(&self, key: &CStr) -> Option<bool>;
    fn double(&self, key: &CStr) -> Result<Option<f64>, Stop>;
    /// `zend::String`.
    fn bytes(&self, key: &CStr) -> Result<Option<Vec<u8>>, Stop>;
    /// `php_swoole_fatal_error(level, message)`; `set()` stops after it.
    fn fatal(&self, level: c_int, message: &CStr) -> Stop;
    /// `sw_callable_create` of the setting, when it is callable.
    fn callable(&self, key: &CStr) -> Result<Option<*mut c_void>, Stop>;
    /// `sw_callable_free`.
    fn release(&self, callable: *mut c_void);
    /// `serv->single_thread = true`.
    fn single_thread(&self);
    /// `php_swoole_server_length_func`, the `Protocol::LengthFunc` that runs the callable.
    fn length_func(&self) -> *mut c_void;
    /// `ssl_sni_certs`: a context per host name, configured and added.
    fn sni(&self) -> Step;
    /// `port->ssl_init()`.
    fn ssl_init(&self) -> bool;
}

fn u8_of(v: i64) -> u8 {
    u8::try_from(v.clamp(0, u8::MAX.into())).unwrap_or(u8::MAX)
}

fn u16_of(v: i64) -> u16 {
    u16::try_from(v.clamp(0, u16::MAX.into())).unwrap_or(u16::MAX)
}

fn u32_of(v: i64) -> u32 {
    u32::try_from(v.clamp(0, u32::MAX.into())).unwrap_or(u32::MAX)
}

fn i32_of(v: i64) -> c_int {
    c_int::try_from(v.clamp(c_int::MIN.into(), c_int::MAX.into())).unwrap_or(c_int::MAX)
}

/// The socket buffer sizes: clamped, and unlimited when not positive.
fn buffer(v: i64) -> c_int {
    match i32_of(v) {
        n if n <= 0 => c_int::MAX,
        n => n,
    }
}

/// `SSLContext::protocols` as `set_ssl_protocols` stores it: a datagram port
/// drops DTLS (`SW_SUPPORT_DTLS`, which the OpenSSL this links has).
fn protocols(v: i64, dgram: bool) -> i64 {
    if v & SSL_DTLS != 0 && dgram {
        v ^ SSL_DTLS
    } else {
        v
    }
}

/// `Socket::is_dgram(type)`: UDP, UDP6 or a Unix datagram socket.
pub(crate) fn is_dgram(kind: c_int) -> bool {
    matches!(kind, 2 | 4 | 6)
}

/// `package_eof`: the length is published as a `uint8_t` before it is checked.
fn set_eof(s: &impl Source, c: &mut PortConfig, eof: &[u8]) -> Step {
    // The low byte: a `uint8_t`.
    let eof_len = eof.len().to_le_bytes()[0];
    c.protocol.package_eof_len = eof_len;
    if eof_len == 0 {
        return Err(s.fatal(E_ERROR, c"package_eof cannot be an empty string"));
    }
    let Some(destination) = c.protocol.package_eof.get_mut(..eof.len()) else {
        return Err(s.fatal(E_ERROR, c"package_eof max length is 8"));
    };
    destination.copy_from_slice(eof);
    Ok(())
}

/// `package_length_offset` and `package_body_offset`: published, then checked.
fn offset(s: &impl Source, v: i64, field: &mut u16, error: &CStr) -> Step {
    *field = u16_of(v);
    if usize::from(*field) > SW_IPC_BUFFER_SIZE {
        return Err(s.fatal(E_ERROR, error));
    }
    Ok(())
}

/// Every option but TLS, in upstream's order.
pub(crate) fn apply(s: &impl Source, c: &mut PortConfig) -> Step {
    sockets(s, c)?;
    websocket(s, c)?;
    framing(s, c)
}

/// The socket and protocol options up to the WebSocket ones.
fn sockets(s: &impl Source, c: &mut PortConfig) -> Step {
    if let Some(v) = s.long(c"backlog")? {
        c.backlog = u16_of(v);
    }
    if let Some(v) = s.size(c"socket_buffer_size")? {
        c.socket_buffer_size = buffer(v);
    }
    if let Some(v) = s.size(c"kernel_socket_recv_buffer_size")? {
        c.kernel_socket_recv_buffer_size = buffer(v);
    }
    if let Some(v) = s.size(c"kernel_socket_send_buffer_size")? {
        c.kernel_socket_send_buffer_size = buffer(v);
    }
    if let Some(v) = s.long(c"heartbeat_idle_time")? {
        c.heartbeat_idle_time = u16_of(v);
    }
    if let Some(v) = s.size(c"buffer_high_watermark")? {
        c.buffer_high_watermark = u32_of(v);
    }
    if let Some(v) = s.size(c"buffer_low_watermark")? {
        c.buffer_low_watermark = u32_of(v);
    }
    c.open_tcp_nodelay = s.truth(c"open_tcp_nodelay").unwrap_or(true);
    if let Some(v) = s.long(c"tcp_defer_accept")? {
        c.tcp_defer_accept = i32_of(v);
    }
    if let Some(on) = s.truth(c"open_tcp_keepalive") {
        c.open_tcp_keepalive = on;
    }
    if let Some(on) = s.truth(c"open_eof_check") {
        c.open_eof_check = on;
    }
    if let Some(on) = s.truth(c"open_eof_split") {
        c.protocol.split_by_eof = on;
        c.open_eof_check |= on;
    }
    if let Some(eof) = s.bytes(c"package_eof")? {
        set_eof(s, c, &eof)?;
    }
    if let Some(on) = s.truth(c"open_http_protocol") {
        c.open_http_protocol = on;
    }
    if let Some(on) = s.truth(c"open_websocket_protocol") {
        c.open_websocket_protocol = on;
        c.open_http_protocol |= on;
    }
    Ok(())
}

/// The options after the WebSocket ones.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "upstream's implicit conversions to the uint8_t fields"
)]
fn framing(s: &impl Source, c: &mut PortConfig) -> Step {
    if let Some(on) = s.truth(c"open_http2_protocol") {
        c.open_http2_protocol = on;
        c.open_http_protocol |= on;
    }
    if let Some(on) = s.truth(c"open_mqtt_protocol") {
        c.open_mqtt_protocol = on;
    }
    if let Some(v) = s.long(c"open_redis_protocol")? {
        c.open_redis_protocol = v != 0;
    }
    if let Some(v) = s.double(c"max_idle_time")? {
        // SW_MAX: a NaN takes the minimum.
        c.max_idle_time = if v > TIMER_MIN_SEC { v } else { TIMER_MIN_SEC };
    }
    if let Some(v) = s.long(c"tcp_keepidle")? {
        c.tcp_keepidle = i32_of(v);
    }
    if let Some(v) = s.long(c"tcp_keepinterval")? {
        c.tcp_keepinterval = i32_of(v);
    }
    if let Some(v) = s.long(c"tcp_keepcount")? {
        c.tcp_keepcount = i32_of(v);
    }
    if let Some(v) = s.long(c"tcp_user_timeout")? {
        c.tcp_user_timeout = i32_of(v);
    }
    if let Some(on) = s.truth(c"tcp_fastopen") {
        c.tcp_fastopen = on.into();
    }
    if let Some(on) = s.truth(c"open_length_check") {
        c.open_length_check = on;
    }
    if let Some(kind) = s.bytes(c"package_length_type")? {
        // `str_v.val()[0]`: an empty string's NUL.
        let kind = kind.first().copied().unwrap_or(0);
        c.protocol.package_length_type = kind;
        c.protocol.package_length_size = swoole_type_size(c_char::from_ne_bytes([kind])) as u8;
        if c.protocol.package_length_size == 0 {
            return Err(s.fatal(
                E_ERROR,
                c"unknown package_length_type, see pack(). Link: https://php.net/pack",
            ));
        }
    }
    if let Some(v) = s.long(c"package_length_offset")? {
        let error = c"'package_length_offset' value is too large";
        offset(s, v, &mut c.protocol.package_length_offset, error)?;
    }
    let body = match s.long(c"package_body_offset")? {
        Some(v) => Some(v),
        None => s.long(c"package_body_start")?,
    };
    if let Some(v) = body {
        let error = c"'package_body_offset' value is too large";
        offset(s, v, &mut c.protocol.package_body_offset, error)?;
    }
    if let Some(callable) = s.callable(c"package_length_func")? {
        c.protocol.get_package_length = s.length_func();
        let previous = std::mem::replace(&mut c.protocol.private_data_1, callable);
        if !previous.is_null() {
            s.release(previous);
        }
        c.protocol.package_length_size = 0;
        c.protocol.package_length_type = 0;
        c.protocol.package_length_offset = SW_IPC_BUFFER_SIZE as u16;
        s.single_thread();
    }
    if let Some(v) = s.size(c"package_max_length")? {
        c.protocol.package_max_length = u32_of(v);
    }
    Ok(())
}

/// `php_swoole_server_set_websocket_option`: the frames the port passes on
/// (`open_websocket_{close,ping,pong}_frame` as bits 0, 1 and 2) and its
/// handshake's `Sec-WebSocket-Protocol`.
fn websocket(s: &impl Source, c: &mut PortConfig) -> Step {
    for (key, bit) in [
        (c"open_websocket_close_frame", 1),
        (c"open_websocket_ping_frame", 2),
        (c"open_websocket_pong_frame", 4),
    ] {
        if let Some(on) = s.truth(key) {
            c.websocket_frames = if on {
                c.websocket_frames | bit
            } else {
                c.websocket_frames & !bit
            };
        }
    }
    if let Some(protocol) = s.bytes(c"websocket_subprotocol")? {
        c.websocket_protocol = Some(Box::new(protocol));
    }
    Ok(())
}

/// `E_ERROR "<prefix><file>] not found"`, the file up to its first NUL as `%s` reads it.
fn not_found(s: &impl Source, prefix: &[u8], file: &[u8]) -> Stop {
    let mut message = Message::new();
    message.push(prefix);
    message.push(file.split(|&b| b == 0).next().unwrap_or_default());
    message.push(b"] not found");
    s.fatal(E_ERROR, message.as_cstr())
}

/// `php_swoole_server_set_ssl_option`: the context's certificate and key.
fn files(s: &impl Source, owner: *const OwnedTlsContext) -> Step {
    for (key, which, prefix) in [
        (c"ssl_cert_file", Text::CertFile, &b"ssl cert file["[..]),
        (c"ssl_key_file", Text::KeyFile, b"ssl key file["),
    ] {
        if let Some(file) = s.bytes(key)? {
            // SAFETY: the port's live context, which nothing else writes during set().
            if !unsafe { tls::set_text(owner, which, &file) } {
                return Err(not_found(s, prefix, &file));
            }
        }
    }
    Ok(())
}

/// The TLS options of an SSL port, then its SNI contexts and `ssl_init()`.
pub(crate) fn tls(s: &impl Source, owner: *const OwnedTlsContext, dgram: bool) -> Step {
    files(s, owner)?;
    // SAFETY: as in `files`.
    let set = |value| unsafe { tls::set_value(owner, value) };
    // SAFETY: as above.
    let text = |which, value: &[u8]| unsafe { tls::set_text(owner, which, value) };
    if let Some(on) = s.truth(c"ssl_compress") {
        set(Value::DisableCompress(!on));
    }
    if let Some(v) = s.long(c"ssl_protocols")? {
        set(Value::Protocols(protocols(v, dgram)));
    }
    if let Some(on) = s.truth(c"ssl_verify_peer") {
        set(Value::VerifyPeer(on));
    }
    if let Some(on) = s.truth(c"ssl_allow_self_signed") {
        set(Value::AllowSelfSigned(on));
    }
    if let Some(file) = s.bytes(c"ssl_client_cert_file")?
        && !text(Text::ClientCertFile, &file)
    {
        return Err(not_found(s, b"ssl_client_cert_file[", &file));
    }
    for (key, which) in [(c"ssl_cafile", Text::Cafile), (c"ssl_capath", Text::Capath)] {
        if let Some(value) = s.bytes(key)? {
            text(which, &value);
        }
    }
    if let Some(v) = s.long(c"ssl_verify_depth")? {
        set(Value::VerifyDepth(u8_of(v)));
    }
    if let Some(on) = s.truth(c"ssl_prefer_server_ciphers") {
        set(Value::PreferServerCiphers(on));
    }
    for (key, which) in [
        (c"ssl_ciphers", Text::Ciphers),
        (c"ssl_ecdh_curve", Text::EcdhCurve),
        (c"ssl_dhparam", Text::DhParam),
    ] {
        if let Some(value) = s.bytes(key)? {
            text(which, &value);
        }
    }
    s.sni()?;
    if !s.ssl_init() {
        return Err(s.fatal(E_ERROR, c"ssl_init() failed"));
    }
    Ok(())
}

/// The settings array of a PHP `set()` call, for a bound port.
pub(crate) struct Zend<'a> {
    pub(crate) settings: Settings<'a>,
    pub(crate) port: *mut c_void,
}

impl Source for Zend<'_> {
    fn long(&self, key: &CStr) -> Result<Option<i64>, Stop> {
        self.settings.get(key).map(Settings::long).transpose()
    }

    fn size(&self, key: &CStr) -> Result<Option<i64>, Stop> {
        self.settings.get(key).map(Settings::size).transpose()
    }

    fn truth(&self, key: &CStr) -> Option<bool> {
        self.settings.get(key).map(|v| Settings::truth(v) != 0)
    }

    fn double(&self, key: &CStr) -> Result<Option<f64>, Stop> {
        let Some(value) = self.settings.get(key) else {
            return Ok(None);
        };
        let value = value.double();
        checked().map(|()| Some(value))
    }

    fn bytes(&self, key: &CStr) -> Result<Option<Vec<u8>>, Stop> {
        self.settings
            .get(key)
            .map(|v| Settings::string(v, <[u8]>::to_vec))
            .transpose()
    }

    fn fatal(&self, level: c_int, message: &CStr) -> Stop {
        crate::server::fatal(level, message);
        Stop
    }

    fn callable(&self, key: &CStr) -> Result<Option<*mut c_void>, Stop> {
        let Some(value) = self.settings.get(key) else {
            return Ok(None);
        };
        // SAFETY: a live setting; its bailouts stay deferred.
        let callable = unsafe { swoole_rs_phpruntime::callable_create(value.as_ptr()) };
        if checked().is_err() {
            if !callable.is_null() {
                self.release(callable);
            }
            return Err(Stop);
        }
        Ok((!callable.is_null()).then_some(callable))
    }

    fn release(&self, callable: *mut c_void) {
        // SAFETY: a callable `callable_create` made, released once.
        unsafe { swoole_rs_phpruntime::callable_free(callable) };
    }

    fn single_thread(&self) {
        // SAFETY: the live server being configured; nothing else holds its config.
        unsafe { self.settings.server().config_mut() }.single_thread = true;
    }

    fn length_func(&self) -> *mut c_void {
        crate::dispatch::length_func as *mut c_void
    }

    fn sni(&self) -> Step {
        let Some(certs) = self.settings.get(c"ssl_sni_certs") else {
            return Ok(());
        };
        let Some(certs) = certs.array() else {
            let message = c"ssl_sni_certs requires an array mapping host names to cert paths";
            return Err(self.fatal(E_WARNING, message));
        };
        for (key, value) in certs.entries() {
            let Some(name) = key else {
                let message = c"ssl_sni_certs array requires string host name keys";
                return Err(self.fatal(E_WARNING, message));
            };
            if value.kind() != IS_ARRAY {
                return Err(self.fatal(E_WARNING, c"invalid SNI_cert setting"));
            }
            // SAFETY: an SSL port; the copy's reference goes to `sni_add` below or is released.
            let context = unsafe { context_dup(self.port) };
            let item = Self {
                settings: Settings {
                    serv: self.settings.serv,
                    vht: value.array(),
                },
                port: self.port,
            };
            if let Err(stop) = files(&item, context) {
                // SAFETY: the copy's only reference.
                unsafe { tls::swr_v1_tls_owner_free(context) };
                return Err(stop);
            }
            // SAFETY: a live port and name, and the copy's reference, handed over once.
            let added = unsafe { sni_add(self.port, name, context) };
            if !added {
                return Err(self.fatal(E_ERROR, c"ssl_add_sni_cert() failed"));
            }
        }
        Ok(())
    }

    fn ssl_init(&self) -> bool {
        // SAFETY: an SSL port.
        unsafe { swoole_rs_ports::methods::ssl_init(self.port) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, collections::HashMap};
    use swoole_rs_server::port_config::swr_v1_port_config_new;

    /// A settings value, as PHP converts it.
    #[derive(Clone)]
    struct Fixture {
        number: i64,
        real: f64,
        truth: bool,
        bytes: Vec<u8>,
    }

    fn fixture(number: i64) -> Fixture {
        Fixture {
            number,
            real: 0.0,
            truth: number != 0,
            bytes: Vec::new(),
        }
    }

    #[derive(Default)]
    struct Table {
        values: HashMap<&'static str, Fixture>,
        trace: RefCell<Vec<String>>,
    }

    impl Table {
        fn with(mut self, key: &'static str, value: Fixture) -> Self {
            self.values.insert(key, value);
            self
        }

        fn get(&self, key: &CStr, kind: &str) -> Option<Fixture> {
            let key = key.to_str().unwrap();
            self.trace.borrow_mut().push(format!("{kind}:{key}"));
            self.values.get(key).cloned()
        }
    }

    impl Source for Table {
        fn long(&self, key: &CStr) -> Result<Option<i64>, Stop> {
            Ok(self.get(key, "long").map(|v| v.number))
        }
        fn size(&self, key: &CStr) -> Result<Option<i64>, Stop> {
            Ok(self.get(key, "size").map(|v| v.number))
        }
        fn truth(&self, key: &CStr) -> Option<bool> {
            self.get(key, "bool").map(|v| v.truth)
        }
        fn double(&self, key: &CStr) -> Result<Option<f64>, Stop> {
            Ok(self.get(key, "double").map(|v| v.real))
        }
        fn bytes(&self, key: &CStr) -> Result<Option<Vec<u8>>, Stop> {
            Ok(self.get(key, "string").map(|v| v.bytes))
        }
        fn fatal(&self, level: c_int, message: &CStr) -> Stop {
            self.trace
                .borrow_mut()
                .push(format!("fatal{level}:{}", message.to_str().unwrap()));
            Stop
        }
        fn callable(&self, key: &CStr) -> Result<Option<*mut c_void>, Stop> {
            Ok(self
                .get(key, "callable")
                .map(|v| std::ptr::without_provenance_mut(usize::try_from(v.number).unwrap())))
        }
        fn release(&self, callable: *mut c_void) {
            self.trace
                .borrow_mut()
                .push(format!("release:{}", callable.addr()));
        }
        fn single_thread(&self) {
            self.trace.borrow_mut().push("single_thread".into());
        }
        fn length_func(&self) -> *mut c_void {
            std::ptr::without_provenance_mut(11)
        }
        fn sni(&self) -> Step {
            self.trace.borrow_mut().push("sni".into());
            Ok(())
        }
        fn ssl_init(&self) -> bool {
            self.trace.borrow_mut().push("ssl_init".into());
            true
        }
    }

    fn port() -> Box<PortConfig> {
        // SAFETY: a new allocation, owned by the box.
        unsafe { Box::from_raw(swr_v1_port_config_new(8192)) }
    }

    fn text(value: &[u8]) -> Fixture {
        Fixture {
            bytes: value.to_vec(),
            ..fixture(1)
        }
    }

    const EXTREMES: [i64; 10] = [
        i64::MIN,
        i64::MAX,
        i32::MIN as i64,
        i32::MAX as i64,
        -1,
        0,
        1,
        255,
        65535,
        u32::MAX as i64,
    ];

    #[test]
    fn integer_bounds_match_sw_max_sw_min() {
        // ports-policy: each clamp against SW_MAX(lo, SW_MIN(v, hi)).
        let mut state = 71234_u64;
        let samples = (0..50_000).map(|n| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            EXTREMES
                .get(n)
                .copied()
                .unwrap_or(i64::from_ne_bytes(state.to_ne_bytes()))
        });
        for v in samples {
            assert_eq!(i64::from(u16_of(v)), v.clamp(0, 65535));
            assert_eq!(i64::from(u32_of(v)), v.clamp(0, u32::MAX.into()));
            assert_eq!(
                i64::from(i32_of(v)),
                v.clamp(i32::MIN.into(), i32::MAX.into())
            );
            assert_eq!(i64::from(u8_of(v)), v.clamp(0, 255));
            let clamped = v.clamp(i32::MIN.into(), i32::MAX.into());
            let expected = if clamped <= 0 {
                i32::MAX.into()
            } else {
                clamped
            };
            assert_eq!(i64::from(buffer(v)), expected);
        }
    }

    #[test]
    fn options_are_read_in_upstream_order() {
        let table = Table::default();
        let mut c = port();
        assert!(apply(&table, &mut c).is_ok());
        let trace = table.trace.borrow().join(";");
        let expected = [
            "long:backlog",
            "size:socket_buffer_size",
            "size:kernel_socket_recv_buffer_size",
            "size:kernel_socket_send_buffer_size",
            "long:heartbeat_idle_time",
            "size:buffer_high_watermark",
            "size:buffer_low_watermark",
            "bool:open_tcp_nodelay",
            "long:tcp_defer_accept",
            "bool:open_tcp_keepalive",
            "bool:open_eof_check",
            "bool:open_eof_split",
            "string:package_eof",
            "bool:open_http_protocol",
            "bool:open_websocket_protocol",
            "bool:open_websocket_close_frame",
            "bool:open_websocket_ping_frame",
            "bool:open_websocket_pong_frame",
            "string:websocket_subprotocol",
            "bool:open_http2_protocol",
            "bool:open_mqtt_protocol",
            "long:open_redis_protocol",
            "double:max_idle_time",
            "long:tcp_keepidle",
            "long:tcp_keepinterval",
            "long:tcp_keepcount",
            "long:tcp_user_timeout",
            "bool:tcp_fastopen",
            "bool:open_length_check",
            "string:package_length_type",
            "long:package_length_offset",
            "long:package_body_offset",
            "long:package_body_start",
            "callable:package_length_func",
            "size:package_max_length",
        ]
        .join(";");
        assert_eq!(trace, expected);
        // A missing open_tcp_nodelay turns it on.
        assert!(c.open_tcp_nodelay);
    }

    #[test]
    fn flags_and_their_dependents() {
        // flags: each truth value, and the flags that turn another one on.
        for on in [false, true] {
            let value = fixture(i64::from(on));
            let mut c = port();
            let table = [
                "open_tcp_nodelay",
                "open_tcp_keepalive",
                "open_eof_split",
                "open_websocket_protocol",
                "open_http2_protocol",
                "open_mqtt_protocol",
                "open_redis_protocol",
                "tcp_fastopen",
                "open_length_check",
            ]
            .into_iter()
            .fold(Table::default(), |t, key| t.with(key, value.clone()));
            assert!(apply(&table, &mut c).is_ok());
            assert_eq!(c.open_tcp_nodelay, on);
            assert_eq!(c.open_tcp_keepalive, on);
            assert_eq!(c.protocol.split_by_eof, on);
            assert_eq!(c.open_eof_check, on, "open_eof_split turns eof_check on");
            assert_eq!(c.open_websocket_protocol, on);
            assert_eq!(c.open_http2_protocol, on);
            assert_eq!(c.open_http_protocol, on, "websocket and http2 turn http on");
            assert_eq!(c.open_mqtt_protocol, on);
            assert_eq!(c.open_redis_protocol, on);
            assert_eq!(c.tcp_fastopen, c_int::from(on));
            assert_eq!(c.open_length_check, on);
        }
        // A false dependent leaves the flag it would set as it was.
        let mut c = port();
        c.open_eof_check = true;
        c.open_http_protocol = true;
        let table = Table::default()
            .with("open_eof_split", fixture(0))
            .with("open_websocket_protocol", fixture(0))
            .with("open_http2_protocol", fixture(0));
        assert!(apply(&table, &mut c).is_ok());
        assert!(c.open_eof_check && c.open_http_protocol);
        // open_redis_protocol is read as an integer.
        for v in [i64::MIN, -1, 0, 1, i64::MAX] {
            let mut c = port();
            let table = Table::default().with("open_redis_protocol", fixture(v));
            assert!(apply(&table, &mut c).is_ok());
            assert_eq!(c.open_redis_protocol, v != 0);
        }
    }

    #[test]
    fn numbers_are_clamped_into_their_fields() {
        for v in EXTREMES {
            let mut c = port();
            let table = [
                "backlog",
                "socket_buffer_size",
                "kernel_socket_recv_buffer_size",
                "kernel_socket_send_buffer_size",
                "heartbeat_idle_time",
                "buffer_high_watermark",
                "buffer_low_watermark",
                "tcp_defer_accept",
                "tcp_keepidle",
                "tcp_keepinterval",
                "tcp_keepcount",
                "tcp_user_timeout",
                "package_max_length",
            ]
            .into_iter()
            .fold(Table::default(), |t, key| t.with(key, fixture(v)));
            assert!(apply(&table, &mut c).is_ok());
            assert_eq!(c.backlog, u16_of(v));
            assert_eq!(c.heartbeat_idle_time, u16_of(v));
            for size in [
                c.socket_buffer_size,
                c.kernel_socket_recv_buffer_size,
                c.kernel_socket_send_buffer_size,
            ] {
                assert_eq!(size, buffer(v));
            }
            assert_eq!(c.buffer_high_watermark, u32_of(v));
            assert_eq!(c.buffer_low_watermark, u32_of(v));
            assert_eq!(c.protocol.package_max_length, u32_of(v));
            for n in [
                c.tcp_defer_accept,
                c.tcp_keepidle,
                c.tcp_keepinterval,
                c.tcp_keepcount,
                c.tcp_user_timeout,
            ] {
                assert_eq!(n, i32_of(v));
            }
        }
    }

    #[test]
    fn max_idle_time_has_a_floor() {
        for (v, expected) in [
            (0.0, TIMER_MIN_SEC),
            (-5.0, TIMER_MIN_SEC),
            (f64::NAN, TIMER_MIN_SEC),
            (0.5, 0.5),
        ] {
            let mut c = port();
            let value = Fixture {
                real: v,
                ..fixture(0)
            };
            let table = Table::default().with("max_idle_time", value);
            assert!(apply(&table, &mut c).is_ok());
            assert_eq!(c.max_idle_time.to_bits(), expected.to_bits());
        }
    }

    #[test]
    fn package_eof_is_bounded() {
        // packet_settings: the u8 length is published, then an empty or
        // over-long one is fatal, and a valid one is copied.
        for length in [0, 1, 8, 9, 256, 264] {
            let mut c = port();
            let eof = vec![b'x'; length];
            let table = Table::default().with("package_eof", text(&eof));
            let result = apply(&table, &mut c);
            assert_eq!(c.protocol.package_eof_len, length.to_le_bytes()[0]);
            let trace = table.trace.borrow().join(";");
            match length {
                0 | 256 => assert!(trace.ends_with("fatal1:package_eof cannot be an empty string")),
                9 | 264 => assert!(trace.ends_with("fatal1:package_eof max length is 8")),
                _ => {
                    assert!(result.is_ok());
                    assert_eq!(&c.protocol.package_eof[..length], &eof[..]);
                }
            }
            assert_eq!(result.is_ok(), matches!(length, 1 | 8));
        }
    }

    #[test]
    fn package_length_type_sets_the_width() {
        for (kind, width) in [
            (&b"N"[..], 4),
            (b"c", 1),
            (b"n", 2),
            (b"Q", 8),
            (b"x", 0),
            (b"", 0),
        ] {
            let mut c = port();
            let table = Table::default().with("package_length_type", text(kind));
            let result = apply(&table, &mut c);
            assert_eq!(
                c.protocol.package_length_type,
                kind.first().copied().unwrap_or(0)
            );
            assert_eq!(c.protocol.package_length_size, width);
            assert_eq!(result.is_ok(), width != 0);
        }
    }

    #[test]
    fn offsets_are_clamped_then_checked() {
        let limit = i64::try_from(SW_IPC_BUFFER_SIZE).unwrap();
        for v in [i64::MIN, -1, 0, 1, limit, limit + 1, 65535, i64::MAX] {
            let expected = v.clamp(0, 65535);
            for key in [
                "package_length_offset",
                "package_body_offset",
                "package_body_start",
            ] {
                let mut c = port();
                let table = Table::default().with(key, fixture(v));
                let result = apply(&table, &mut c);
                let field = if key == "package_length_offset" {
                    c.protocol.package_length_offset
                } else {
                    c.protocol.package_body_offset
                };
                assert_eq!(i64::from(field), expected);
                assert_eq!(result.is_ok(), expected <= limit);
            }
        }
        // package_body_offset wins over its alias.
        let mut c = port();
        let table = Table::default()
            .with("package_body_offset", fixture(3))
            .with("package_body_start", fixture(5));
        assert!(apply(&table, &mut c).is_ok());
        assert_eq!(c.protocol.package_body_offset, 3);
    }

    #[test]
    fn package_length_func_replaces_the_framing() {
        let mut c = port();
        c.protocol.private_data_1 = std::ptr::without_provenance_mut(7);
        let table = Table::default().with("package_length_func", fixture(9));
        assert!(apply(&table, &mut c).is_ok());
        assert_eq!(c.protocol.private_data_1.addr(), 9);
        assert_eq!(c.protocol.package_length_size, 0);
        assert_eq!(c.protocol.package_length_type, 0);
        assert_eq!(
            usize::from(c.protocol.package_length_offset),
            SW_IPC_BUFFER_SIZE
        );
        assert_eq!(c.protocol.get_package_length.addr(), 11);
        let trace = table.trace.borrow().join(";");
        assert!(trace.contains("callable:package_length_func;release:7;single_thread"));
    }

    #[test]
    fn websocket_frames_and_subprotocol() {
        let mut c = port();
        c.websocket_frames = 2;
        let table = Table::default()
            .with("open_websocket_close_frame", fixture(1))
            .with("open_websocket_ping_frame", fixture(0))
            .with("open_websocket_pong_frame", fixture(1))
            .with("websocket_subprotocol", text(b"chat"));
        assert!(apply(&table, &mut c).is_ok());
        assert_eq!(c.websocket_frames, 5);
        assert_eq!(
            c.websocket_protocol.as_deref().map(Vec::as_slice),
            Some(&b"chat"[..])
        );
    }

    fn warning(_function: &CStr, _message: &str) {}

    #[test]
    fn tls_options_in_order_and_converted() {
        // tls_settings: each option's order and conversion.
        let prepared = tls::owner_new(warning);
        let owner = prepared.owner;
        let manifest = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml").as_bytes();
        let table = Table::default()
            .with("ssl_cert_file", text(manifest))
            .with("ssl_key_file", text(manifest))
            .with("ssl_compress", fixture(1))
            .with("ssl_protocols", fixture(SSL_DTLS | 4))
            .with("ssl_verify_peer", fixture(1))
            .with("ssl_allow_self_signed", fixture(1))
            .with("ssl_client_cert_file", text(manifest))
            .with("ssl_cafile", text(b"ca"))
            .with("ssl_capath", text(b"cap"))
            .with("ssl_verify_depth", fixture(256))
            .with("ssl_prefer_server_ciphers", fixture(1))
            .with("ssl_ciphers", text(b"HIGH"))
            .with("ssl_ecdh_curve", text(b"auto"))
            .with("ssl_dhparam", text(b"dh"));
        assert!(tls(&table, owner, true).is_ok());
        let expected = [
            "string:ssl_cert_file",
            "string:ssl_key_file",
            "bool:ssl_compress",
            "long:ssl_protocols",
            "bool:ssl_verify_peer",
            "bool:ssl_allow_self_signed",
            "string:ssl_client_cert_file",
            "string:ssl_cafile",
            "string:ssl_capath",
            "long:ssl_verify_depth",
            "bool:ssl_prefer_server_ciphers",
            "string:ssl_ciphers",
            "string:ssl_ecdh_curve",
            "string:ssl_dhparam",
            "sni",
            "ssl_init",
        ]
        .join(";");
        assert_eq!(table.trace.borrow().join(";"), expected);
        // SAFETY: the owner's fields, live until it is freed below.
        let fields = unsafe { *prepared.fields };
        assert_eq!(fields.disable_compress, 0, "ssl_compress is inverted");
        assert_eq!(fields.protocols, 4, "a datagram port drops DTLS");
        assert_eq!((fields.verify_peer, fields.allow_self_signed), (1, 1));
        assert_eq!(fields.verify_depth, 255);
        assert_eq!(fields.prefer_server_ciphers, 1);
        let view = |index: usize| {
            // SAFETY: a slot of the live owner.
            let view =
                unsafe { swoole_rs_corebase::swr_v1_core_string_view(prepared.strings[index]) };
            // SAFETY: as above.
            unsafe { std::slice::from_raw_parts(view.data.cast::<u8>(), view.length) }.to_vec()
        };
        assert_eq!(view(0), b"HIGH");
        assert_eq!(view(1), b"auto");
        assert_eq!(view(3), b"dh");
        assert_eq!(view(7), manifest);
        assert_eq!(view(9), b"ca");
        assert_eq!(view(10), b"cap");
        assert_eq!(protocols(SSL_DTLS | 4, false), SSL_DTLS | 4);
        for v in [i64::MIN, -1, 0, 1, 255, 256, i64::MAX] {
            assert_eq!(i64::from(u8_of(v)), v.clamp(0, 255));
        }
        // SAFETY: the owner made above.
        unsafe { tls::swr_v1_tls_owner_free(owner) };
    }

    #[test]
    fn a_missing_certificate_is_fatal() {
        let prepared = tls::owner_new(warning);
        for key in ["ssl_cert_file", "ssl_key_file", "ssl_client_cert_file"] {
            let table = Table::default().with(key, text(b"/nonexistent/x.pem\0tail"));
            assert!(tls(&table, prepared.owner, false).is_err());
            let message = match key {
                "ssl_cert_file" => "ssl cert file[/nonexistent/x.pem] not found",
                "ssl_key_file" => "ssl key file[/nonexistent/x.pem] not found",
                _ => "ssl_client_cert_file[/nonexistent/x.pem] not found",
            };
            let trace = table.trace.borrow().join(";");
            assert!(trace.ends_with(&format!("fatal1:{message}")), "{trace}");
        }
        // SAFETY: the owner made above.
        unsafe { tls::swr_v1_tls_owner_free(prepared.owner) };
    }
}
