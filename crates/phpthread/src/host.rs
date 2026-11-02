//! What C++ provides (`swr_v1_phpthread_host` in
//! `native/process/swoole_rs_phpthread.h`), and the deferred bailouts and
//! fatal errors the outermost Rust frame raises.
use std::{
    cell::Cell,
    ffi::{c_char, c_int, c_void},
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
};
use swoole_rs_zend::sys::{_zend_bailout, zend_class_entry, zend_string, zval};

/// Field for field the C struct.
#[repr(C)]
pub struct Host {
    pub exception: *mut zend_class_entry,
    pub serialize: unsafe extern "C" fn(value: *mut zval, bailed: *mut bool) -> *mut zend_string,
    pub unserialize:
        unsafe extern "C" fn(data: *const c_char, length: usize, out: *mut zval) -> bool,
    pub object_kind: unsafe extern "C" fn(object: *mut zval) -> u32,
    pub resource_cast: unsafe extern "C" fn(object: *mut zval, kind: u32) -> *mut c_void,
    pub resource_create:
        unsafe extern "C" fn(return_value: *mut zval, kind: u32, resource: *mut c_void),
    pub resource_add_ref: unsafe extern "C" fn(resource: *mut c_void),
    pub resource_del_ref: unsafe extern "C" fn(resource: *mut c_void),
    pub stream_cast: unsafe extern "C" fn(stream: *mut zval) -> c_int,
    pub co_socket_cast: unsafe extern "C" fn(object: *mut zval, kind: *mut c_int) -> c_int,
    pub php_socket_cast: Option<unsafe extern "C" fn(object: *mut zval) -> c_int>,
    pub stream_create: unsafe extern "C" fn(return_value: *mut zval, fd: i64),
    pub co_socket_create: unsafe extern "C" fn(return_value: *mut zval, fd: i64, kind: c_int),
    pub php_socket_create: Option<unsafe extern "C" fn(return_value: *mut zval, fd: i64)>,
    pub set_last_error: unsafe extern "C" fn(error: c_int),
    pub unconstructed: unsafe extern "C" fn(list: bool),
    /// Swoole\Thread's body on the new thread (`php_swoole_thread_start`):
    /// TSRM, request startup and the script; `enter` and `exit` report its life.
    pub thread_run: unsafe extern "C" fn(
        file: *const c_char,
        length: usize,
        argv: *mut c_void,
        context: *mut c_void,
        enter: extern "C" fn(*mut c_void),
        exit: extern "C" fn(*mut c_void, c_int),
    ),
    pub thread_arguments: unsafe extern "C" fn(return_value: *mut zval),
    pub thread_info: unsafe extern "C" fn(return_value: *mut zval),
    pub thread_count: unsafe extern "C" fn() -> usize,
    pub thread_set_name: unsafe extern "C" fn(name: *const c_char) -> bool,
    pub thread_native_id: unsafe extern "C" fn() -> i64,
    /// `php_swoole_error(level, "%s", message)`.
    pub error: unsafe extern "C" fn(level: c_int, message: *const c_char),
    /// `php_swoole_sys_error(level, "%s", message)`.
    pub sys_error: unsafe extern "C" fn(level: c_int, message: *const c_char),
}

static HOST: AtomicPtr<Host> = AtomicPtr::new(ptr::null_mut());

/// MINIT (`php_swoole_thread_minit`): C++'s static table, before any class.
/// # Safety
/// `host` is a complete table that lives for the process.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpthread_minit(host: *const Host) {
    HOST.store(host.cast_mut(), Ordering::Release);
}

/// The table MINIT installed.
pub(crate) fn host() -> &'static Host {
    // SAFETY: MINIT stores C++'s static table before any method runs, and it
    // lives for the process.
    unsafe { &*HOST.load(Ordering::Acquire) }
}

/// What the outermost Rust frame must raise once everything it holds is
/// released: a longjmp may never cross a Rust frame that owns anything.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Pending {
    /// PHP code (`__serialize`, `__unserialize`) bailed out inside C++'s trap.
    Bailout,
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

/// Raise what was deferred. Called last by every handler and export, whose
/// frame owns nothing by then.
pub(crate) fn finish() {
    match PENDING.with(Cell::take) {
        None => {}
        // SAFETY: Zend's own bailout from the outermost Rust frame, which owns
        // nothing that needs a drop.
        Some(Pending::Bailout) => unsafe {
            _zend_bailout(c"crates/phpthread".as_ptr().cast_mut(), line!());
        },
        // SAFETY: C++'s fatal error, as above.
        Some(Pending::Unconstructed { list }) => unsafe { (host().unconstructed)(list) },
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
        defer(Pending::Bailout);
        assert_eq!(
            PENDING.with(Cell::take),
            Some(Pending::Unconstructed { list: true }),
            "a later failure does not replace the first"
        );
        assert_eq!(PENDING.with(Cell::take), None, "taken once");
    }
}
