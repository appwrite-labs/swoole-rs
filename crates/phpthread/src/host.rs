//! What the thread classes reach outside this crate (Swoole's exception class,
//! serialization, the coroutine and ext/sockets sockets a container may hold,
//! Swoole's errors), and the deferred bailouts and fatal errors the outermost
//! Rust frame raises.
use std::{
    cell::Cell,
    ffi::{CStr, c_int},
};
use swoole_rs_error::ErrorCode::{
    SW_ERROR_PHP_FATAL_ERROR, SW_ERROR_PHP_RUNTIME_NOTICE, SW_ERROR_WRONG_OPERATION,
};
use swoole_rs_zend::{
    E_ERROR, Owned, Value,
    bailout::{self, Bailout},
};

/// `php_swoole_serialize`: the serialized bytes, or `None`; a bailout is
/// deferred.
pub(crate) fn serialize(value: &Value) -> Option<Box<[u8]>> {
    let (serialized, bailed) = swoole_rs_phpswoole::serialize(value);
    bailout::defer(bailed);
    serialized
}

/// `php_swoole_unserialize`; a bailout is deferred.
pub(crate) fn unserialize(data: &[u8]) -> Owned {
    let (out, bailed) = swoole_rs_phpswoole::unserialize(data);
    bailout::defer(bailed);
    out
}

/// `php_swoole_thread_co_socket_cast`: a duplicate of a coroutine socket's
/// descriptor and its type, or -1; `None` when `object` is no coroutine socket
/// (`sw_zval_is_co_socket`).
pub(crate) fn co_socket_cast(object: &Value) -> Option<(c_int, c_int)> {
    if !object.instance_of(swoole_rs_phpclients::socket()) {
        return None;
    }
    let Some((descriptor, kind)) = swoole_rs_phpclients::socket_descriptor(object) else {
        return Some((-1, 0));
    };
    if descriptor < 0 {
        return Some((-1, 0));
    }
    let fd = swoole_rs_corebase::sys::dup(descriptor);
    if fd < 0 {
        return Some((-1, 0));
    }
    Some((fd, kind))
}

/// A coroutine socket of type `kind` over `fd`, which it takes; `None` on
/// failure.
pub(crate) fn co_socket_create(fd: c_int, kind: c_int) -> Option<Owned> {
    // SAFETY: a descriptor the new socket takes, in a request.
    unsafe { swoole_rs_phpclients::create_from_fd(fd, kind) }
}

/// A duplicate of an ext/sockets Socket's descriptor: -1 after throwing
/// "failed to dup socket fd", -2 after "invalid socket fd", -3 when `object`
/// is no Socket (or Swoole was built without ext/sockets).
pub(crate) fn php_socket_cast(object: &Value) -> c_int {
    swoole_rs_phpswoole::socket_cast(object)
}

/// An ext/sockets Socket over `fd`, which it takes; `None` on failure.
pub(crate) fn php_socket_create(fd: c_int) -> Option<Owned> {
    let (socket, imported) = swoole_rs_phpswoole::socket_create(fd);
    imported.then_some(socket)
}

/// `php_swoole_error(level, "%s", message)`: reported when
/// `swoole.display_errors` is on, or for `E_ERROR`.
pub(crate) fn error(level: c_int, message: &CStr) -> Result<(), Bailout> {
    swoole_rs_corebase::log::set_last_error(SW_ERROR_PHP_RUNTIME_NOTICE as c_int);
    if swoole_rs_phpswoole::display_errors() || level == E_ERROR {
        return swoole_rs_zend::report(level, message);
    }
    Ok(())
}

/// `php_swoole_sys_error(level, "%s", message)`: [`error`] with errno's
/// message and number appended, and errno as Swoole's last error.
pub(crate) fn sys_error(level: c_int, message: &CStr) -> Result<(), Bailout> {
    let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    swoole_rs_corebase::log::set_last_error(errno);
    if !swoole_rs_phpswoole::display_errors() && level != E_ERROR {
        return Ok(());
    }
    let full = std::ffi::CString::new(format!(
        "{}, Error: {}[{errno}]",
        message.to_string_lossy(),
        strerror(errno)
    ))
    .unwrap_or_default();
    swoole_rs_zend::report(level, &full)
}

pub(crate) use swoole_rs_corebase::sys::strerror;

/// What the outermost Rust frame must raise once everything it holds is
/// released: a longjmp may never cross a Rust frame that owns anything.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Pending {
    /// An unconstructed container was stored: upstream's fatal error.
    Unconstructed { list: bool },
}

thread_local! {
    static PENDING: Cell<Option<Pending>> = const { Cell::new(None) };
}

/// Remember the first thing to raise.
pub(crate) fn defer(pending: Pending) {
    PENDING.with(|cell| {
        if cell.get().is_none() {
            cell.set(Some(pending));
        }
    });
}

/// The fatal "must call constructor first": Swoole's for an `ArrayList` (or a
/// Barrier), PHP's for a Map or Queue. Neither returns.
pub(crate) fn unconstructed(list: bool) {
    let message = c"must call constructor first";
    if list {
        // SAFETY: from the outermost Rust frame, which owns nothing.
        unsafe { swoole_rs_corebase::fatal_error(SW_ERROR_WRONG_OPERATION as c_int, message) };
    } else {
        swoole_rs_corebase::log::set_last_error(SW_ERROR_PHP_FATAL_ERROR as c_int);
        bailout::raise(swoole_rs_zend::report(E_ERROR, message));
    }
}

/// Raise what was deferred: a bailout PHP code (`__serialize`,
/// `__unserialize`, a user error handler) left, then the fatal error. Called
/// last by every handler and export, whose frame owns nothing by then.
pub(crate) fn finish() {
    swoole_rs_zend::bailout::finish();
    if let Some(Pending::Unconstructed { list }) = PENDING.with(Cell::take) {
        unconstructed(list);
    }
}

/// Zend handlers under Zend's own names; each runs `body(&Call)`, then raises
/// anything deferred ([`finish`]).
macro_rules! handlers {
    ($($name:ident => $body:path;)*) => {
        swoole_rs_zend::handlers!(@then { $crate::host::finish(); } $($name => $body;)*);
    };
}
pub(crate) use handlers;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_deferred_raise_wins() {
        defer(Pending::Unconstructed { list: true });
        defer(Pending::Unconstructed { list: false });
        assert_eq!(
            PENDING.with(Cell::take),
            Some(Pending::Unconstructed { list: true }),
            "a later failure does not replace the first"
        );
        assert_eq!(PENDING.with(Cell::take), None, "taken once");
    }
}
