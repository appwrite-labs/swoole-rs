//! `Swoole\Server\Port::set()`'s `ListenPort` and TLS options: each lookup,
//! conversion and bound, in the order upstream reads them, written into the
//! port's Rust-owned settings (crates/server's `PortConfig`), TLS owner and SNI
//! contexts (crates/ports).
use crate::settings::{Settings, Step, Stop, checked};
use std::ffi::{CStr, c_char, c_int, c_void};
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::net_types::{GetPackageLength, Protocol};
use swoole_rs_corebase::{constants::SW_IPC_BUFFER_SIZE, swoole_type_size};
use swoole_rs_ports::lifecycle::{context_dup, sni_add};
use swoole_rs_server::port_config::{LengthOwner, PortConfig};
use swoole_rs_server::view;
use swoole_rs_tls::{self as tls, OwnedTlsContext, Text, Value};
use swoole_rs_zend::{E_ERROR, E_WARNING, Message};

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
    /// `sw_callable_create` of the setting, when it is callable: the pointer
    /// the protocol names, and its owner.
    fn callable(&self, key: &CStr) -> Result<Option<(*mut c_void, LengthOwner)>, Stop>;
    /// `sw_callable_free` of the owner a new one replaced.
    fn release(&self, owner: LengthOwner);
    /// `serv->single_thread = true`.
    fn single_thread(&self);
    /// `php_swoole_server_length_func`, the `Protocol::LengthFunc` that runs the callable.
    fn length_func(&self) -> GetPackageLength;
    /// `ssl_sni_certs`: a context per host name, configured and added.
    fn sni(&self) -> Step;
    /// `port->ssl_init()`.
    fn ssl_init(&self) -> bool;
    /// `f` over the port's settings, borrowed for `f` alone. `f` runs no PHP
    /// code; every read above can, and that code may `set()` the same port.
    fn config<R>(&self, f: impl FnOnce(&PortConfig) -> R) -> R;
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
fn set_eof(s: &impl Source, eof: &[u8]) -> Step {
    // The low byte: a `uint8_t`.
    let eof_len = eof.len().to_le_bytes()[0];
    let copied = protocol(s, |p| {
        p.package_eof_len = eof_len;
        let destination = p.package_eof.get_mut(..eof.len());
        destination.map(|destination| destination.copy_from_slice(eof))
    });
    if eof_len == 0 {
        return Err(s.fatal(E_ERROR, c"package_eof cannot be an empty string"));
    }
    if copied.is_none() {
        return Err(s.fatal(E_ERROR, c"package_eof max length is 8"));
    }
    Ok(())
}

/// `f` over the port's protocol, which then replaces it.
fn protocol<R>(s: &impl Source, f: impl FnOnce(&mut Protocol) -> R) -> R {
    s.config(|c| c.protocol.update(f))
}

/// `package_length_offset` and `package_body_offset`: published, then checked.
fn offset(s: &impl Source, v: i64, field: fn(&mut Protocol) -> &mut u16, error: &CStr) -> Step {
    let v = u16_of(v);
    protocol(s, |p| *field(p) = v);
    if usize::from(v) > SW_IPC_BUFFER_SIZE {
        return Err(s.fatal(E_ERROR, error));
    }
    Ok(())
}

/// Every option but TLS, in upstream's order.
pub(crate) fn apply(s: &impl Source) -> Step {
    sockets(s)?;
    websocket(s)?;
    framing(s)
}

/// The socket and protocol options up to the WebSocket ones.
fn sockets(s: &impl Source) -> Step {
    if let Some(v) = s.long(c"backlog")? {
        s.config(|c| c.backlog.set(u16_of(v)));
    }
    if let Some(v) = s.size(c"socket_buffer_size")? {
        s.config(|c| c.socket_buffer_size.set(buffer(v)));
    }
    if let Some(v) = s.size(c"kernel_socket_recv_buffer_size")? {
        s.config(|c| c.kernel_socket_recv_buffer_size.set(buffer(v)));
    }
    if let Some(v) = s.size(c"kernel_socket_send_buffer_size")? {
        s.config(|c| c.kernel_socket_send_buffer_size.set(buffer(v)));
    }
    if let Some(v) = s.long(c"heartbeat_idle_time")? {
        s.config(|c| c.heartbeat_idle_time.set(u16_of(v)));
    }
    if let Some(v) = s.size(c"buffer_high_watermark")? {
        s.config(|c| c.buffer_high_watermark.set(u32_of(v)));
    }
    if let Some(v) = s.size(c"buffer_low_watermark")? {
        s.config(|c| c.buffer_low_watermark.set(u32_of(v)));
    }
    let on = s.truth(c"open_tcp_nodelay").unwrap_or(true);
    s.config(|c| c.open_tcp_nodelay.set(on));
    if let Some(v) = s.long(c"tcp_defer_accept")? {
        s.config(|c| c.tcp_defer_accept.set(i32_of(v)));
    }
    if let Some(on) = s.truth(c"open_tcp_keepalive") {
        s.config(|c| c.open_tcp_keepalive.set(on));
    }
    if let Some(on) = s.truth(c"open_eof_check") {
        s.config(|c| c.open_eof_check.set(on));
    }
    if let Some(on) = s.truth(c"open_eof_split") {
        s.config(|c| {
            c.protocol.update(|p| p.split_by_eof = on);
            c.open_eof_check.set(c.open_eof_check.get() | on);
        });
    }
    if let Some(eof) = s.bytes(c"package_eof")? {
        set_eof(s, &eof)?;
    }
    if let Some(on) = s.truth(c"open_http_protocol") {
        s.config(|c| c.open_http_protocol.set(on));
    }
    if let Some(on) = s.truth(c"open_websocket_protocol") {
        s.config(|c| {
            c.open_websocket_protocol.set(on);
            c.open_http_protocol.set(c.open_http_protocol.get() | on);
        });
    }
    Ok(())
}

/// The options after the WebSocket ones.
fn framing(s: &impl Source) -> Step {
    if let Some(on) = s.truth(c"open_http2_protocol") {
        s.config(|c| {
            c.open_http2_protocol.set(on);
            c.open_http_protocol.set(c.open_http_protocol.get() | on);
        });
    }
    if let Some(on) = s.truth(c"open_mqtt_protocol") {
        s.config(|c| c.open_mqtt_protocol.set(on));
    }
    if let Some(v) = s.long(c"open_redis_protocol")? {
        s.config(|c| c.open_redis_protocol.set(v != 0));
    }
    if let Some(v) = s.double(c"max_idle_time")? {
        // SW_MAX: a NaN takes the minimum.
        let idle = if v > TIMER_MIN_SEC { v } else { TIMER_MIN_SEC };
        s.config(|c| c.max_idle_time.set(idle));
    }
    if let Some(v) = s.long(c"tcp_keepidle")? {
        s.config(|c| c.tcp_keepidle.set(i32_of(v)));
    }
    if let Some(v) = s.long(c"tcp_keepinterval")? {
        s.config(|c| c.tcp_keepinterval.set(i32_of(v)));
    }
    if let Some(v) = s.long(c"tcp_keepcount")? {
        s.config(|c| c.tcp_keepcount.set(i32_of(v)));
    }
    if let Some(v) = s.long(c"tcp_user_timeout")? {
        s.config(|c| c.tcp_user_timeout.set(i32_of(v)));
    }
    if let Some(on) = s.truth(c"tcp_fastopen") {
        s.config(|c| c.tcp_fastopen.set(on.into()));
    }
    if let Some(on) = s.truth(c"open_length_check") {
        s.config(|c| c.open_length_check.set(on));
    }
    if let Some(kind) = s.bytes(c"package_length_type")? {
        // `str_v.val()[0]`: an empty string's NUL.
        let kind = kind.first().copied().unwrap_or(0);
        let size = swoole_type_size(c_char::from_ne_bytes([kind])).c_cast::<u8>();
        protocol(s, |p| {
            p.package_length_type = kind;
            p.package_length_size = size;
        });
        if size == 0 {
            return Err(s.fatal(
                E_ERROR,
                c"unknown package_length_type, see pack(). Link: https://php.net/pack",
            ));
        }
    }
    if let Some(v) = s.long(c"package_length_offset")? {
        let error = c"'package_length_offset' value is too large";
        offset(s, v, |p| &mut p.package_length_offset, error)?;
    }
    let body = match s.long(c"package_body_offset")? {
        Some(v) => Some(v),
        None => s.long(c"package_body_start")?,
    };
    if let Some(v) = body {
        let error = c"'package_body_offset' value is too large";
        offset(s, v, |p| &mut p.package_body_offset, error)?;
    }
    if let Some((callable, owner)) = s.callable(c"package_length_func")? {
        let length_func = s.length_func();
        let previous = s.config(|c| {
            c.protocol.update(|p| {
                p.get_package_length = Some(length_func);
                p.private_data_1 = callable;
            });
            c.length_owner.replace(Some(owner))
        });
        if let Some(previous) = previous {
            s.release(previous);
        }
        protocol(s, |p| {
            p.package_length_size = 0;
            p.package_length_type = 0;
            p.package_length_offset = SW_IPC_BUFFER_SIZE.c_cast::<u16>();
        });
        s.single_thread();
    }
    if let Some(v) = s.size(c"package_max_length")? {
        protocol(s, |p| p.package_max_length = u32_of(v));
    }
    Ok(())
}

/// `php_swoole_server_set_websocket_option`: the frames the port passes on
/// (`open_websocket_{close,ping,pong}_frame` as bits 0, 1 and 2) and its
/// handshake's `Sec-WebSocket-Protocol`.
fn websocket(s: &impl Source) -> Step {
    for (key, bit) in [
        (c"open_websocket_close_frame", 1),
        (c"open_websocket_ping_frame", 2),
        (c"open_websocket_pong_frame", 4),
    ] {
        if let Some(on) = s.truth(key) {
            s.config(|c| {
                c.websocket_frames.set(if on {
                    c.websocket_frames.get() | bit
                } else {
                    c.websocket_frames.get() & !bit
                });
            });
        }
    }
    if let Some(protocol) = s.bytes(c"websocket_subprotocol")? {
        s.config(|c| c.websocket_protocol.set(protocol));
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
fn files(s: &impl Source, owner: &tls::Lent<'_>) -> Step {
    for (key, which, prefix) in [
        (c"ssl_cert_file", Text::CertFile, &b"ssl cert file["[..]),
        (c"ssl_key_file", Text::KeyFile, b"ssl key file["),
    ] {
        if let Some(file) = s.bytes(key)?
            && !owner.set_text(which, &file)
        {
            return Err(not_found(s, prefix, &file));
        }
    }
    Ok(())
}

/// The TLS options of an SSL port, then its SNI contexts and `ssl_init()`.
pub(crate) fn tls(s: &impl Source, owner: &OwnedTlsContext, dgram: bool) -> Step {
    // SAFETY: the port's context, which nothing else reaches during set().
    let owner = unsafe { tls::Lent::new(owner) };
    files(s, &owner)?;
    let set = |value| owner.set_value(value);
    let text = |which, value: &[u8]| owner.set_text(which, value);
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
    pub(crate) port: view::Port<'a>,
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

    fn callable(&self, key: &CStr) -> Result<Option<(*mut c_void, LengthOwner)>, Stop> {
        let Some(value) = self.settings.get(key) else {
            return Ok(None);
        };
        // Its bailouts stay deferred.
        let callable = crate::callable::create(value);
        if checked().is_err() {
            drop(callable);
            return Err(Stop);
        }
        // The port's owner now, which `release` (or the port's release) drops.
        Ok(callable.map(|callable| {
            let named = swoole_rs_phpswoole::length::Named::new(callable);
            let pointer = named.as_ptr();
            let owner: Box<dyn std::any::Any> = Box::new(named);
            (pointer, Box::new(owner))
        }))
    }

    fn release(&self, owner: LengthOwner) {
        // A thread-mode worker replaced the master's owner, whose callable other
        // threads' reactors, or this thread's running length function, may still be
        // calling: the master owns it, so it leaks, as the port's release does.
        if crate::lifecycle::owns_shared() {
            drop(owner);
        } else {
            #[expect(
                clippy::mem_forget,
                reason = "the master owns the callable other threads may still call"
            )]
            std::mem::forget(owner);
        }
    }

    fn single_thread(&self) {
        self.settings.server().config().single_thread.set(true);
    }

    fn length_func(&self) -> GetPackageLength {
        swoole_rs_phpswoole::length::length::<crate::dispatch::Length>
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
            let Some(vht) = value.array() else {
                return Err(self.fatal(E_WARNING, c"invalid SNI_cert setting"));
            };
            let context = self.config(context_dup);
            let item = Zend {
                settings: Settings {
                    serv: self.settings.serv,
                    vht: Some(vht),
                },
                port: self.port,
            };
            // SAFETY: the fresh duplicate, which nothing else reaches until `sni_add`
            // takes it.
            files(&item, &unsafe { tls::Lent::new(&context) })?;
            let added = self.config(|c| sni_add(c, name, context));
            if !added {
                return Err(self.fatal(E_ERROR, c"ssl_add_sni_cert() failed"));
            }
        }
        Ok(())
    }

    fn ssl_init(&self) -> bool {
        self.config(swoole_rs_ports::lifecycle::ssl_init)
    }

    fn config<R>(&self, f: impl FnOnce(&PortConfig) -> R) -> R {
        f(self.port.config())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::{Cell, RefCell},
        collections::HashMap,
        ptr::NonNull,
    };
    use swoole_rs_corebase::net_types::{PacketLength, Protocol};
    use swoole_rs_server::port_config::swr_v1_port_config_new;

    /// The mock settings' `package_length_func` callback.
    unsafe extern "C" fn length_func(
        _: *const Protocol,
        _: *mut c_void,
        _: &mut PacketLength,
    ) -> isize {
        11
    }

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
        /// The settings `apply` below hands over.
        config: Cell<Option<NonNull<PortConfig>>>,
        /// A `set()` of the same port that reading `tcp_keepidle` runs, as a PHP
        /// conversion can.
        nested: Option<Box<Table>>,
    }

    impl Table {
        fn with(mut self, key: &'static str, value: Fixture) -> Self {
            self.values.insert(key, value);
            self
        }

        fn get(&self, key: &CStr, kind: &str) -> Option<Fixture> {
            let key = key.to_str().unwrap();
            self.trace.borrow_mut().push(format!("{kind}:{key}"));
            if let Some(nested) = &self.nested
                && key == "tcp_keepidle"
            {
                nested.config.set(self.config.get());
                assert!(super::apply(&**nested).is_ok());
            }
            self.values.get(key).cloned()
        }
    }

    /// `super::apply` over `c`.
    fn apply(table: &Table, c: &mut PortConfig) -> Step {
        table.config.set(Some(NonNull::from(c)));
        super::apply(table)
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
        fn callable(&self, key: &CStr) -> Result<Option<(*mut c_void, LengthOwner)>, Stop> {
            Ok(self.get(key, "callable").map(|v| {
                let n = usize::try_from(v.number).unwrap();
                let owner: Box<dyn std::any::Any> = Box::new(n);
                (std::ptr::without_provenance_mut(n), Box::new(owner))
            }))
        }
        fn release(&self, owner: LengthOwner) {
            let n = owner.downcast_ref::<usize>().unwrap();
            self.trace.borrow_mut().push(format!("release:{n}"));
        }
        fn single_thread(&self) {
            self.trace.borrow_mut().push("single_thread".into());
        }
        fn length_func(&self) -> GetPackageLength {
            length_func
        }
        fn sni(&self) -> Step {
            self.trace.borrow_mut().push("sni".into());
            Ok(())
        }
        fn ssl_init(&self) -> bool {
            self.trace.borrow_mut().push("ssl_init".into());
            true
        }
        fn config<R>(&self, f: impl FnOnce(&PortConfig) -> R) -> R {
            f(unsafe { self.config.get().unwrap().as_ref() })
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
        // Each clamp against SW_MAX(lo, SW_MIN(v, hi)).
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
        assert!(c.open_tcp_nodelay.get());
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
            assert_eq!(c.open_tcp_nodelay.get(), on);
            assert_eq!(c.open_tcp_keepalive.get(), on);
            assert_eq!(c.protocol.get_mut().split_by_eof, on);
            assert_eq!(
                c.open_eof_check.get(),
                on,
                "open_eof_split turns eof_check on"
            );
            assert_eq!(c.open_websocket_protocol.get(), on);
            assert_eq!(c.open_http2_protocol.get(), on);
            assert_eq!(
                c.open_http_protocol.get(),
                on,
                "websocket and http2 turn http on"
            );
            assert_eq!(c.open_mqtt_protocol.get(), on);
            assert_eq!(c.open_redis_protocol.get(), on);
            assert_eq!(c.tcp_fastopen.get(), c_int::from(on));
            assert_eq!(c.open_length_check.get(), on);
        }
        // A false dependent leaves the flag it would set as it was.
        let mut c = port();
        c.open_eof_check.set(true);
        c.open_http_protocol.set(true);
        let table = Table::default()
            .with("open_eof_split", fixture(0))
            .with("open_websocket_protocol", fixture(0))
            .with("open_http2_protocol", fixture(0));
        assert!(apply(&table, &mut c).is_ok());
        assert!(c.open_eof_check.get() && c.open_http_protocol.get());
        // open_redis_protocol is read as an integer.
        for v in [i64::MIN, -1, 0, 1, i64::MAX] {
            let mut c = port();
            let table = Table::default().with("open_redis_protocol", fixture(v));
            assert!(apply(&table, &mut c).is_ok());
            assert_eq!(c.open_redis_protocol.get(), v != 0);
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
            assert_eq!(c.backlog.get(), u16_of(v));
            assert_eq!(c.heartbeat_idle_time.get(), u16_of(v));
            for size in [
                c.socket_buffer_size.get(),
                c.kernel_socket_recv_buffer_size.get(),
                c.kernel_socket_send_buffer_size.get(),
            ] {
                assert_eq!(size, buffer(v));
            }
            assert_eq!(c.buffer_high_watermark.get(), u32_of(v));
            assert_eq!(c.buffer_low_watermark.get(), u32_of(v));
            assert_eq!(c.protocol.get_mut().package_max_length, u32_of(v));
            for n in [
                c.tcp_defer_accept.get(),
                c.tcp_keepidle.get(),
                c.tcp_keepinterval.get(),
                c.tcp_keepcount.get(),
                c.tcp_user_timeout.get(),
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
            assert_eq!(c.max_idle_time.get().to_bits(), expected.to_bits());
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
            assert_eq!(
                c.protocol.get_mut().package_eof_len,
                length.to_le_bytes()[0]
            );
            let trace = table.trace.borrow().join(";");
            match length {
                0 | 256 => assert!(trace.ends_with("fatal1:package_eof cannot be an empty string")),
                9 | 264 => assert!(trace.ends_with("fatal1:package_eof max length is 8")),
                _ => {
                    assert!(result.is_ok());
                    assert_eq!(&c.protocol.get_mut().package_eof[..length], &eof[..]);
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
                c.protocol.get_mut().package_length_type,
                kind.first().copied().unwrap_or(0)
            );
            assert_eq!(c.protocol.get_mut().package_length_size, width);
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
                    c.protocol.get_mut().package_length_offset
                } else {
                    c.protocol.get_mut().package_body_offset
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
        assert_eq!(c.protocol.get_mut().package_body_offset, 3);
    }

    #[test]
    fn package_length_func_replaces_the_framing() {
        let mut c = port();
        let previous: Box<dyn std::any::Any> = Box::new(7_usize);
        c.length_owner.set(Some(Box::new(previous)));
        let table = Table::default().with("package_length_func", fixture(9));
        assert!(apply(&table, &mut c).is_ok());
        assert_eq!(c.protocol.get_mut().private_data_1.addr(), 9);
        assert_eq!(c.protocol.get_mut().package_length_size, 0);
        assert_eq!(c.protocol.get_mut().package_length_type, 0);
        assert_eq!(
            usize::from(c.protocol.get_mut().package_length_offset),
            SW_IPC_BUFFER_SIZE
        );
        assert_eq!(
            c.protocol
                .get_mut()
                .get_package_length
                .map(|f| f as *const ()),
            Some(length_func as *const ())
        );
        let trace = table.trace.borrow().join(";");
        assert!(trace.contains("callable:package_length_func;release:7;single_thread"));
    }

    #[test]
    fn websocket_frames_and_subprotocol() {
        let mut c = port();
        c.websocket_frames.set(2);
        let table = Table::default()
            .with("open_websocket_close_frame", fixture(1))
            .with("open_websocket_ping_frame", fixture(0))
            .with("open_websocket_pong_frame", fixture(1))
            .with("websocket_subprotocol", text(b"chat"));
        assert!(apply(&table, &mut c).is_ok());
        assert_eq!(c.websocket_frames.get(), 5);
        assert_eq!(
            c.websocket_protocol.get().as_deref().map(Vec::as_slice),
            Some(&b"chat"[..])
        );
    }

    #[test]
    fn a_conversion_may_set_the_same_port() {
        // The nested set() lands mid-way; the outer one goes on writing after it.
        let nested = Table::default()
            .with("backlog", fixture(9))
            .with("package_max_length", fixture(3));
        let table = Table {
            nested: Some(Box::new(nested)),
            ..Table::default()
                .with("backlog", fixture(5))
                .with("tcp_keepidle", fixture(7))
        };
        let mut c = port();
        assert!(apply(&table, &mut c).is_ok());
        assert_eq!(c.backlog.get(), 9);
        assert_eq!(c.tcp_keepidle.get(), 7);
        assert_eq!(c.protocol.get_mut().package_max_length, 3);
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
        let fields = unsafe { &*prepared.fields };
        assert!(!fields.disable_compress, "ssl_compress is inverted");
        assert_eq!(fields.protocols, 4, "a datagram port drops DTLS");
        assert!(fields.verify_peer && fields.allow_self_signed);
        assert_eq!(fields.verify_depth, 255);
        assert!(fields.prefer_server_ciphers);
        let view = |index: usize| {
            // SAFETY: a slot of the live owner.
            let view =
                unsafe { swoole_rs_corebase::swr_v1_core_string_view(&*prepared.strings[index]) };
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
