//! `php_swoole_client_check_setting` and `php_swoole_client_check_ssl_setting`:
//! `Swoole\Client`'s and `Swoole\Async\Client`'s options, read and written
//! into `network::Client`'s record and state here.
use crate::native::{self, Cli, ErrorKind};
use crate::net::{self, bailed, report, warn, warn_sys};
use libc::c_int;
use std::mem::ManuallyDrop;
use swoole_rs_clients::client::ClientRecord;
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::constants::SW_IPC_BUFFER_SIZE;
use swoole_rs_corebase::tls::{self as tls, Text, Value};
use swoole_rs_phpswoole::length::Named;
use swoole_rs_ports::framing_recv::default_length_func;
use swoole_rs_zend::host::callable::Callable;
use swoole_rs_zend::{Array, E_ERROR, Owned, Value as Zval, ZString, bailout};

/// `SW_DATA_EOF_MAXLEN`.
const EOF_MAX: usize = swoole_rs_corebase::constants::SW_DATA_EOF_MAXLEN as usize;
/// `SW_INPUT_BUFFER_SIZE`.
const INPUT_BUFFER_SIZE: i64 = swoole_rs_corebase::constants::SW_INPUT_BUFFER_SIZE as i64;

/// `php_swoole_length_func`: the packet length a client's `package_length_func`
/// returns for the bytes so far.
pub(crate) struct Length;

impl swoole_rs_phpswoole::length::PackageLength for Length {
    fn length(callable: &Callable, bytes: &[u8]) -> Option<usize> {
        package_length(callable, bytes)
    }
}

fn package_length(callable: &Callable, bytes: &[u8]) -> Option<usize> {
    // The call borrows the string, released when `data` drops.
    let data = Owned::string(ZString::new(bytes));
    // Released only once the call returned a result, as upstream releases it.
    let retval = ManuallyDrop::new(Owned::long(0));
    let called = callable.call(swoole_rs_zend::Zvals::one(&data), Some(&**retval), true);
    match called {
        Err(bailout) => {
            bailout.defer();
            None
        }
        Ok(false) => {
            warn(ErrorKind::Fatal, "length function handler error");
            None
        }
        Ok(true) => {
            let length = retval.long();
            // Our reference to the result: a destructor's bailout is deferred.
            drop(ManuallyDrop::into_inner(retval));
            usize::try_from(length).ok()
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
fn edit<R>(cli: Cli<'_>, edit: impl FnOnce(&mut ClientRecord) -> R) -> R {
    cli.get().edit_record(edit)
}

/// `socket_buffer_size` (`socket->set_buffer_size(size)`, then
/// `socket->buffer_size = size`) and the output buffer's watermarks.
fn buffers(cli: Cli<'_>, options: &Options) {
    if let Some(size) = options.size("socket_buffer_size") {
        let size = u32::try_from(clamp(size, 1, i32::MAX.into())).unwrap_or(1);
        if let Some(socket) = cli.net() {
            _ = swoole_rs_corebase::reactor::set_socket_buffer_size(socket.fd(), true, true, size);
            socket.edit(|record| record.output.buffer_size = size);
        }
    }
    for (key, high) in [
        ("buffer_high_watermark", true),
        ("buffer_low_watermark", false),
    ] {
        if let Some(mark) = options.size(key) {
            cli.get().set_watermark(
                high,
                u32::try_from(clamp(mark, 0, u32::MAX.into())).unwrap_or(0),
            );
        }
    }
}

/// `open_length_check = on`, with MQTT's framing when `mqtt` and on, else
/// the default length function.
fn length_check(cli: Cli<'_>, on: bool, mqtt: bool) {
    edit(cli, |record| {
        record.open_length_check = on;
        if !mqtt {
            record.protocol.get_package_length = Some(default_length_func);
        } else if on {
            swoole_rs_clients::mqtt::set_protocol(&mut record.protocol);
        }
    });
}

/// `bind(address, port)`, which logs a failed `SO_REUSEADDR` itself.
fn bind(cli: Cli<'_>, address: &[u8], port: i64) -> bool {
    let port = c_int::try_from(port).unwrap_or_default();
    cli.handle().bind(&crate::native::c_text(address), port)
}

/// `php_swoole_client_check_setting(cli, settings)`: false on an invalid
/// option, None after a bailout.
pub(crate) fn apply(cli: Cli<'_>, settings: &Array) -> Option<bool> {
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
        let size = swoole_rs_corebase::swoole_type_size(std::ffi::c_char::from_ne_bytes([kind]));
        // Sizes are up to 8; anything else is the unknown type's 0.
        let size = u8::try_from(size).unwrap_or_default();
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
    if let Some(offset) = options.long("package_length_offset") {
        let offset = u16::try_from(clamp(offset, 0, u16::MAX.into())).unwrap_or(0);
        edit(cli, |record| record.protocol.package_length_offset = offset);
    }
    if let Some(offset) = options.long("package_body_offset") {
        let offset = u16::try_from(clamp(offset, 0, u16::MAX.into())).unwrap_or(0);
        edit(cli, |record| record.protocol.package_body_offset = offset);
    }
    if let Some(function) = options.get("package_length_func") {
        let Some(callable) = bailed(native::owned_callable(function))? else {
            return Some(false);
        };
        let named = Named::new(callable);
        length_option(cli);
        bailed(native::free_owner(
            cli.get()
                .replace_length_owner(Some(std::rc::Rc::new(named))),
        ))?;
    }
    let max_length = options
        .size("package_max_length")
        .unwrap_or(INPUT_BUFFER_SIZE);
    let max = u32::try_from(clamp(max_length, 0, u32::MAX.into())).unwrap_or(0);
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
        let (tcp, fd) = socket.read(|record| (record.is_tcp(), record.fd));
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
fn proxies(cli: Cli<'_>, options: &Options) -> bool {
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

/// Point the client's length function at its `package_length_func`, whose owner the
/// client keeps.
fn length_option(cli: Cli<'_>) {
    edit(cli, |record| {
        let protocol = &mut record.protocol;
        protocol.get_package_length = Some(swoole_rs_phpswoole::length::length::<Length>);
        protocol.package_length_size = 0;
        protocol.package_length_type = 0;
        protocol.package_length_offset = SW_IPC_BUFFER_SIZE.c_cast::<u16>();
    });
}

fn proxy(
    cli: Cli<'_>,
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
/// a client's `SSLContext` (`corebase::tls`).
pub(crate) fn apply_ssl(context: &tls::TlsContext, settings: &Array) {
    let mut owner = context.borrow_configuration();
    let options = Options(settings);
    let text = |owner: &mut tls::Lent<'_>, key: &str, which| {
        if let Some(text) = options.string(key) {
            _ = owner.set_text(which, text.bytes());
        }
    };
    if let Some(protocols) = options.long("ssl_protocols") {
        owner.set_value(Value::Protocols(protocols));
    }
    if let Some(compress) = options.truthy("ssl_compress") {
        owner.set_value(Value::DisableCompress(!compress));
    }
    for (key, which, name) in [
        ("ssl_cert_file", Text::CertFile, "cert"),
        ("ssl_key_file", Text::KeyFile, "key"),
    ] {
        if let Some(path) = options.string(key)
            && !owner.set_text(which, path.bytes())
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
    text(&mut owner, "ssl_passphrase", Text::Passphrase);
    text(&mut owner, "ssl_host_name", Text::HostName);
    if let Some(on) = options.truthy("ssl_verify_peer") {
        owner.set_value(Value::VerifyPeer(on));
    }
    if let Some(on) = options.truthy("ssl_allow_self_signed") {
        owner.set_value(Value::AllowSelfSigned(on));
    }
    text(&mut owner, "ssl_cafile", Text::Cafile);
    text(&mut owner, "ssl_capath", Text::Capath);
    if let Some(depth) = options.long("ssl_verify_depth") {
        let depth = clamp(depth, 0, u8::MAX.into());
        owner.set_value(Value::VerifyDepth(u8::try_from(depth).unwrap_or(u8::MAX)));
    }
    text(&mut owner, "ssl_ciphers", Text::Ciphers);
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
