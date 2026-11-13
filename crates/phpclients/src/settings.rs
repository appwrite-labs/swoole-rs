//! `php_swoole_client_check_setting` and `php_swoole_client_check_ssl_setting`:
//! `Swoole\Client`'s and `Swoole\Async\Client`'s options, read and written
//! into `network::Client`'s record and state here.
use crate::native::{self, Cli, ErrorKind};
use crate::net::{self, bailed, report, warn, warn_sys};
use libc::c_int;
use std::mem::ManuallyDrop;
use swoole_rs_clients::client::ClientRecord;
use swoole_rs_corebase::CCast;
use swoole_rs_phpswoole::length::Named;
use swoole_rs_ports::framing_recv::default_length_func;
use swoole_rs_tls::{self as tls, Text, Value};
use swoole_rs_zend::host::callable::Callable;
use swoole_rs_zend::{Array, E_ERROR, Owned, Value as Zval, ZString, bailout};

/// `SW_DATA_EOF_MAXLEN`.
const EOF_MAX: usize = 8;
/// `SW_INPUT_BUFFER_SIZE`.
const INPUT_BUFFER_SIZE: i64 = 2 * 1024 * 1024;

/// `SW_IPC_BUFFER_SIZE`: the length offset a length function's protocol keeps.
const IPC_BUFFER_SIZE: u16 = 8192;

/// `php_swoole_length_func`: the packet length a client's `package_length_func`
/// returns for the bytes so far.
pub(crate) struct Length;

impl swoole_rs_phpswoole::length::PackageLength for Length {
    fn length(callable: &Callable, bytes: &[u8]) -> isize {
        package_length(callable, bytes)
    }
}

fn package_length(callable: &Callable, bytes: &[u8]) -> isize {
    // The call borrows the string, released when `data` drops.
    let data = Owned::string(ZString::new(bytes));
    // Released only once the call returned a result, as upstream releases it.
    let retval = ManuallyDrop::new(Owned::long(0));
    let called = callable.call(&mut [data.copy_value()], Some(&**retval), true);
    match called {
        Err(bailout) => {
            bailout.defer();
            -1
        }
        Ok(false) => {
            warn(ErrorKind::Fatal, "length function handler error");
            -1
        }
        Ok(true) => {
            let length = retval.long();
            // Our reference to the result: a destructor's bailout is deferred.
            drop(ManuallyDrop::into_inner(retval));
            isize::try_from(length).unwrap_or(-1)
        }
    }
}

/// The options array of one `set()`/`connect()`.
struct Options<'a>(&'a Array);

impl Options<'_> {
    fn get(&self, key: &str) -> Option<&Zval> {
        self.0.get(key.as_bytes())
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
    // Upstream reports the failure and still applies what Zend parsed.
    string.parse_quantity().unwrap_or_else(|(size, error)| {
        let text = c_text(string.bytes());
        let reason = c_text(error.bytes());
        report(
            ErrorKind::Fatal,
            E_ERROR,
            &format!("failed to parse '{text}' to size, Error: {reason}"),
        );
        size
    })
}

/// What `%s` prints of `bytes`: up to the first NUL.
fn c_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes.split(|&b| b == 0).next().unwrap_or_default()).into_owned()
}

fn clamp(value: i64, low: i64, high: i64) -> i64 {
    value.min(high).max(low)
}

/// Edit the client's public record, between its operations.
fn edit<R>(cli: Cli, edit: impl FnOnce(&mut ClientRecord) -> R) -> R {
    cli.get().edit_record(edit)
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
    edit(cli, |record| {
        record.open_length_check = on;
        if !mqtt {
            record.protocol.get_package_length = Some(default_length_func);
        } else if on {
            swoole_rs_protocols_ffi::set_protocol(&mut record.protocol);
        }
    });
}

/// `bind(address, port)`, which logs a failed `SO_REUSEADDR` itself.
fn bind(cli: Cli, address: &[u8], port: i64) -> bool {
    #[expect(clippy::cast_possible_truncation, reason = "clamped to a port")]
    let port = port as c_int;
    cli.handle().bind(&crate::native::c_text(address), port) >= 0
}

/// `php_swoole_client_check_setting(cli, settings)`: false on an invalid
/// option, None after a bailout.
pub(crate) fn apply(cli: Cli, settings: &Array) -> Option<bool> {
    let options = Options(settings);
    if let Some(on) = options.truthy("open_eof_check") {
        edit(cli, |record| record.open_eof_check = on);
    }
    if let Some(on) = options.truthy("open_eof_split") {
        edit(cli, |record| {
            record.protocol.split_by_eof = on;
            record.open_eof_check |= on;
        });
    }
    if let Some(eof) = options.string("package_eof") {
        let eof = eof.bytes();
        let length = eof.len().c_cast::<u8>();
        edit(cli, |record| record.protocol.package_eof_len = length);
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
        edit(cli, |record| {
            let package_eof = &mut record.protocol.package_eof;
            for (slot, byte) in package_eof.iter_mut().zip(eof).take(length) {
                *slot = *byte;
            }
        });
    }
    if let Some(on) = options.truthy("open_mqtt_protocol") {
        length_check(cli, on, true);
    }
    if let Some(on) = options.truthy("open_length_check") {
        length_check(cli, on, false);
    }
    if let Some(kind) = options.string("package_length_type") {
        let kind = kind.bytes().first().copied().unwrap_or(0);
        edit(cli, |record| record.protocol.package_length_type = kind);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "sizes up to 8"
        )]
        let size =
            swoole_rs_corebase::swoole_type_size(std::ffi::c_char::from_ne_bytes([kind])) as u8;
        edit(cli, |record| record.protocol.package_length_size = size);
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
        let offset = clamp(offset, 0, u16::MAX.into()) as u16;
        edit(cli, |record| record.protocol.package_length_offset = offset);
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped"
    )]
    if let Some(offset) = options.long("package_body_offset") {
        let offset = clamp(offset, 0, u16::MAX.into()) as u16;
        edit(cli, |record| record.protocol.package_body_offset = offset);
    }
    if let Some(function) = options.get("package_length_func") {
        let Some(callable) = bailed(native::owned_callable(function))? else {
            return Some(false);
        };
        let named = Named::new(callable);
        length_option(cli, &named);
        bailed(native::free_owner(
            cli.get()
                .replace_length_owner(Some(std::rc::Rc::new(named))),
        ))?;
    }
    let max_length = options
        .size("package_max_length")
        .unwrap_or(INPUT_BUFFER_SIZE);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped"
    )]
    let max = clamp(max_length, 0, u32::MAX.into()) as u32;
    edit(cli, |record| record.protocol.package_max_length = max);
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
    // A reference of this call's own: the context outlives whatever the settings run.
    if let Some(owner) = cli.get().tls_context().cloned() {
        apply_ssl(&owner, settings);
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

/// Point the client's length function at `callable`, which the client then owns.
fn length_option(cli: Cli, named: &Named) {
    edit(cli, |record| {
        let protocol = &mut record.protocol;
        protocol.get_package_length = Some(swoole_rs_phpswoole::length::length::<Length>);
        protocol.package_length_size = 0;
        protocol.package_length_type = 0;
        protocol.package_length_offset = IPC_BUFFER_SIZE;
        protocol.private_data_1 = named.as_ptr();
    });
}

fn proxy(
    cli: Cli,
    http: bool,
    host: &ZString,
    port: i64,
    user: Option<&ZString>,
    password: Option<&ZString>,
) {
    let user = user.map_or(&[][..], ZString::bytes);
    let password = password.map_or(&[][..], ZString::bytes);
    swoole_rs_clients::client::set_proxy(cli.get(), http, host.bytes(), port, user, password);
}

/// `php_swoole_client_check_ssl_setting(cli, settings)`, written into `context`,
/// a client's `SSLContext` (crates/tls).
pub(crate) fn apply_ssl(context: &tls::TlsContext, settings: &Array) {
    // SAFETY: the context `context` keeps live, which nothing else reaches during the call.
    let owner = unsafe { tls::Lent::new(context.as_ptr()) };
    let value = |value| owner.set_value(value);
    let options = Options(settings);
    let set_text = |which, text: &[u8]| owner.set_text(which, text);
    let text = |key: &str, which| {
        if let Some(text) = options.string(key) {
            set_text(which, text.bytes());
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
            && !set_text(which, path.bytes())
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
    match tls::has_files(context) {
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
