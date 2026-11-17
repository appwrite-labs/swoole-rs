//! Listening socket import and close.
use core::ffi::{CStr, c_int};
use swoole_rs_clients::address::{address_port, with_host};
use swoole_rs_corebase::log::LogLevel::SW_LOG_WARNING as LOG_WARNING;
use swoole_rs_corebase::reactor::{OwnedSocket, SocketRecord, socket_type};
use swoole_rs_corebase::{log, sys};
use swoole_rs_server::port_config::PortConfig;
pub use swoole_rs_server::port_tls::{context_dup, sni_add, ssl_init};

use crate::methods::{FD_DGRAM_SERVER, FD_STREAM_SERVER, socket_free};

/// `getsockopt(fd, SOL_SOCKET, option)`: the value, or `None` (errno set).
fn get_option(fd: c_int, option: c_int) -> Option<c_int> {
    let mut value: c_int = 0;
    swoole_rs_corebase::sys::getsockopt(fd, libc::SOL_SOCKET, option, &mut value).ok()?;
    Some(value)
}

/// `ListenPort::import(fd)`: adopts a listening socket; the fd stays the
/// caller's when it isn't one. On failure the port takes no record.
pub(crate) fn import(config: &mut PortConfig, fd: c_int) -> bool {
    let origin = c"ListenPort::import";
    let mut socket = SocketRecord::with_fd(fd, 0);
    let Some(so_type) = get_option(fd, libc::SO_TYPE) else {
        let message = format_args!("getsockopt({fd}, SOL_SOCKET, SO_TYPE) failed");
        log::sys_warning(origin, message);
        return false;
    };
    if !socket.get_name() {
        log::sys_warning(origin, format_args!("getsockname({fd}) failed"));
        return false;
    }
    let Some(acceptconn) = get_option(fd, libc::SO_ACCEPTCONN) else {
        let message = format_args!("getsockopt({fd}, SOL_SOCKET, SO_ACCEPTCONN) failed");
        log::sys_warning(origin, message);
        return false;
    };
    if acceptconn == 0 {
        let message = format_args!("the socket[{fd}] is not a listening socket");
        log::error_log(LOG_WARNING, libc::EINVAL, origin, message);
        return false;
    }
    let family = c_int::from(socket.info.addr.family());
    let kind = socket_type(family, so_type);
    socket.socket_type = kind.raw();
    socket.info.kind = kind.raw();
    socket.fd_type = if kind.is_dgram() {
        FD_DGRAM_SERVER
    } else {
        FD_STREAM_SERVER
    };
    socket.removed = true;
    let info = socket.info;
    config.set_listening_socket(OwnedSocket::new(Box::new(socket)));
    config.r#type = kind.raw();
    if !with_host(&info, |host| config.host.set(host.to_bytes())) {
        swoole_rs_corebase::fatal("out of memory storing a port's host");
    }
    config.port.set(address_port(&info).into());
    config.listening.set(true);
    true
}

/// `ListenPort::close()`: drops the TLS context, frees the socket
/// and unlinks a Unix socket's path.
pub(crate) fn close(config: &PortConfig) {
    if config.ssl {
        drop(config.ssl_context.replace(None));
        drop(config.tls.settings.replace(None));
    }
    // Taken first, so nothing finds the record once it is freed.
    socket_free(config.take_socket());
    if matches!(config.r#type, 5 | 6) {
        let mut path = config.host_bytes().to_vec();
        path.push(0);
        if let Ok(path) = CStr::from_bytes_until_nul(&path) {
            _ = sys::unlink(path);
        }
    }
}
