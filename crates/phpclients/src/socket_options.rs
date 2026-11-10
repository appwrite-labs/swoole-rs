//! `Coroutine\Socket`'s option surface: `getOption`, `setOption`, `import` and
//! `swoole_coroutine_socketpair()`, and
//! the settings arrays every coroutine client applies to its socket
//! (`php_swoole_socket_set`, `_set_protocol` and `_set_ssl`). Each reads its
//! arguments in upstream's order and stops where upstream's fatal error would;
//! the coroutine socket's fields stay C++, reached through `native`.
use crate::{
    multicast,
    native::{self, Co, ErrorKind},
    net::{self, warn_sys},
    socket::open,
};
use libc::{c_char, c_int, c_void, socklen_t};
use std::{
    ffi::{CStr, CString},
    mem::size_of,
    ptr,
    sync::atomic::Ordering,
};
use swoole_rs_corebase::net_types::Protocol;
use swoole_rs_tls::{self as tls, OwnedTlsContext, Text, Value};
use swoole_rs_zend::{
    Call,
    bailout::{self, Bailout},
    php_error, set_object,
    string::bytes,
    sys::{
        _zend_expected_type_Z_EXPECTED_RESOURCE, E_ERROR, E_WARNING, HashTable, IS_ARRAY, IS_NULL,
        IS_OBJECT, IS_RESOURCE, IS_STRING, ZPP_ERROR_WRONG_ARG, php_stream, swr_zend_is_true,
        swr_zend_object_properties, swr_zend_object_release, swr_zend_stream_socketd,
        swr_zend_stream_unbuffer, swr_zend_string_release, swr_zend_zval_value_name,
        zend_argument_type_error, zend_argument_value_error, zend_hash_next_index_insert,
        zend_hash_str_find, zend_ini_parse_quantity, zend_object, zend_string,
        zend_wrong_parameter_error, zval,
    },
    throw_exception, type_of,
    value::{self, get_double, get_long, get_string, table},
};

/// `SW_TIMEOUT_*`.
const TIMEOUT_CONNECT: c_int = 1 << 1;
const TIMEOUT_READ: c_int = 1 << 2;
const TIMEOUT_WRITE: c_int = 1 << 3;
const TIMEOUT_ALL: c_int = 0b1111;
/// `SW_SOCK_TCP` and `SW_SOCK_TCP6` .. `SW_SOCK_UDP6`.
const SOCK_TCP: c_int = 1;
const SOCK_UDP: c_int = 2;
const SOCK_TCP6: c_int = 3;
const SOCK_UDP6: c_int = 4;
/// `SW_DATA_EOF_MAXLEN`.
const EOF_MAXLEN: usize = 8;
/// `SW_ERROR_OPERATION_NOT_SUPPORT`.
const OPERATION_NOT_SUPPORT: i64 = 507;
/// `socket_settings`'s `which`.
pub(crate) const ALL: c_int = 0;
pub(crate) const PROTOCOL: c_int = 1;
pub(crate) const SSL: c_int = 2;
/// `swoole_socket_coro_set_flag`'s flags.
const FLAG_LENGTH: c_int = 0;
const FLAG_EOF: c_int = 1;
const FLAG_HTTP2: c_int = 2;

/// `swoole_socket_coro_ssl_supports`'s optional settings.
const SSL_HOST_NAME: c_int = 0;
const SSL_GREASE: c_int = 1;

/// `PacketLength`.
#[repr(C)]
pub(crate) struct PacketLength {
    pub(crate) buf: *const u8,
    pub(crate) buf_size: u32,
    header_len: u32,
}

/// `open_fastcgi_protocol`'s length: the record's content and padding lengths
/// plus its 8-byte header, or -1 past 65535.
unsafe extern "C" fn fastcgi_length(
    _protocol: *const Protocol,
    _socket: *mut c_void,
    pl: *mut PacketLength,
) -> isize {
    const HEADER: u32 = 8;
    // SAFETY: the receive loop passes its live packet view.
    let pl = unsafe { &*pl };
    if pl.buf_size < HEADER {
        return 0;
    }
    // SAFETY: at least `HEADER` bytes are buffered.
    let p = unsafe { pl.buf.cast::<[u8; HEADER as usize]>().read() };
    fastcgi_record_length(p)
}

fn fastcgi_record_length(p: [u8; 8]) -> isize {
    let length = ((isize::from(p[4]) << 8) | isize::from(p[5])) + isize::from(p[6]);
    if length > 0xffff { -1 } else { length + 8 }
}

/// `swoole_type_size`: the byte width of a `pack()` length type, or 0.
fn type_size(kind: u8) -> u8 {
    match kind {
        b'c' | b'C' => 1,
        b's' | b'S' | b'n' | b'v' => 2,
        b'l' | b'L' | b'N' | b'V' => 4,
        b'q' | b'Q' | b'J' | b'P' => 8,
        _ => 0,
    }
}

/// A bailout is pending: upstream never ran past it.
pub(crate) struct Stop;

pub(crate) fn checked() -> Result<(), Stop> {
    if bailout::pending() {
        Err(Stop)
    } else {
        Ok(())
    }
}

/// `bytes` as C text, cut at a NUL as `%s` reads it.
pub(crate) fn text(bytes: &[u8]) -> CString {
    CString::new(bytes.split(|&b| b == 0).next().unwrap_or_default()).unwrap_or_default()
}

/// `php_swoole_fatal_error(level, "%s", message)`.
fn fatal(level: u32, message: &[u8]) -> Result<(), Stop> {
    let message = text(message);
    #[expect(clippy::cast_possible_wrap, reason = "E_* levels are small")]
    net::report(ErrorKind::Fatal, level as c_int, &message.to_string_lossy());
    checked()
}

/// The settings array.
struct Settings(*mut HashTable);

impl Settings {
    /// `php_swoole_array_get_value`: present and not null.
    fn get(&self, key: &CStr) -> Option<*mut zval> {
        // SAFETY: a live table and a NUL-terminated key.
        let found = unsafe { zend_hash_str_find(self.0, key.as_ptr(), key.count_bytes()) };
        // SAFETY: a found value is live.
        (!found.is_null() && unsafe { type_of(found) } != IS_NULL).then_some(found)
    }
}

fn truth(value: *mut zval) -> bool {
    // SAFETY: a live setting.
    unsafe { swr_zend_is_true(value) }
}

pub(crate) fn long(value: *mut zval) -> Result<i64, Stop> {
    // SAFETY: a live setting.
    let long = unsafe { get_long(value) };
    checked().map(|()| long)
}

fn double(value: *mut zval) -> Result<f64, Stop> {
    // SAFETY: a live setting.
    let double = unsafe { get_double(value) };
    checked().map(|()| double)
}

/// `zend::String(value)`'s bytes.
pub(crate) fn string(value: *mut zval) -> Result<Vec<u8>, Stop> {
    // SAFETY: a live setting; an owned reference, released below.
    let string = unsafe { get_string(value) };
    // SAFETY: live until released.
    let copy = unsafe { bytes(string) }.to_vec();
    // SAFETY: the reference taken above.
    unsafe { swr_zend_string_release(string) };
    checked().map(|()| copy)
}

/// `php_swoole_parse_to_size`.
fn size(value: *mut zval) -> Result<i64, Stop> {
    // SAFETY: a live setting.
    if unsafe { type_of(value) } != IS_STRING {
        return long(value);
    }
    let mut error: *mut zend_string = ptr::null_mut();
    // SAFETY: a string zval holds a live string.
    let string = unsafe { value::str_of(value) };
    // SAFETY: a live string; Zend leaves an error string or null.
    let size = unsafe { zend_ini_parse_quantity(string, &raw mut error) };
    if !error.is_null() {
        // SAFETY: both strings are live.
        let message = [
            b"failed to parse '".as_slice(),
            &text(unsafe { bytes(string) }).into_bytes(),
            b"' to size, Error: ",
            // SAFETY: as above.
            &text(unsafe { bytes(error) }).into_bytes(),
        ]
        .concat();
        let reported = fatal(E_ERROR, &message);
        // SAFETY: Zend's error string, owned here.
        unsafe { swr_zend_string_release(error) };
        reported?;
    }
    Ok(size)
}

/// The socket's `Protocol`, edited by `f`.
fn edit(socket: Co, f: impl FnOnce(&mut Protocol)) {
    f(&mut socket.record().protocol);
}

/// The settings `which` names applied to `socket` (`ALL`, `PROTOCOL`, `SSL`):
/// whether they all applied. Stops at a pending bailout, for the caller's
/// handler to raise.
pub(crate) fn apply(socket: Co, settings: *const zval, which: c_int) -> bool {
    // SAFETY: an array zval.
    let s = Settings(unsafe { table(settings) });
    let applied = match which {
        ALL => set(&s, socket),
        PROTOCOL => set_protocol(&s, socket),
        _ => set_ssl(&s, socket),
    };
    applied.unwrap_or(false)
}

/// [`apply`] for C++ callers, which raise no bailout of their own.
/// # Safety
/// `socket` is a live coroutine socket and `settings` a live array zval.
pub unsafe extern "C" fn socket_settings(
    socket: *mut c_void,
    settings: *const zval,
    which: c_int,
) -> bool {
    // SAFETY: per the caller.
    let applied = apply(unsafe { Co::new(socket) }, settings, which);
    bailout::finish();
    applied
}

/// `php_swoole_socket_set(socket, settings)`.
/// # Safety
/// As [`socket_settings`].
#[unsafe(export_name = "_Z21php_swoole_socket_setPN6swoole9coroutine6SocketEPK12_zval_struct")]
pub unsafe extern "C" fn php_swoole_socket_set(
    socket: native::Socket,
    settings: *const zval,
) -> bool {
    // SAFETY: per the caller.
    unsafe { socket_settings(socket, settings, ALL) }
}

/// `php_swoole_socket_set_protocol(socket, settings)`.
/// # Safety
/// As [`socket_settings`].
#[unsafe(export_name = "_Z30php_swoole_socket_set_protocolPN6swoole9coroutine6SocketEPK12_zval_struct")]
pub unsafe extern "C" fn php_swoole_socket_set_protocol(
    socket: native::Socket,
    settings: *const zval,
) -> bool {
    // SAFETY: per the caller.
    unsafe { socket_settings(socket, settings, PROTOCOL) }
}

/// `php_swoole_socket_set_protocol`.
fn set_protocol(s: &Settings, socket: Co) -> Result<bool, Stop> {
    let mut ret = true;
    if let Some(v) = s.get(c"open_ssl")
        && truth(v)
    {
        enable_ssl(socket);
    }
    if let Some(v) = s.get(c"open_http2_protocol") {
        socket.set_flag(FLAG_HTTP2, truth(v));
    }
    if !socket.ssl_context().is_null() && !set_ssl(s, socket)? {
        ret = false;
    }
    if let Some(v) = s.get(c"open_eof_check") {
        socket.set_flag(FLAG_EOF, truth(v));
    }
    if let Some(v) = s.get(c"open_eof_split") {
        let split = truth(v);
        edit(socket, |p| p.split_by_eof = split);
        if split {
            socket.set_flag(FLAG_EOF, true);
        }
    }
    if let Some(v) = s.get(c"package_eof") {
        let eof = string(v)?;
        if eof.is_empty() {
            fatal(E_ERROR, b"package_eof cannot be an empty string")?;
            ret = false;
        } else if eof.len() > EOF_MAXLEN {
            fatal(
                E_ERROR,
                format!("package_eof max length is {EOF_MAXLEN}").as_bytes(),
            )?;
            ret = false;
        } else {
            edit(socket, |p| {
                p.package_eof_len = u8::try_from(eof.len()).unwrap_or(0);
                for (slot, byte) in p.package_eof.iter_mut().zip(&eof) {
                    *slot = *byte;
                }
            });
        }
    }
    if let Some(v) = s.get(c"open_fastcgi_protocol") {
        socket.set_flag(FLAG_LENGTH, truth(v));
        edit(socket, |p| {
            p.package_length_size = 8;
            p.package_length_offset = 0;
            p.package_body_offset = 0;
            p.get_package_length = fastcgi_length as *mut c_void;
        });
    }
    if let Some(v) = s.get(c"open_mqtt_protocol") {
        let mqtt = truth(v);
        socket.set_flag(FLAG_LENGTH, mqtt);
        if mqtt {
            socket.protocol_preset(0);
        }
    }
    if let Some(v) = s.get(c"open_length_check") {
        socket.set_flag(FLAG_LENGTH, truth(v));
        socket.protocol_preset(1);
    }
    if let Some(v) = s.get(c"package_length_type") {
        let kind = string(v)?.first().copied().unwrap_or(0);
        let width = type_size(kind);
        edit(socket, |p| {
            p.package_length_type = kind;
            p.package_length_size = width;
        });
        if width == 0 {
            let message = [
                b"Unknown package_length_type name '".as_slice(),
                &[kind],
                b"', see pack(). Link: https://php.net/pack",
            ]
            .concat();
            fatal(E_WARNING, &message)?;
            ret = false;
        }
    }
    if let Some(v) = s.get(c"package_length_offset") {
        let offset = clamp_u16(long(v)?);
        edit(socket, |p| p.package_length_offset = offset);
    }
    if let Some(v) = s.get(c"package_body_offset") {
        let offset = clamp_u16(long(v)?);
        edit(socket, |p| p.package_body_offset = offset);
    }
    if let Some(v) = s.get(c"package_length_func") {
        let mut args = native::LengthCallable { socket, value: v };
        // SAFETY: C++ that may report a PHP error, with its arguments.
        bailout::defer(unsafe {
            bailout::trap(
                native::swoole_socket_coro_length_callable,
                (&raw mut args).cast(),
            )
        });
        checked()?;
    }
    if let Some(v) = s.get(c"package_max_length") {
        let max = u32::try_from(size(v)?.clamp(0, i64::from(u32::MAX))).unwrap_or(u32::MAX);
        edit(socket, |p| p.package_max_length = max);
    }
    Ok(ret)
}

fn clamp_u16(value: i64) -> u16 {
    u16::try_from(value.clamp(0, i64::from(u16::MAX))).unwrap_or(u16::MAX)
}

fn clamp_u8(value: i64) -> u8 {
    u8::try_from(value.clamp(0, i64::from(u8::MAX))).unwrap_or(u8::MAX)
}

/// `php_swoole_socket_set`.
fn set(s: &Settings, socket: Co) -> Result<bool, Stop> {
    let mut ret = true;
    for (key, kind) in [
        (c"timeout", TIMEOUT_ALL),
        (c"connect_timeout", TIMEOUT_CONNECT),
        (c"read_timeout", TIMEOUT_READ),
        (c"write_timeout", TIMEOUT_WRITE),
    ] {
        if let Some(v) = s.get(key) {
            socket.set_timeout(double(v)?, kind);
        }
    }
    let mut bind_port = 0;
    if let Some(v) = s.get(c"bind_port") {
        bind_port = i64::from(clamp_u16(long(v)?));
    }
    let mut bind_address = Vec::new();
    if let Some(v) = s.get(c"bind_address") {
        bind_address = string(v)?;
    }
    if !bind_address.is_empty() {
        ret &= socket.bind(&bind_address, bind_port);
    }
    if let Some(v) = s.get(c"socket_buffer_size") {
        let size = size(v)?;
        if size <= 0 {
            let message = format!("socket buffer size must be greater than 0, got {size}");
            fatal(E_WARNING, message.as_bytes())?;
            ret = false;
        } else {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "set_option takes an int, as upstream's"
            )]
            let size = size as c_int;
            let _ = socket.set_option(libc::SOL_SOCKET, libc::SO_RCVBUF, size)
                && socket.set_option(libc::SOL_SOCKET, libc::SO_SNDBUF, size);
        }
    }
    let kind = socket.kind();
    if let Some(v) = s.get(c"open_tcp_nodelay")
        // Upstream's test: every type but TCP6.
        && (kind == SOCK_TCP || kind != SOCK_TCP6)
    {
        let on = truth(v);
        let nodelay = c_int::from(on);
        if let Some(record) = socket.net()
            && swoole_rs_reactor::swr_v1_socket_tcp_nodelay(record.fd, nodelay)
        {
            record.tcp_nodelay = u8::from(on);
        }
    }
    if !set_protocol(s, socket)? {
        ret = false;
    }
    let inet = matches!(kind, SOCK_TCP | SOCK_UDP | SOCK_TCP6 | SOCK_UDP6);
    if let Some(host) = s.get(c"socks5_host") {
        if !inet {
            throw_unsupported(c"Only tcp socket supports socks5 proxy settings");
            return Ok(false);
        }
        let host = string(host)?;
        let Some(port) = s.get(c"socks5_port") else {
            fatal(E_WARNING, b"socks5_port should not be null")?;
            return Ok(false);
        };
        let port = long(port)?;
        let (mut user, mut password) = (Vec::new(), Vec::new());
        if let Some(v) = s.get(c"socks5_username") {
            user = string(v)?;
            match s.get(c"socks5_password") {
                Some(v) if !user.is_empty() => password = string(v)?,
                _ => {
                    fatal(E_WARNING, b"socks5_password should not be null")?;
                    ret = false;
                }
            }
        }
        socket.proxy(false, &host, port, &user, &password);
    } else if let Some(host) = s.get(c"http_proxy_host") {
        if !inet {
            throw_unsupported(c"Only tcp socket supports http proxy settings");
            return Ok(false);
        }
        let host = string(host)?;
        let Some(port) = s.get(c"http_proxy_port") else {
            fatal(E_WARNING, b"http_proxy_port should not be null")?;
            return Ok(false);
        };
        let port = long(port)?;
        let (mut user, mut password) = (Vec::new(), Vec::new());
        if let Some(v) = s
            .get(c"http_proxy_username")
            .or_else(|| s.get(c"http_proxy_user"))
        {
            user = string(v)?;
            match s.get(c"http_proxy_password") {
                Some(v) if !user.is_empty() => password = string(v)?,
                _ => {
                    fatal(E_WARNING, b"socks5_password should not be null")?;
                    ret = false;
                }
            }
        }
        socket.proxy(true, &host, port, &user, &password);
    }
    Ok(ret)
}

fn throw_unsupported(message: &CStr) {
    let exception = net::EXCEPTION_PARENT.load(Ordering::Acquire);
    // SAFETY: from a handler or a C++ method, with Swoole's exception class.
    unsafe { throw_exception(exception, message, OPERATION_NOT_SUPPORT) };
}

/// `php_swoole_socket_set_ssl`.
fn set_ssl(s: &Settings, socket: Co) -> Result<bool, Stop> {
    let owner = socket.ssl_context().cast::<OwnedTlsContext>();
    if owner.is_null() {
        return Ok(true);
    }
    // SAFETY: the live socket's context, which nothing else writes during the call.
    let set = |value| unsafe { tls::set_value(owner, value) };
    // SAFETY: as above.
    let set_text = |which, text: &[u8]| unsafe { tls::set_text(owner, which, text) };
    let text = |which, v| -> Result<bool, Stop> { Ok(set_text(which, &string(v)?)) };
    if let Some(v) = s.get(c"ssl_protocols") {
        set(Value::Protocols(long(v)?));
    }
    if let Some(v) = s
        .get(c"ssl_compress")
        .or_else(|| s.get(c"ssl_disable_compression"))
    {
        set(Value::DisableCompress(!truth(v)));
    }
    for (key, which, what) in [
        (c"ssl_cert_file", Text::CertFile, "cert"),
        (c"ssl_key_file", Text::KeyFile, "key"),
    ] {
        if let Some(v) = s.get(key) {
            let file = string(v)?;
            if !set_text(which, &file) {
                let message = [
                    format!("ssl {what} file[").as_bytes(),
                    &self::text(&file).into_bytes(),
                    b"] not found",
                ]
                .concat();
                fatal(E_WARNING, &message)?;
                return Ok(false);
            }
        }
    }
    // SAFETY: the live context.
    match unsafe { tls::has_files(owner) } {
        (true, false) => fatal(E_WARNING, b"ssl require key file")?,
        (false, true) => fatal(E_WARNING, b"ssl require cert file")?,
        _ => {}
    }
    if let Some(v) = s.get(c"ssl_passphrase") {
        text(Text::Passphrase, v)?;
    }
    if native::ssl_supports(SSL_HOST_NAME)
        && let Some(v) = s.get(c"ssl_host_name")
    {
        text(Text::HostName, v)?;
    }
    if let Some(v) = s.get(c"ssl_verify_peer") {
        set(Value::VerifyPeer(truth(v)));
    }
    if let Some(v) = s.get(c"ssl_allow_self_signed") {
        set(Value::AllowSelfSigned(truth(v)));
    }
    if let Some(v) = s.get(c"ssl_cafile") {
        text(Text::Cafile, v)?;
    }
    if let Some(v) = s.get(c"ssl_capath") {
        text(Text::Capath, v)?;
    }
    if let Some(v) = s.get(c"ssl_verify_depth") {
        set(Value::VerifyDepth(clamp_u8(long(v)?)));
    }
    if let Some(v) = s.get(c"ssl_ciphers") {
        text(Text::Ciphers, v)?;
    }
    if let Some(v) = s.get(c"ssl_ecdh_curve") {
        text(Text::EcdhCurve, v)?;
    }
    if native::ssl_supports(SSL_GREASE)
        && let Some(v) = s.get(c"ssl_grease")
    {
        set(Value::Grease(clamp_u8(long(v)?)));
    }
    Ok(true)
}

/// Turn a client socket's TLS on (`enable_ssl_encrypt`): false if it had it.
pub(crate) fn enable_ssl(socket: Co) -> bool {
    socket.enable_ssl()
}

/// The coroutine socket's descriptor, from its `network::Socket` record.
pub(crate) fn fd(socket: Co) -> c_int {
    socket.net().map_or(-1, |record| record.fd)
}

pub(crate) fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// `getsockopt` into `T`, with the length it reported.
pub(crate) fn getsockopt<T>(
    fd: c_int,
    level: c_int,
    name: c_int,
    out: &mut T,
) -> Option<socklen_t> {
    let mut length = socklen_t::try_from(size_of::<T>()).unwrap_or(0);
    // SAFETY: `out` is writable for `length` bytes.
    let status =
        unsafe { libc::getsockopt(fd, level, name, ptr::from_mut(out).cast(), &raw mut length) };
    (status == 0).then_some(length)
}

/// `getOption(int $level, int $opt_name): mixed`.
fn get_option(call: &Call) {
    let parsed = call
        .arity(2, 2)
        .and_then(|()| Ok((call.long(1, 0)?, call.long(2, 0)?)));
    let Ok((level, name)) = parsed else {
        return call.set_bool(false);
    };
    let Some(socket) = open(call) else { return };
    let fd = fd(socket);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream passes them as int"
    )]
    let (level_int, name_int) = (level as c_int, name as c_int);
    let failed = || {
        warn_sys(&format!("getsockopt({fd}, {level}, {name})"), errno());
        call.set_bool(false);
    };
    if level == i64::from(libc::IPPROTO_IP) && name == i64::from(libc::IP_MULTICAST_IF) {
        let mut address = libc::in_addr { s_addr: 0 };
        if getsockopt(fd, level_int, name_int, &mut address).is_none() {
            return failed();
        }
        return match multicast::interface_index(address) {
            Ok(Some(index)) => call.set_long(i64::from(index)),
            Ok(None) => call.set_bool(false),
            Err(Stop) => {}
        };
    } else if level == i64::from(libc::IPPROTO_IPV6) {
        match multicast::get_ipv6(socket, level_int, name_int, call.return_value()) {
            Ok(0) | Err(Stop) => return,
            Ok(-1) => return call.set_bool(false),
            Ok(_) => {}
        }
    }
    if name == i64::from(libc::SO_LINGER) {
        let mut linger = libc::linger {
            l_onoff: 0,
            l_linger: 0,
        };
        if getsockopt(fd, level_int, name_int, &mut linger).is_none() {
            return failed();
        }
        call.set_array();
        call.add_assoc_long(c"l_onoff", i64::from(linger.l_onoff));
        call.add_assoc_long(c"l_linger", i64::from(linger.l_linger));
        return;
    }
    if (name == i64::from(libc::SO_RCVTIMEO)) || (name == i64::from(libc::SO_SNDTIMEO)) {
        let kind = if name == i64::from(libc::SO_RCVTIMEO) {
            TIMEOUT_READ
        } else {
            TIMEOUT_WRITE
        };
        let timeout = socket.get_timeout(kind);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "(int) timeout, as upstream"
        )]
        let sec = timeout as i32;
        call.set_array();
        call.add_assoc_long(c"sec", i64::from(sec));
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the double to zend_long conversion upstream makes"
        )]
        call.add_assoc_long(c"usec", ((timeout - f64::from(sec)) * 1_000_000.0) as i64);
        return;
    }
    #[cfg(target_os = "linux")]
    if name_int == libc::TCP_INFO {
        // SAFETY: an all-zero `tcp_info` is a valid out value.
        let mut info: libc::tcp_info = unsafe { std::mem::zeroed() };
        if getsockopt(fd, libc::SOL_TCP, libc::TCP_INFO, &mut info).is_none() {
            return warn_sys(&format!("getsockopt({fd}, SOL_TCP, TCP_INFO)"), errno());
        }
        call.set_array();
        for (key, value) in native::tcp_fields(&info) {
            #[expect(clippy::cast_possible_wrap, reason = "upstream's (zend_long) cast")]
            call.add_assoc_long(key, value as i64);
        }
        return;
    }
    let mut other: c_int = 0;
    let Some(length) = getsockopt(fd, level_int, name_int, &mut other) else {
        return failed();
    };
    if length == 1 {
        let byte = other.to_ne_bytes()[0];
        call.set_long(i64::from(c_char::from_ne_bytes([byte])));
    } else {
        call.set_long(i64::from(other));
    }
}

/// The array (or object properties) of a `setOption` value, or `None` after
/// the type error upstream throws.
fn option_table(value: *mut zval, name: &str) -> Option<*mut HashTable> {
    // SAFETY: a live argument.
    match unsafe { type_of(value) } {
        // SAFETY: an array argument.
        IS_ARRAY => Some(unsafe { table(value) }),
        // SAFETY: an object argument.
        IS_OBJECT => Some(unsafe { swr_zend_object_properties(value) }),
        _ => {
            // SAFETY: a live argument.
            let given = unsafe { swr_zend_zval_value_name(value) };
            // SAFETY: Zend returns a static name.
            let given = unsafe { CStr::from_ptr(given) };
            let message = format!(
                "must be of type array|object when argument $opt_name is {name}, {} given",
                given.to_string_lossy()
            );
            let message = CString::new(message).unwrap_or_default();
            // SAFETY: from a handler; "%s" takes the message.
            unsafe { zend_argument_type_error(3, c"%s".as_ptr(), message.as_ptr()) };
            None
        }
    }
}

/// The two longs a `setOption` array holds under `keys`: `Err(false)` after
/// the missing-key warning, `Err(true)` when the call must stop (a bailout).
fn option_pair(table: *mut HashTable, keys: [&CStr; 2]) -> Result<[i64; 2], bool> {
    let mut found = [ptr::null_mut(); 2];
    for (slot, key) in found.iter_mut().zip(keys) {
        // SAFETY: a live table and a NUL-terminated key.
        *slot = unsafe { zend_hash_str_find(table, key.as_ptr(), key.count_bytes()) };
        if slot.is_null() {
            let message = format!("no key \"{}\" passed in optval", key.to_string_lossy());
            let message = CString::new(message).unwrap_or_default();
            #[expect(clippy::cast_possible_wrap, reason = "E_WARNING is 2")]
            // SAFETY: from a handler.
            bailout::defer(unsafe { php_error(E_WARNING as i32, &message) });
            return Err(bailout::pending());
        }
    }
    let mut values = [0; 2];
    for (value, item) in values.iter_mut().zip(found) {
        *value = long(item).map_err(|Stop| true)?;
    }
    Ok(values)
}

/// `setOption(int $level, int $opt_name, mixed $opt_value): bool`.
fn set_option(call: &Call) {
    let parsed = call
        .arity(3, 3)
        .and_then(|()| Ok((call.long(1, 0)?, call.long(2, 0)?, call.zval(3))));
    let Ok((level, name, value)) = parsed else {
        return call.set_bool(false);
    };
    let Some(socket) = open(call) else { return };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream passes them as int"
    )]
    let (level_int, name_int) = (level as c_int, name as c_int);
    let mut plain = false;
    if level == i64::from(libc::IPPROTO_IP) || level == i64::from(libc::IPPROTO_IPV6) {
        let result = if level == i64::from(libc::IPPROTO_IPV6) {
            multicast::set_ipv6(socket, level_int, name_int, value)
        } else {
            multicast::set_ip(socket, level_int, name_int, value)
        };
        let Ok(result) = result else { return };
        match result {
            1 => plain = true,
            0 => return call.set_bool(true),
            _ => return call.set_bool(false),
        }
    }
    let linger;
    let int;
    let (option, length): (*const c_void, usize) = if !plain && name == i64::from(libc::SO_LINGER) {
        let Some(table) = option_table(value, "SO_LINGER") else {
            return;
        };
        let [on, wait] = match option_pair(table, [c"l_onoff", c"l_linger"]) {
            Ok(pair) => pair,
            Err(true) => return,
            Err(false) => return call.set_bool(false),
        };
        for (value, key) in [(on, "l_onoff"), (wait, "l_linger")] {
            if !(0..=i64::from(u16::MAX)).contains(&value) {
                let message = format!("\"{key}\" must be between 0 and {}", u16::MAX);
                let message = CString::new(message).unwrap_or_default();
                // SAFETY: from a handler; "%s" takes the message.
                unsafe { zend_argument_value_error(3, c"%s".as_ptr(), message.as_ptr()) };
                return;
            }
        }
        linger = libc::linger {
            l_onoff: c_int::from(clamp_u16(on)),
            l_linger: c_int::from(clamp_u16(wait)),
        };
        ((&raw const linger).cast(), size_of::<libc::linger>())
    } else if !plain
        && (name == i64::from(libc::SO_RCVTIMEO) || name == i64::from(libc::SO_SNDTIMEO))
    {
        let read = name == i64::from(libc::SO_RCVTIMEO);
        let label = if read { "SO_RCVTIMEO" } else { "SO_SNDTIMEO" };
        let Some(table) = option_table(value, label) else {
            return;
        };
        let [sec, usec] = match option_pair(table, [c"sec", c"usec"]) {
            Ok(pair) => pair,
            Err(true) => return,
            Err(false) => return call.set_bool(false),
        };
        // `struct timeval`'s fields, as upstream assigns them.
        #[expect(clippy::cast_precision_loss, reason = "(double) tv_sec, as upstream")]
        let seconds = sec as f64;
        #[expect(clippy::cast_precision_loss, reason = "(double) tv_usec, as upstream")]
        let timeout = seconds + (tv_usec(usec) as f64) / 1000.0 / 1000.0;
        let kind = if read {
            TIMEOUT_READ
        } else {
            TIMEOUT_CONNECT | TIMEOUT_WRITE
        };
        socket.set_timeout(timeout, kind);
        return call.set_bool(true);
    } else if !plain && bind_to_device(name) {
        // SAFETY: a live argument.
        if unsafe { type_of(value) } == IS_STRING {
            // SAFETY: a string argument.
            let device = unsafe { value::str_of(value) };
            // SAFETY: live for the call.
            let device = unsafe { bytes(device) };
            (device.as_ptr().cast(), device.len())
        } else {
            (c"".as_ptr().cast(), 0)
        }
    } else {
        let Ok(number) = long(value) else { return };
        #[expect(
            clippy::cast_possible_truncation,
            reason = "an int option, as upstream's"
        )]
        {
            int = number as c_int;
        }
        ((&raw const int).cast(), size_of::<c_int>())
    };
    let fd = fd(socket);
    // SAFETY: `option` is readable for `length` bytes.
    let status = unsafe {
        libc::setsockopt(
            fd,
            level_int,
            name_int,
            option,
            socklen_t::try_from(length).unwrap_or(0),
        )
    };
    if status != 0 {
        warn_sys(&format!("setsockopt({fd}) failed"), errno());
        return call.set_bool(false);
    }
    call.set_bool(true);
}

/// `timeval.tv_usec = value`: `suseconds_t` is 64 bits on Linux.
#[cfg(target_os = "linux")]
fn tv_usec(value: i64) -> i64 {
    value
}

/// `timeval.tv_usec = value`: `suseconds_t` is 32 bits here.
#[cfg(not(target_os = "linux"))]
fn tv_usec(value: i64) -> i64 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the narrowing upstream's assignment makes"
    )]
    i64::from(value as i32)
}

#[cfg(target_os = "linux")]
fn bind_to_device(name: i64) -> bool {
    name == i64::from(libc::SO_BINDTODEVICE)
}

#[cfg(not(target_os = "linux"))]
fn bind_to_device(_name: i64) -> bool {
    false
}

/// The domain of an imported descriptor: `SO_DOMAIN` where it exists, else the
/// family of its name.
fn domain(fd: c_int) -> Option<c_int> {
    #[cfg(target_os = "linux")]
    {
        let mut domain: c_int = libc::AF_INET;
        if getsockopt(fd, libc::SOL_SOCKET, libc::SO_DOMAIN, &mut domain).is_some() {
            return Some(domain);
        }
    }
    // SAFETY: an all-zero sockaddr_storage is valid.
    let mut address: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let mut length = socklen_t::try_from(size_of::<libc::sockaddr_storage>()).unwrap_or(0);
    // SAFETY: `address` is writable for `length` bytes.
    let status = unsafe { libc::getsockname(fd, (&raw mut address).cast(), &raw mut length) };
    (status == 0).then_some(c_int::from(address.ss_family))
}

/// `php_swoole_create_socket_from_fd(fd, kind)` inside `zend_try`: null when
/// no socket could be made (after upstream's warning).
fn from_fd(fd: c_int, kind: c_int) -> Result<*mut zend_object, Bailout> {
    // SAFETY: transfer this descriptor through the real Rust object/custody constructor.
    unsafe { crate::socket::owner::from_fd(fd, kind) }
}

/// `swoole_coroutine_socketpair(int $domain, int $type, int $protocol): array|false`.
fn socketpair(call: &Call) {
    let parsed = call
        .arity(3, 3)
        .and_then(|()| Ok((call.long(1, 0)?, call.long(2, 0)?, call.long(3, 0)?)));
    let Ok((domain, kind, protocol)) = parsed else {
        return call.set_bool(false);
    };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "(int) casts, as upstream's"
    )]
    let (domain, kind, protocol) = (domain as c_int, kind as c_int, protocol as c_int);
    let mut pair: [c_int; 2] = [-1; 2];
    // SAFETY: two writable descriptors.
    if unsafe { libc::socketpair(domain, kind, protocol, pair.as_mut_ptr()) } != 0 {
        warn_sys("failed to create socket", errno());
        return call.set_bool(false);
    }
    // SAFETY: from a function handler.
    unsafe { native::swoole_rs_check_reactor() };
    let kind = swoole_rs_reactor::swr_v1_socket_type(domain, kind);
    let first = match from_fd(pair[0], kind) {
        Ok(object) if !object.is_null() => object,
        Ok(_) => return call.set_bool(false),
        Err(bailout) => return bailout.defer(),
    };
    let second = match from_fd(pair[1], kind) {
        Ok(object) if !object.is_null() => object,
        result => {
            // SAFETY: the first object, not yet shared.
            unsafe { swr_zend_object_release(first) };
            return match result {
                Err(bailout) => bailout.defer(),
                Ok(_) => call.set_bool(false),
            };
        }
    };
    // SAFETY: the return slot is writable and holds nothing yet.
    unsafe { value::array_init(call.return_value()) };
    // SAFETY: the array just made.
    let array = unsafe { table(call.return_value()) };
    for object in [first, second] {
        // SAFETY: an all-zero zval is undefined; it is given the object's only reference.
        let mut item: zval = unsafe { std::mem::zeroed() };
        // SAFETY: as above.
        unsafe { set_object(&raw mut item, object) };
        // SAFETY: the array takes the reference.
        unsafe { zend_hash_next_index_insert(array, &raw mut item) };
    }
}

/// `static import($stream): Socket|false`.
fn import(call: &Call) {
    if call.arity(1, 1).is_err() {
        return;
    }
    let stream_zval = call.zval(1);
    // SAFETY: a passed argument.
    if unsafe { type_of(stream_zval) } != IS_RESOURCE {
        #[expect(clippy::cast_possible_wrap, reason = "ZPP error codes are small")]
        // SAFETY: from a handler, as ZPP reports a wrong argument.
        unsafe {
            zend_wrong_parameter_error(
                ZPP_ERROR_WRONG_ARG as c_int,
                1,
                ptr::null_mut(),
                _zend_expected_type_Z_EXPECTED_RESOURCE,
                stream_zval,
            );
        }
        return;
    }
    let mut stream: *mut php_stream = ptr::null_mut();
    let mut fd: c_int = -1;
    // SAFETY: a resource argument and writable locals.
    match unsafe { swr_zend_stream_socketd(stream_zval, &raw mut stream, &raw mut fd) } {
        0 => {}
        2 => return bailout::defer(Err(bailout::Bailout)),
        // Not a stream: Zend has thrown, and nothing is returned.
        _ if stream.is_null() => return,
        _ => return call.set_bool(false),
    }
    let Some(domain) = domain(fd) else {
        warn_sys("getsockname() failed", errno());
        return call.set_bool(false);
    };
    let mut kind: c_int = libc::SOCK_STREAM;
    if getsockopt(fd, libc::SOL_SOCKET, libc::SO_TYPE, &mut kind).is_none() {
        warn_sys("getsockopt(SOL_SOCKET, SO_TYPE) failed", errno());
        return call.set_bool(false);
    }
    let kind = swoole_rs_reactor::swr_v1_socket_type(domain, kind);
    // SAFETY: a descriptor query.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        warn_sys("fcntl(F_GETFL) failed", errno());
        return call.set_bool(false);
    }
    // SAFETY: the stream argument and native descriptor classification stay live through import.
    let object = match unsafe {
        crate::socket::owner::import_stream(fd, kind, flags & libc::O_NONBLOCK, stream_zval)
    } {
        Ok(object) => object,
        Err(error) => return error.defer(),
    };
    if object.is_null() {
        return call.set_bool(false);
    }
    // SAFETY: the stream the object now holds.
    unsafe { swr_zend_stream_unbuffer(stream) };
    // SAFETY: the return slot takes the new object's only reference.
    unsafe { set_object(call.return_value(), object) };
}

swoole_rs_zend::handlers! {
    zif_swoole_coroutine_socketpair => socketpair;
    zim_swoole_socket_coro_getOption => get_option;
    zim_swoole_socket_coro_setOption => set_option;
    zim_swoole_socket_coro_import => import;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_type_sizes() {
        for (kinds, width) in [
            (&b"cC"[..], 1),
            (b"sSnv", 2),
            (b"lLNV", 4),
            (b"qQJP", 8),
            (b"x\0a", 0),
        ] {
            for kind in kinds {
                assert_eq!(type_size(*kind), width, "type {}", char::from(*kind));
            }
        }
    }

    #[test]
    fn fastcgi_lengths() {
        // content 0x0102, padding 3: 258 + 3 + header 8.
        assert_eq!(fastcgi_record_length([1, 6, 0, 1, 1, 2, 3, 0]), 269);
        assert_eq!(
            fastcgi_record_length([1, 6, 0, 1, 0xff, 0xff, 0, 0]),
            0xffff + 8
        );
        assert_eq!(fastcgi_record_length([1, 6, 0, 1, 0xff, 0xff, 1, 0]), -1);
    }

    #[test]
    fn protocol_matches_native_layout() {
        assert_eq!(size_of::<Protocol>(), 64, "C++ asserts sizeof(Protocol)");
        assert_eq!(std::mem::offset_of!(Protocol, get_package_length), 48);
        assert_eq!(std::mem::offset_of!(Protocol, package_max_length), 16);
    }

    #[test]
    fn text_stops_at_nul() {
        assert_eq!(text(b"ab\0cd").as_bytes(), b"ab");
        assert_eq!(text(b"abc").as_bytes(), b"abc");
    }
}
