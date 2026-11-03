//! `php_swoole_client_check_setting` and `php_swoole_client_check_ssl_setting`:
//! `Swoole\Client`'s and `Swoole\Async\Client`'s options, read here and written
//! into `network::Client` through `swoole_client_option` (the framing head
//! directly, through crates/framing's mirror of `swoole::Protocol`).
use crate::native::{self, Cli, ClientCall, ErrorKind, Op};
use crate::net::{bailed, report, warn, warn_sys};
use libc::{c_int, c_void};
use std::ptr;
use swoole_rs_framing::Protocol;
use swoole_rs_phptable::{self as table, Owned};
use swoole_rs_zend::{
    E_ERROR, bailout,
    sys::{IS_STRING, zend_ini_parse_quantity, zval},
    type_of, value,
};

/// `SW_DATA_EOF_MAXLEN`.
const EOF_MAX: usize = 8;
/// `SW_INPUT_BUFFER_SIZE`.
const INPUT_BUFFER_SIZE: i64 = 2 * 1024 * 1024;

const EOF_CHECK: c_int = 0;
const LENGTH_CHECK: c_int = 1;
const MQTT: c_int = 2;
const LENGTH_FUNC: c_int = 3;
const BUFFER_SIZE: c_int = 4;
const HIGH_WATERMARK: c_int = 5;
const LOW_WATERMARK: c_int = 6;
const BIND: c_int = 7;
const TCP_NODELAY: c_int = 8;
const OPEN_SSL: c_int = 9;
const SSL_PROTOCOLS: c_int = 10;
const SSL_DISABLE_COMPRESS: c_int = 11;
const SSL_CERT_FILE: c_int = 12;
const SSL_KEY_FILE: c_int = 13;
const SSL_PASSPHRASE: c_int = 14;
const SSL_HOST_NAME: c_int = 15;
const SSL_VERIFY_PEER: c_int = 16;
const SSL_ALLOW_SELF_SIGNED: c_int = 17;
const SSL_CAFILE: c_int = 18;
const SSL_CAPATH: c_int = 19;
const SSL_VERIFY_DEPTH: c_int = 20;
const SSL_CIPHERS: c_int = 21;
const SSL_FILES: c_int = 22;

/// The options array of one `set()`/`connect()`.
struct Options(*const zval);

impl Options {
    fn get(&self, key: &str) -> Option<*mut zval> {
        // SAFETY: a live array zval for the call.
        unsafe { table::get(self.0, key.as_bytes()) }
    }

    fn truthy(&self, key: &str) -> Option<bool> {
        // SAFETY: a value found in the array.
        self.get(key).map(|found| unsafe { table::truthy(found) })
    }

    fn long(&self, key: &str) -> Option<i64> {
        // SAFETY: as above.
        self.get(key).map(|found| unsafe { table::long(found) })
    }

    /// `zend::String(value)`.
    fn string(&self, key: &str) -> Option<Owned> {
        // SAFETY: as above; a new reference, released on drop.
        self.get(key).map(|found| unsafe { Owned::of(found) })
    }

    /// `php_swoole_parse_to_size`.
    fn size(&self, key: &str) -> Option<i64> {
        self.get(key).map(parse_to_size)
    }
}

/// `php_swoole_parse_to_size(value)`.
fn parse_to_size(value: *mut zval) -> i64 {
    // SAFETY: a live zval.
    if unsafe { type_of(value) } != IS_STRING {
        // SAFETY: as above.
        return unsafe { table::long(value) };
    }
    // SAFETY: a string zval holds a live string.
    let string = unsafe { value::str_of(value) };
    let mut error = ptr::null_mut();
    // SAFETY: a live string; Zend writes an error string or leaves null.
    let size = unsafe { zend_ini_parse_quantity(string, &raw mut error) };
    if !error.is_null() {
        // SAFETY: Zend's new error string, released on drop.
        let error = unsafe { Owned::from_raw(error) };
        // SAFETY: the live string above.
        let text = c_text(unsafe { swoole_rs_zend::string::bytes(string) });
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
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

fn clamp(value: i64, low: i64, high: i64) -> i64 {
    value.min(high).max(low)
}

fn option(cli: Cli, option: c_int, value: i64) -> i64 {
    // SAFETY: a live client; the option reads no text.
    unsafe { native::swoole_client_option(cli, option, value, ptr::null(), 0) }
}

fn text_option(cli: Cli, option: c_int, value: i64, text: &[u8]) -> i64 {
    // SAFETY: a live client; C++ copies the text during the call.
    unsafe { native::swoole_client_option(cli, option, value, text.as_ptr().cast(), text.len()) }
}

/// `php_swoole_client_check_setting(cli, settings)`: false on an invalid
/// option, None after a bailout.
pub(crate) fn apply(cli: Cli, settings: *const zval) -> Option<bool> {
    let options = Options(settings);
    // SAFETY: a live client.
    let protocol = unsafe { native::swoole_client_protocol(cli) }.cast::<Protocol>();
    // SAFETY: the client's protocol, which nothing else touches meanwhile.
    let protocol = unsafe { &mut *protocol };
    if let Some(on) = options.truthy("open_eof_check") {
        option(cli, EOF_CHECK, i64::from(on));
    }
    if let Some(on) = options.truthy("open_eof_split") {
        protocol.split_by_eof = u8::from(on);
        if on {
            option(cli, EOF_CHECK, 1);
        }
    }
    if let Some(eof) = options.string("package_eof") {
        let eof = eof.bytes();
        #[expect(clippy::cast_possible_truncation, reason = "upstream's uint8_t length")]
        let length = eof.len() as u8;
        protocol.package_eof_len = length;
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
        protocol.package_eof[..length].copy_from_slice(&eof[..length]);
    }
    if let Some(on) = options.truthy("open_mqtt_protocol") {
        option(cli, MQTT, i64::from(on));
    }
    if let Some(on) = options.truthy("open_length_check") {
        option(cli, LENGTH_CHECK, i64::from(on));
    }
    if let Some(kind) = options.string("package_length_type") {
        let kind = kind.bytes().first().copied().unwrap_or(0);
        protocol.package_length_type = kind;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "sizes up to 8"
        )]
        let size = swoole_rs_corebase::swr_v1_corebase_type_size(kind) as u8;
        protocol.package_length_size = size;
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
        protocol.package_length_offset = clamp(offset, 0, u16::MAX.into()) as u16;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped"
    )]
    if let Some(offset) = options.long("package_body_offset") {
        protocol.package_body_offset = clamp(offset, 0, u16::MAX.into()) as u16;
    }
    if let Some(function) = options.get("package_length_func") {
        let mut create = ClientCall::new(ptr::null_mut(), Op::Callable);
        create.settings = function;
        bailed(create.run())?;
        if create.buffer.is_null() {
            return Some(false);
        }
        let previous = option_pointer(cli, create.buffer.cast());
        if !previous.is_null() {
            bailed(ClientCall::new(previous, Op::CallableFree).run())?;
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
        protocol.package_max_length = clamp(max_length, 0, u32::MAX.into()) as u32;
    }
    if let Some(size) = options.size("socket_buffer_size") {
        option(cli, BUFFER_SIZE, clamp(size, 1, i32::MAX.into()));
    }
    if let Some(mark) = options.size("buffer_high_watermark") {
        option(cli, HIGH_WATERMARK, clamp(mark, 0, u32::MAX.into()));
    }
    if let Some(mark) = options.size("buffer_low_watermark") {
        option(cli, LOW_WATERMARK, clamp(mark, 0, u32::MAX.into()));
    }
    if bailout::pending() {
        return Some(false);
    }
    let bind_port = options
        .long("bind_port")
        .map_or(0, |port| clamp(port, 0, u16::MAX.into()));
    if let Some(address) = options.string("bind_address")
        && !address.bytes().is_empty()
        && text_option(cli, BIND, bind_port, address.bytes()) < 0
    {
        return Some(false);
    }
    if options.truthy("open_tcp_nodelay").unwrap_or(true) && option(cli, TCP_NODELAY, 0) == 0 {
        let error = crate::net::errno();
        // SAFETY: a live client with a socket.
        let fd = unsafe { native::swoole_client_fd(cli) };
        warn_sys(&format!("setsockopt({fd}, TCP_NODELAY) failed"), error);
    }
    if !proxies(cli, &options) {
        return Some(false);
    }
    if option(cli, OPEN_SSL, 0) != 0 {
        apply_ssl(cli, settings);
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

/// `SWOOLE_CLIENT_OPT_LENGTH_FUNC`: install `callable`, returning the previous one.
fn option_pointer(cli: Cli, callable: *const c_void) -> *mut c_void {
    // SAFETY: a live client; C++ keeps the callable, which it now owns.
    let previous = unsafe { native::swoole_client_option(cli, LENGTH_FUNC, 0, callable.cast(), 0) };
    ptr::with_exposed_provenance_mut(usize::try_from(previous).unwrap_or(0))
}

fn proxy(
    cli: Cli,
    http: bool,
    host: &Owned,
    port: i64,
    user: Option<&Owned>,
    password: Option<&Owned>,
) {
    let host = host.bytes();
    let user = user.map_or(&[][..], Owned::bytes);
    let password = password.map_or(&[][..], Owned::bytes);
    // SAFETY: a live client; C++ copies every string during the call.
    unsafe {
        native::swoole_client_proxy(
            cli,
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

/// `php_swoole_client_check_ssl_setting(cli, settings)`.
pub(crate) fn apply_ssl(cli: Cli, settings: *const zval) {
    let options = Options(settings);
    if let Some(protocols) = options.long("ssl_protocols") {
        option(cli, SSL_PROTOCOLS, protocols);
    }
    if let Some(compress) = options.truthy("ssl_compress") {
        option(cli, SSL_DISABLE_COMPRESS, i64::from(!compress));
    }
    for (key, file, name) in [
        ("ssl_cert_file", SSL_CERT_FILE, "cert"),
        ("ssl_key_file", SSL_KEY_FILE, "key"),
    ] {
        if let Some(path) = options.string(key)
            && text_option(cli, file, 0, path.bytes()) == 0
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
    let text = |key: &str, which: c_int| {
        if let Some(value) = options.string(key) {
            text_option(cli, which, 0, value.bytes());
        }
    };
    text("ssl_passphrase", SSL_PASSPHRASE);
    text("ssl_host_name", SSL_HOST_NAME);
    if let Some(on) = options.truthy("ssl_verify_peer") {
        option(cli, SSL_VERIFY_PEER, i64::from(on));
    }
    if let Some(on) = options.truthy("ssl_allow_self_signed") {
        option(cli, SSL_ALLOW_SELF_SIGNED, i64::from(on));
    }
    text("ssl_cafile", SSL_CAFILE);
    text("ssl_capath", SSL_CAPATH);
    if let Some(depth) = options.long("ssl_verify_depth") {
        option(cli, SSL_VERIFY_DEPTH, clamp(depth, 0, u8::MAX.into()));
    }
    text("ssl_ciphers", SSL_CIPHERS);
    match option(cli, SSL_FILES, 0) {
        1 => report(ErrorKind::Fatal, E_ERROR, "ssl require key file"),
        2 => report(ErrorKind::Fatal, E_ERROR, "ssl require cert file"),
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
