//! `php_async_socket_connect_to_host` and `php_async_socket_poll`: the
//! bundled ftp extension's network calls over a hooked coroutine socket.
use crate::{
    in_coroutine,
    socket::{Report, Socket, report, strerror, timeout},
    stream::{raise, seconds},
};
use libc::{c_char, c_int, c_long};
use std::ffi::CStr;
use swoole_rs_zend::{
    E_WARNING,
    bailout::Bailout,
    message, string,
    sys::{timeval, zend_string},
};

use swoole_rs_hooks::hooked::{swoole_coroutine_close, swoole_coroutine_socket};

unsafe extern "C" {
    fn inet_pton(family: c_int, source: *const c_char, destination: *mut libc::c_void) -> c_int;
}

/// `STREAM_SOCKOP_SO_BROADCAST`, `STREAM_SOCKOP_TCP_NODELAY`.
const SO_BROADCAST: c_long = 1 << 2;
const TCP_NODELAY: c_long = 1 << 5;
/// `SW_ERROR_DNSLOOKUP_RESOLVE_FAILED`.
const DNS_RESOLVE_FAILED: i32 = 711;

/// `php_async_socket_connect_to_host`: a connected coroutine socket's fd, or
/// -1 with the error. A trapped bailout is raised last.
/// # Safety
/// Called with the ftp extension's arguments: `host` and a non-null `bindto`
/// are NUL-terminated; the other pointers are null or writable.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "php_async_socket_connect_to_host")
)]
pub unsafe extern "C" fn connect_to_host(
    host: *const c_char,
    port: u16,
    _socktype: c_int,
    _asynchronous: c_int,
    timeout: *mut timeval,
    error_string: *mut *mut zend_string,
    error_code: *mut c_int,
    bindto: *const c_char,
    bindport: u16,
    sockopts: c_long,
) -> c_int {
    // SAFETY: per the caller.
    let host = unsafe { CStr::from_ptr(host) };
    // SAFETY: as above.
    let bindto = (!bindto.is_null()).then(|| unsafe { CStr::from_ptr(bindto) });
    let options = Options {
        timeout,
        error_string,
        error_code,
        bind: bindto.map(|host| (host, bindport)),
        sockopts,
    };
    raise(connect(host, port, &options), -1)
}

struct Options<'a> {
    timeout: *mut timeval,
    error_string: *mut *mut zend_string,
    error_code: *mut c_int,
    bind: Option<(&'a CStr, u16)>,
    sockopts: c_long,
}

fn connect(host: &CStr, port: u16, options: &Options) -> Result<c_int, Bailout> {
    if !in_coroutine() {
        report(
            Report::Fatal,
            E_WARNING,
            c"This API must be called in coroutine",
        )?;
        return Ok(-1);
    }
    let mut address = [0u8; 16];
    // SAFETY: a NUL-terminated host and room for an IPv6 address.
    let ipv6 = unsafe { inet_pton(libc::AF_INET6, host.as_ptr(), address.as_mut_ptr().cast()) };
    let domain = if ipv6 == 1 {
        libc::AF_INET6
    } else {
        libc::AF_INET
    };
    let fd = swoole_coroutine_socket(domain, libc::SOCK_STREAM, 0);
    if fd < 0 {
        return Ok(-1);
    }
    let Some(socket) = Socket::of_fd(fd) else {
        return Ok(-1);
    };
    if let Some((local, local_port)) = options.bind
        && !socket.bind(local, local_port.into())
    {
        let message = message!(
            "Failed to bind to '{}:{local_port}', system said: {}",
            local.to_string_lossy(),
            socket.strerror().to_string_lossy()
        );
        report(Report::Fatal, E_WARNING, message.as_cstr())?;
    }
    if options.sockopts & SO_BROADCAST != 0 {
        socket.set_option(libc::SOL_SOCKET, libc::SO_BROADCAST, 1, true);
    }
    if options.sockopts & TCP_NODELAY != 0 {
        socket.set_option(libc::IPPROTO_TCP, libc::TCP_NODELAY, 1, true);
    }
    if !options.timeout.is_null() {
        socket.set_timeout(seconds(options.timeout), timeout::ALL);
    }
    if socket.connect(host, port.into()) {
        return Ok(fd);
    }
    let code = socket.error();
    if !options.error_code.is_null() {
        // SAFETY: the caller's output slot.
        unsafe { *options.error_code = code };
    }
    if !options.error_string.is_null() {
        // SAFETY: the caller's output slot.
        unsafe { *options.error_string = string::copy(strerror(code).to_bytes()) };
    }
    if code == DNS_RESOLVE_FAILED {
        let message = message!(
            "getaddrinfo for '{}' failed, error: {}",
            host.to_string_lossy(),
            socket.strerror().to_string_lossy()
        );
        report(Report::Fatal, E_WARNING, message.as_cstr())?;
    }
    drop(socket);
    // The fd this call opened.
    swoole_coroutine_close(fd);
    Ok(-1)
}

/// `php_async_socket_poll`: wait `timeout` ms for `fd` to be writable
/// (`POLLOUT` in `events`) or readable; -1 if it is not a coroutine socket.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "php_async_socket_poll")
)]
#[must_use]
pub extern "C" fn socket_poll(fd: c_int, events: c_int, timeout: c_int) -> c_int {
    let Some(socket) = Socket::of_fd(fd) else {
        return -1;
    };
    let write = events & c_int::from(libc::POLLOUT) != 0;
    socket.poll(write, f64::from(timeout) / 1000.0).into()
}
