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
    E_ERROR, Value,
    bailout::{self, Bailout},
    sys::{swr_zend_instanceof, zend_class_entry, zend_string, zval},
};

/// `swoole_exception_ce`.
pub(crate) fn exception() -> *mut zend_class_entry {
    swoole_rs_phpswoole::exception_class()
}

/// `php_swoole_serialize`: a persistent string or null; a bailout is deferred.
/// # Safety
/// In a request, with a live zval.
pub(crate) unsafe fn serialize(value: *mut zval) -> *mut zend_string {
    // SAFETY: per the caller.
    let Some(value) = (unsafe { swoole_rs_zend::Value::from_ptr(value) }) else {
        return std::ptr::null_mut();
    };
    let (serialized, bailed) = swoole_rs_phpswoole::serialize(value);
    bailout::defer(bailed);
    serialized
}

/// `php_swoole_unserialize` into `out`; a bailout is deferred.
/// # Safety
/// In a request; `out` is writable.
pub(crate) unsafe fn unserialize(data: &[u8], out: *mut zval) {
    // SAFETY: per the caller.
    let (_, bailed) = unsafe { swoole_rs_phpswoole::unserialize(data, out) };
    bailout::defer(bailed);
}

/// `sw_zval_is_co_socket`.
pub(crate) fn is_co_socket(object: &Value) -> bool {
    let socket = swoole_rs_phpclients::socket_class();
    let Some(ce) = object.class() else {
        return false;
    };
    // SAFETY: a live object's class and a registered one.
    !socket.is_null() && unsafe { swr_zend_instanceof(ce, socket) }
}

/// `php_swoole_thread_co_socket_cast`: a duplicate of the coroutine socket's
/// descriptor and its type, or -1.
/// # Safety
/// `object` is a live Coroutine\Socket zval.
pub(crate) unsafe fn co_socket_cast(object: *mut zval) -> (c_int, c_int) {
    // SAFETY: per the caller.
    let socket = unsafe { swoole_rs_phpclients::get_socket(object) };
    if socket.is_null() {
        return (-1, 0);
    }
    // SAFETY: a live socket.
    let record = unsafe { swoole_rs_phpruntime::co::record(socket) };
    if record.sock_fd < 0 {
        return (-1, 0);
    }
    // SAFETY: the socket's own descriptor.
    let fd = unsafe { libc::dup(record.sock_fd) };
    if fd < 0 {
        return (-1, 0);
    }
    (fd, record.kind)
}

/// A coroutine socket of type `kind` over `fd`, which it takes; false on failure.
/// # Safety
/// In a request; `out` is writable.
pub(crate) unsafe fn co_socket_create(out: *mut zval, fd: c_int, kind: c_int) -> bool {
    // SAFETY: per the caller.
    let object = unsafe { swoole_rs_phpclients::create_from_fd(fd, kind) };
    if object.is_null() {
        return false;
    }
    crate::item::set(
        out,
        swoole_rs_zend::sys::IS_OBJECT_EX,
        swoole_rs_zend::sys::zend_value { obj: object },
    );
    true
}

/// A duplicate of an ext/sockets Socket's descriptor: -1 after throwing
/// "failed to dup socket fd", -2 after "invalid socket fd", -3 when `object`
/// is no Socket (or Swoole was built without ext/sockets).
pub(crate) fn php_socket_cast(object: &Value) -> c_int {
    swoole_rs_phpswoole::socket_cast(object)
}

/// An ext/sockets Socket over `fd`, which it takes; false on failure.
/// # Safety
/// In a request; `out` is writable.
pub(crate) unsafe fn php_socket_create(out: *mut zval, fd: c_int) -> bool {
    // SAFETY: per the caller.
    unsafe { swoole_rs_phpswoole::socket_create(out, fd) }
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
    // SAFETY: no preconditions.
    let text = unsafe { libc::strerror(errno) };
    // SAFETY: libc's NUL-terminated message.
    let text = unsafe { CStr::from_ptr(text) };
    let full = std::ffi::CString::new(format!(
        "{}, Error: {}[{errno}]",
        message.to_string_lossy(),
        text.to_string_lossy()
    ))
    .unwrap_or_default();
    swoole_rs_zend::report(level, &full)
}

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
/// anything deferred.
macro_rules! handlers {
    ($($name:ident => $body:path;)*) => {$(
        /// A Zend method handler.
        /// # Safety
        /// Called by Zend with a live frame and return slot.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(
            frame: *mut swoole_rs_zend::sys::zend_execute_data,
            value: *mut swoole_rs_zend::sys::zval,
        ) {
            {
                // SAFETY: per Zend's handler contract.
                let call = unsafe { swoole_rs_zend::Call::new(frame, value) };
                $body(&call);
            }
            $crate::host::finish();
        }
    )*};
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
