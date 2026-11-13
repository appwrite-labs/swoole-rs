//! What a descriptor candidate (`php_swoole_convert_to_fd`) reads from a value,
//! and the executor state an event loop checks around its wait.
use crate::{
    Class, Value, ZStr,
    bailout::{self, Bailout},
    sys::{IS_RESOURCE, swr_zend_event_socket_fd, swr_zend_stream_select_fd, zend_long},
};
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ptr;

unsafe extern "C" {
    /// `php_stream_from_zval_no_verify`: the stream a resource zval names, or
    /// null for any other value or resource. It reads the zval, the resource's
    /// type and the two stream type numbers PHP registers at startup.
    safe fn swr_zend_stream_from_zval(value: &Value) -> *mut crate::sys::php_stream;
    /// `zend_read_property_ex` of an object zval under its own class, inside
    /// `zend_try` (nonzero for a bailout): `found` is null (`None`) or the
    /// initialized zval the read lends, which lives until PHP code runs.
    fn swr_zend_event_property(
        object: &Value,
        name: &ZStr,
        found: &mut Option<&Value>,
    ) -> std::ffi::c_int;
    /// Whether the executor's last error was fatal; if so its message, its
    /// file and its line are written, the strings as PHP keeps them until the
    /// next error. It reads the executor's globals only.
    safe fn swr_zend_last_fatal_error(
        message: &mut *const c_char,
        file: &mut *const c_char,
        line: &mut c_int,
    ) -> bool;
    /// `zend_memory_usage`: it reads the allocator's statistics.
    safe fn zend_memory_usage(real: bool) -> usize;
    /// Clears the executor's in-shutdown flag and returns whether it was set;
    /// no PHP code runs.
    safe fn swr_zend_event_pause_shutdown() -> u8;
    /// Sets the in-shutdown flag when `previous` is nonzero; no PHP code runs.
    safe fn swr_zend_event_restore_shutdown(previous: u8);
    /// Saves the executor's stack limit and base in `saved`, then clears both,
    /// so Zend stops checking the stack's depth. The two are only ever compared
    /// with stack addresses.
    safe fn swr_zend_stack_unlimit(saved: &mut [*mut c_void; 2]);
    /// Sets the executor's stack limit and base to `saved`'s.
    safe fn swr_zend_stack_restore(saved: &[*mut c_void; 2]);
}

impl Value {
    /// `instanceof_function(Z_OBJCE_P(value), class)`: false for a non-object or
    /// a class not registered.
    #[must_use]
    pub fn instance_of(&self, class: &Class) -> bool {
        match (self.class_entry(), class.class_entry()) {
            (Some(own), Some(parent)) => own.instance_of(parent),
            _ => false,
        }
    }

    /// A stream resource's descriptor (`php_stream_cast` for select), if it
    /// is one and has one.
    /// # Errors
    /// The stream's wrapper bailed out.
    pub fn stream_fd(&self) -> Result<Option<c_int>, Bailout> {
        if self.kind() != IS_RESOURCE {
            return Ok(None);
        }
        let stream = swr_zend_stream_from_zval(self);
        if stream.is_null() {
            return Ok(None);
        }
        let mut fd = -1;
        // SAFETY: the live stream; the shim contains a wrapper's bailout.
        match unsafe { swr_zend_stream_select_fd(stream, &raw mut fd) } {
            2 => Err(Bailout),
            0 if fd >= 0 => Ok(Some(fd)),
            _ => Ok(None),
        }
    }

    /// `zend_read_property_ex(Z_OBJCE_P, Z_OBJ_P, name)` of an object: the
    /// property's int, if it is one.
    /// # Errors
    /// A magic `__get` bailed out.
    pub fn property_long(&self, name: &ZStr) -> Result<Option<zend_long>, Bailout> {
        if self.object().is_none() {
            return Ok(None);
        }
        let mut found = None;
        // SAFETY: a live object and name; the shim contains a magic-method
        // bailout, and what it lends is read before any PHP runs.
        if unsafe { swr_zend_event_property(self, name, &mut found) } != 0 {
            return Err(Bailout);
        }
        Ok(found.and_then(Self::as_long))
    }

    /// An ext/sockets `Socket`'s descriptor and whether it was closed, if this
    /// is one and PHP has ext/sockets.
    #[must_use]
    pub fn php_socket_fd(&self) -> Option<(c_int, bool)> {
        self.object()?;
        let mut closed = 0;
        // SAFETY: a live object zval.
        let fd = unsafe { swr_zend_event_socket_fd(self.as_ptr(), &raw mut closed) };
        (fd != -2).then_some((fd, closed != 0))
    }
}

/// The executor's last error, when it was fatal: where PHP keeps its message
/// and file until the next error, and its line.
fn fatal_parts() -> Option<([*const c_char; 2], c_int)> {
    let (mut message, mut file, mut line) = (ptr::null(), ptr::null(), 0);
    swr_zend_last_fatal_error(&mut message, &mut file, &mut line).then_some(([message, file], line))
}

/// `php_swoole_is_fatal_error()`: the last error was fatal.
#[must_use]
pub fn last_fatal_error() -> bool {
    fatal_parts().is_some()
}

/// A fatal error's message, file and line.
pub struct FatalError {
    pub message: CString,
    pub file: CString,
    pub line: c_int,
}

/// The last error, when it was fatal: copies, since PHP replaces its own at
/// the next error.
#[must_use]
pub fn fatal_error() -> Option<FatalError> {
    let (texts, line) = fatal_parts()?;
    // SAFETY: a fatal error's NUL-terminated message, then its file, each live
    // until the next error.
    let [message, file] = texts.map(|text| unsafe { CStr::from_ptr(text) }.to_owned());
    Some(FatalError {
        message,
        file,
        line,
    })
}

/// `zend_memory_usage(real)`: the bytes this thread's PHP heap holds.
#[must_use]
pub fn memory_usage(real: bool) -> usize {
    zend_memory_usage(real)
}

/// Clear the executor's in-shutdown flag, as a loop waiting in a shutdown
/// function does: whether it was set, for [`restore_shutdown`].
#[must_use]
pub fn pause_shutdown() -> bool {
    swr_zend_event_pause_shutdown() != 0
}

/// Set the in-shutdown flag again when [`pause_shutdown`] found it set.
pub fn restore_shutdown(was: bool) {
    swr_zend_event_restore_shutdown(u8::from(was));
}

/// `f()` with the executor's stack limit lifted, as `HOOK_PHP_CALL_STACK` does
/// for a call from a stack that isn't the VM's, such as a reactor thread's.
pub fn with_unlimited_stack<R>(f: impl FnOnce() -> R) -> R {
    let mut saved = [ptr::null_mut(); 2];
    swr_zend_stack_unlimit(&mut saved);
    let result = f();
    swr_zend_stack_restore(&saved);
    result
}

/// `body()` inside `zend_try`. A bailout jumps over `body` and what it calls,
/// as over any frame below one that raises it ([`bailout::finish`]), so those
/// frames own nothing that needs a drop when PHP may bail out.
/// # Errors
/// `body` bailed out.
pub fn contain<T: Copy>(body: fn() -> T) -> Result<T, Bailout> {
    let mut out = None;
    // SAFETY: the closure holds a function pointer and a borrow of `out`, a
    // `Copy` value: nothing to drop. `body`'s frames are its caller's (above).
    unsafe { bailout::trap_with(|| out = Some(body())) }?;
    out.ok_or(Bailout)
}
