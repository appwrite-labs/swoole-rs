//! `php_swoole_client_check_setting` and `php_swoole_client_check_ssl_setting`:
//! `Swoole\Client`'s and `Swoole\Async\Client`'s options, read and written
//! into `network::Client`'s record and state here.
use crate::native::{self, Cli, ClientCall, ErrorKind, Op};
use crate::net::{self, bailed, report, warn, warn_sys};
use libc::{c_int, c_void};
use std::ptr;
use swoole_rs_clients::client::{ClientRecord, OPEN_SSL};
use swoole_rs_phptable::{self as table, Owned};
use swoole_rs_tls::{self as tls, Text, Value};
use swoole_rs_zend::{
    E_ERROR, bailout,
    sys::{IS_STRING, zend_ini_parse_quantity, zval},
    type_of, value,
};

/// `SW_DATA_EOF_MAXLEN`.
const EOF_MAX: usize = 8;
/// `SW_INPUT_BUFFER_SIZE`.
const INPUT_BUFFER_SIZE: i64 = 2 * 1024 * 1024;

/// `SW_IPC_BUFFER_SIZE`: the length offset a length function's protocol keeps.
const IPC_BUFFER_SIZE: u16 = 8192;

// The C++ framing functions a client's protocol points at.
unsafe extern "C" {
    /// `swoole::mqtt::set_protocol(protocol)`.
    #[link_name = "_ZN6swoole4mqtt12set_protocolEPNS_8ProtocolE"]
    fn mqtt_set_protocol(protocol: *mut c_void);
    /// `Protocol::default_length_func`.
    #[link_name = "_ZN6swoole8Protocol19default_length_funcEPKS0_PNS_7network6SocketEPNS_12PacketLengthE"]
    fn default_length_func();
    /// `php_swoole_length_func`: calls the callable in `private_data_1`.
    #[link_name = "_Z22php_swoole_length_funcPKN6swoole8ProtocolEPNS_7network6SocketEPNS_12PacketLengthE"]
    fn php_swoole_length_func();
}

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

fn record<'a>(cli: Cli) -> &'a mut ClientRecord {
    // SAFETY: a live client, whose record only this call touches meanwhile.
    unsafe { native::client(cli) }
}

fn state(cli: Cli) -> *mut native::ClientState {
    // SAFETY: a live client.
    unsafe { native::swoole_client_state(cli) }.cast()
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
        // SAFETY: a live client with a socket.
        let socket = unsafe { native::client_socket_mut(cli) };
        swoole_rs_reactor::swr_v1_socket_buffer_size(socket.fd, true, true, size);
        socket.output.buffer_size = size;
    }
    for (key, high) in [
        ("buffer_high_watermark", true),
        ("buffer_low_watermark", false),
    ] {
        if let Some(mark) = options.size(key) {
            // SAFETY: the live client's state.
            let slot =
                unsafe { swoole_rs_clients::client::swr_v1_clients_watermark(state(cli), high) };
            // SAFETY: its watermark, which lives as long.
            unsafe { *slot = clamp(mark, 0, u32::MAX.into()) as u32 };
        }
    }
}

/// `open_length_check = on`, with MQTT's framing when `mqtt` and on, else
/// the default length function.
fn length_check(cli: Cli, on: bool, mqtt: bool) {
    let record = record(cli);
    record.open_length_check = on;
    if !mqtt {
        record.protocol.get_package_length = default_length_func as *mut c_void;
    } else if on {
        // SAFETY: the client's own protocol.
        unsafe { mqtt_set_protocol((&raw mut record.protocol).cast()) };
    }
}

/// `bind(address, port)`, which logs a failed `SO_REUSEADDR` itself.
fn bind(cli: Cli, address: &[u8], port: i64) -> bool {
    unsafe extern "C" fn reuse_failed(fd: c_int) {
        swoole_rs_corebase::log::sys_warning(
            c"bind",
            format_args!("setsockopt({fd}, SO_REUSEADDR) failed"),
        );
    }
    let mut host = address.to_vec();
    host.push(0);
    #[expect(clippy::cast_possible_truncation, reason = "clamped to a port")]
    // SAFETY: a live client with a socket; a NUL-terminated host.
    let bound = unsafe {
        swoole_rs_clients::client::swr_v1_clients_bind(
            state(cli),
            host.as_ptr().cast(),
            address.len(),
            port as c_int,
            reuse_failed,
        )
    };
    bound >= 0
}

/// `php_swoole_client_check_setting(cli, settings)`: false on an invalid
/// option, None after a bailout.
pub(crate) fn apply(cli: Cli, settings: *const zval) -> Option<bool> {
    let options = Options(settings);
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
        record(cli).protocol.package_eof[..length].copy_from_slice(&eof[..length]);
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
    if options.truthy("open_tcp_nodelay").unwrap_or(true) {
        // SAFETY: a live client with a socket.
        let socket = unsafe { native::client_socket_mut(cli) };
        let tcp = matches!(
            i64::from(socket.socket_type),
            net::SOCK_TCP | net::SOCK_TCP6
        );
        if tcp && !socket.set_tcp_nodelay(1) {
            let error = net::errno();
            warn_sys(
                &format!("setsockopt({}, TCP_NODELAY) failed", socket.fd),
                error,
            );
        }
    }
    if !proxies(cli, &options) {
        return Some(false);
    }
    // SAFETY: the live client's state.
    if unsafe { swoole_rs_clients::client::swr_v1_clients_flag(state(cli), OPEN_SSL) } {
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

/// Install `callable` as the client's length function, returning the previous one.
fn option_pointer(cli: Cli, callable: *mut c_void) -> *mut c_void {
    let protocol = &mut record(cli).protocol;
    protocol.get_package_length = php_swoole_length_func as *mut c_void;
    protocol.package_length_size = 0;
    protocol.package_length_type = 0;
    protocol.package_length_offset = IPC_BUFFER_SIZE;
    std::mem::replace(&mut protocol.private_data_1, callable)
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

/// `php_swoole_client_check_ssl_setting(cli, settings)`, written into the
/// client's `SSLContext` (crates/tls).
pub(crate) fn apply_ssl(cli: Cli, settings: *const zval) {
    // SAFETY: a live client.
    let owner = unsafe { native::swoole_client_ssl_context(cli) }.cast::<tls::OwnedTlsContext>();
    if owner.is_null() {
        return;
    }
    // SAFETY: the live client's context, which nothing else writes during the call.
    let value = |value| unsafe { tls::set_value(owner, value) };
    let options = Options(settings);
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
