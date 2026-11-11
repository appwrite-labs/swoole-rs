//! `php_swoole_client_check_setting` and `php_swoole_client_check_ssl_setting`:
//! `Swoole\Client`'s and `Swoole\Async\Client`'s options, read and written
//! into `network::Client`'s record and state here.
use crate::native::{self, Cli, ErrorKind};
use crate::net::{self, bailed, report, warn, warn_sys};
use libc::{c_int, c_void};
use std::ptr::{self, NonNull};
use swoole_rs_clients::client::ClientRecord;
use swoole_rs_corebase::net_types::{PacketLength, Protocol};
use swoole_rs_ports::framing_recv::default_length_func;
use swoole_rs_tls::{self as tls, Text, Value};
use swoole_rs_zend::{
    Array, E_ERROR, Value as Zval, ZString, bailout, long_zval, string,
    sys::{self, zend_ini_parse_quantity, zval, zval_ptr_dtor},
    value,
};

/// `SW_DATA_EOF_MAXLEN`.
const EOF_MAX: usize = 8;
/// `SW_INPUT_BUFFER_SIZE`.
const INPUT_BUFFER_SIZE: i64 = 2 * 1024 * 1024;

/// `SW_IPC_BUFFER_SIZE`: the length offset a length function's protocol keeps.
const IPC_BUFFER_SIZE: u16 = 8192;

/// `php_swoole_length_func`: the packet length the PHP callable in the
/// protocol's `private_data_1` returns for the bytes so far, or -1. A bailout
/// is re-raised as the last step.
/// # Safety
/// C++ calls it with a live protocol whose `private_data_1` is a
/// callable, and a live packet of `buf_size` bytes.
pub(crate) unsafe extern "C" fn php_swoole_length_func(
    protocol: *const Protocol,
    _socket: *mut c_void,
    packet: &mut PacketLength,
) -> isize {
    // SAFETY: per the caller.
    let callable = unsafe { (*protocol).private_data_1 };
    // SAFETY: as above.
    let bytes = unsafe { std::slice::from_raw_parts(packet.buf, packet.buf_size as usize) };
    let length = package_length(callable, bytes);
    bailout::finish();
    length
}

fn package_length(callable: *mut c_void, bytes: &[u8]) -> isize {
    let mut data = long_zval(0);
    // SAFETY: a fresh string; the zval takes its reference.
    unsafe { value::set_str(&raw mut data, string::copy(bytes)) };
    let mut retval = long_zval(0);
    let called = native::invoke(callable, std::slice::from_mut(&mut data), &raw mut retval);
    let length = match called {
        Err(bailout) => {
            bailout.defer();
            -1
        }
        Ok(false) => {
            warn(ErrorKind::Fatal, "length function handler error");
            -1
        }
        Ok(true) => {
            let length = Zval::of(&mut retval).long();
            // SAFETY: our reference to the result, released inside a trap.
            bailout::defer(unsafe {
                bailout::trap(sys::swr_zend_zval_release, (&raw mut retval).cast())
            });
            isize::try_from(length).unwrap_or(-1)
        }
    };
    // SAFETY: our reference to the string.
    unsafe { zval_ptr_dtor(&raw mut data) };
    length
}

/// The options array of one `set()`/`connect()`.
struct Options<'a>(Option<&'a Array>);

impl Options<'_> {
    /// # Safety
    /// `settings` is null or a live zval for the call.
    unsafe fn new(settings: *const zval) -> Self {
        // SAFETY: per the caller.
        Self(unsafe { Zval::from_ptr(settings) }.and_then(Zval::array))
    }

    fn get(&self, key: &str) -> Option<&Zval> {
        self.0?.get(key.as_bytes())
    }

    fn truthy(&self, key: &str) -> Option<bool> {
        self.get(key).map(Zval::truthy)
    }

    fn long(&self, key: &str) -> Option<i64> {
        self.get(key).map(Zval::long)
    }

    /// `zend::String(value)`.
    fn string(&self, key: &str) -> Option<ZString> {
        self.get(key).map(Zval::string)
    }

    /// `php_swoole_parse_to_size`.
    fn size(&self, key: &str) -> Option<i64> {
        self.get(key).map(parse_to_size)
    }
}

/// `php_swoole_parse_to_size(value)`.
fn parse_to_size(value: &Zval) -> i64 {
    let Some(string) = value.str() else {
        return value.long();
    };
    let mut error = ptr::null_mut();
    // SAFETY: a live string; Zend writes an error string or leaves null.
    let size = unsafe { zend_ini_parse_quantity(string.as_ptr(), &raw mut error) };
    if !error.is_null() {
        // SAFETY: Zend's new error string, released on drop.
        let error = unsafe { ZString::from_raw(error) };
        let text = c_text(string.bytes());
        let reason = c_text(error.bytes());
        report(
            ErrorKind::Fatal,
            E_ERROR,
            &format!("failed to parse '{text}' to size, Error: {reason}"),
        );
    }
    size
}

/// What `%s` prints of `bytes`: up to the first NUL.
fn c_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes.split(|&b| b == 0).next().unwrap_or_default()).into_owned()
}

fn clamp(value: i64, low: i64, high: i64) -> i64 {
    value.min(high).max(low)
}

fn record<'a>(cli: Cli) -> &'a mut ClientRecord {
    cli.record()
}

/// `socket_buffer_size` (`socket->set_buffer_size(size)`, then
/// `socket->buffer_size = size`) and the output buffer's watermarks.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped"
)]
fn buffers(cli: Cli, options: &Options) {
    if let Some(size) = options.size("socket_buffer_size") {
        let size = clamp(size, 1, i32::MAX.into()) as u32;
        if let Some(socket) = cli.net() {
            swoole_rs_reactor::swr_v1_socket_buffer_size(socket.fd(), true, true, size);
            socket.edit(|record| record.output.buffer_size = size);
        }
    }
    for (key, high) in [
        ("buffer_high_watermark", true),
        ("buffer_low_watermark", false),
    ] {
        if let Some(mark) = options.size(key) {
            cli.get()
                .set_watermark(high, clamp(mark, 0, u32::MAX.into()) as u32);
        }
    }
}

/// `open_length_check = on`, with MQTT's framing when `mqtt` and on, else
/// the default length function.
fn length_check(cli: Cli, on: bool, mqtt: bool) {
    let record = record(cli);
    record.open_length_check = on;
    if !mqtt {
        record.protocol.get_package_length = Some(default_length_func);
    } else if on {
        swoole_rs_protocols_ffi::set_protocol(&mut record.protocol);
    }
}

/// `bind(address, port)`, which logs a failed `SO_REUSEADDR` itself.
fn bind(cli: Cli, address: &[u8], port: i64) -> bool {
    #[expect(clippy::cast_possible_truncation, reason = "clamped to a port")]
    let port = port as c_int;
    cli.handle().bind(&crate::native::c_text(address), port) >= 0
}

/// `php_swoole_client_check_setting(cli, settings)`: false on an invalid
/// option, None after a bailout.
pub(crate) fn apply(cli: Cli, settings: *const zval) -> Option<bool> {
    // SAFETY: the caller's live settings.
    let options = unsafe { Options::new(settings) };
    if let Some(on) = options.truthy("open_eof_check") {
        record(cli).open_eof_check = on;
    }
    if let Some(on) = options.truthy("open_eof_split") {
        record(cli).protocol.split_by_eof = on;
        if on {
            record(cli).open_eof_check = true;
        }
    }
    if let Some(eof) = options.string("package_eof") {
        let eof = eof.bytes();
        #[expect(clippy::cast_possible_truncation, reason = "upstream's uint8_t length")]
        let length = eof.len() as u8;
        record(cli).protocol.package_eof_len = length;
        if length == 0 {
            report(
                ErrorKind::Fatal,
                E_ERROR,
                "package_eof cannot be an empty string",
            );
            return Some(false);
        } else if usize::from(length) > EOF_MAX {
            report(
                ErrorKind::Fatal,
                E_ERROR,
                &format!("package_eof max length is {EOF_MAX}"),
            );
            return Some(false);
        }
        let length = usize::from(length);
        let package_eof = &mut record(cli).protocol.package_eof;
        for (slot, byte) in package_eof.iter_mut().zip(eof).take(length) {
            *slot = *byte;
        }
    }
    if let Some(on) = options.truthy("open_mqtt_protocol") {
        length_check(cli, on, true);
    }
    if let Some(on) = options.truthy("open_length_check") {
        length_check(cli, on, false);
    }
    if let Some(kind) = options.string("package_length_type") {
        let kind = kind.bytes().first().copied().unwrap_or(0);
        record(cli).protocol.package_length_type = kind;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "sizes up to 8"
        )]
        let size =
            swoole_rs_corebase::swoole_type_size(std::ffi::c_char::from_ne_bytes([kind])) as u8;
        record(cli).protocol.package_length_size = size;
        if size == 0 {
            let name = c_text(&[kind]);
            report(
                ErrorKind::Fatal,
                E_ERROR,
                &format!(
                    "Unknown package_length_type name '{name}', see pack(). Link: https://php.net/pack"
                ),
            );
            return Some(false);
        }
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped"
    )]
    if let Some(offset) = options.long("package_length_offset") {
        record(cli).protocol.package_length_offset = clamp(offset, 0, u16::MAX.into()) as u16;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped"
    )]
    if let Some(offset) = options.long("package_body_offset") {
        record(cli).protocol.package_body_offset = clamp(offset, 0, u16::MAX.into()) as u16;
    }
    if let Some(function) = options.get("package_length_func") {
        let callable = bailed(native::callable(function.as_ptr()))?;
        if callable.is_null() {
            return Some(false);
        }
        let previous = option_pointer(cli, callable);
        if !previous.is_null() {
            bailed(native::callable_free(previous))?;
        }
    }
    let max_length = options
        .size("package_max_length")
        .unwrap_or(INPUT_BUFFER_SIZE);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped"
    )]
    {
        record(cli).protocol.package_max_length = clamp(max_length, 0, u32::MAX.into()) as u32;
    }
    buffers(cli, &options);
    if bailout::pending() {
        return Some(false);
    }
    let bind_port = options
        .long("bind_port")
        .map_or(0, |port| clamp(port, 0, u16::MAX.into()));
    if let Some(address) = options.string("bind_address")
        && !address.bytes().is_empty()
        && !bind(cli, address.bytes(), bind_port)
    {
        return Some(false);
    }
    if options.truthy("open_tcp_nodelay").unwrap_or(true)
        && let Some(socket) = cli.net()
    {
        let (kind, fd) = socket.read(|record| (i64::from(record.socket_type), record.fd));
        let tcp = matches!(kind, net::SOCK_TCP | net::SOCK_TCP6);
        if tcp && !socket.edit(|record| record.set_tcp_nodelay(1)) {
            let error = net::errno();
            warn_sys(&format!("setsockopt({fd}, TCP_NODELAY) failed"), error);
        }
    }
    if !proxies(cli, &options) {
        return Some(false);
    }
    if let Some(owner) = cli.handle().ssl_context() {
        apply_ssl(owner, settings);
    }
    Some(true)
}

/// The SOCKS5 or HTTP proxy options: false when the port is missing.
fn proxies(cli: Cli, options: &Options) -> bool {
    if let Some(host) = options.string("socks5_host") {
        let Some(port) = options.long("socks5_port") else {
            warn(ErrorKind::Fatal, "socks5_port should not be null");
            return false;
        };
        let user = options.string("socks5_username");
        let password = options.string("socks5_password");
        proxy(cli, false, &host, port, user.as_ref(), password.as_ref());
    } else if let Some(host) = options.string("http_proxy_host") {
        let Some(port) = options.long("http_proxy_port") else {
            warn(ErrorKind::Fatal, "http_proxy_port should not be null");
            return false;
        };
        let user = options
            .string("http_proxy_username")
            .or_else(|| options.string("http_proxy_user"));
        let password = options.string("http_proxy_password");
        proxy(cli, true, &host, port, user.as_ref(), password.as_ref());
    }
    true
}

/// Install `callable` as the client's length function, returning the previous one.
fn option_pointer(cli: Cli, callable: *mut c_void) -> *mut c_void {
    let protocol = &mut record(cli).protocol;
    protocol.get_package_length = Some(php_swoole_length_func);
    protocol.package_length_size = 0;
    protocol.package_length_type = 0;
    protocol.package_length_offset = IPC_BUFFER_SIZE;
    std::mem::replace(&mut protocol.private_data_1, callable)
}

fn proxy(
    cli: Cli,
    http: bool,
    host: &ZString,
    port: i64,
    user: Option<&ZString>,
    password: Option<&ZString>,
) {
    let host = host.bytes();
    let user = user.map_or(&[][..], ZString::bytes);
    let password = password.map_or(&[][..], ZString::bytes);
    // SAFETY: a live client; the proxy copies every string during the call.
    unsafe {
        swoole_rs_clients::client::swr_v1_clients_proxy(
            cli.0,
            http,
            host.as_ptr().cast(),
            host.len(),
            port,
            user.as_ptr().cast(),
            user.len(),
            password.as_ptr().cast(),
            password.len(),
        );
    }
}

/// `php_swoole_client_check_ssl_setting(cli, settings)`, written into `owner`,
/// a client's live `SSLContext` (crates/tls).
pub(crate) fn apply_ssl(owner: NonNull<tls::OwnedTlsContext>, settings: *const zval) {
    let owner = owner.as_ptr().cast_const();
    // SAFETY: the live client's context, which nothing else writes during the call.
    let value = |value| unsafe { tls::set_value(owner, value) };
    // SAFETY: the caller's live settings.
    let options = unsafe { Options::new(settings) };
    let text = |key: &str, which| {
        if let Some(text) = options.string(key) {
            // SAFETY: as above.
            unsafe { tls::set_text(owner, which, text.bytes()) };
        }
    };
    if let Some(protocols) = options.long("ssl_protocols") {
        value(Value::Protocols(protocols));
    }
    if let Some(compress) = options.truthy("ssl_compress") {
        value(Value::DisableCompress(!compress));
    }
    for (key, which, name) in [
        ("ssl_cert_file", Text::CertFile, "cert"),
        ("ssl_key_file", Text::KeyFile, "key"),
    ] {
        if let Some(path) = options.string(key)
            // SAFETY: as above.
            && !unsafe { tls::set_text(owner, which, path.bytes()) }
        {
            let path = c_text(path.bytes());
            report(
                ErrorKind::Fatal,
                E_ERROR,
                &format!("ssl {name} file[{path}] not found"),
            );
            return;
        }
    }
    text("ssl_passphrase", Text::Passphrase);
    text("ssl_host_name", Text::HostName);
    if let Some(on) = options.truthy("ssl_verify_peer") {
        value(Value::VerifyPeer(on));
    }
    if let Some(on) = options.truthy("ssl_allow_self_signed") {
        value(Value::AllowSelfSigned(on));
    }
    text("ssl_cafile", Text::Cafile);
    text("ssl_capath", Text::Capath);
    if let Some(depth) = options.long("ssl_verify_depth") {
        let depth = clamp(depth, 0, u8::MAX.into());
        value(Value::VerifyDepth(u8::try_from(depth).unwrap_or(u8::MAX)));
    }
    text("ssl_ciphers", Text::Ciphers);
    // SAFETY: as above.
    match unsafe { tls::has_files(owner) } {
        (true, false) => report(ErrorKind::Fatal, E_ERROR, "ssl require key file"),
        (false, true) => report(ErrorKind::Fatal, E_ERROR, "ssl require cert file"),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::{c_text, clamp};

    #[test]
    fn c_text_stops_at_nul() {
        assert_eq!(c_text(b"ab\0cd"), "ab");
        assert_eq!(c_text(b"\0"), "");
        assert_eq!(c_text(b"abc"), "abc");
    }

    #[test]
    fn clamp_bounds_both_sides() {
        assert_eq!(clamp(-5, 0, 10), 0);
        assert_eq!(clamp(50, 0, 10), 10);
        assert_eq!(clamp(7, 0, 10), 7);
    }
}
