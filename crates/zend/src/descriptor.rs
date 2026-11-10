//! What a descriptor candidate (`php_swoole_convert_to_fd`) reads from a value,
//! and the executor state an event loop checks around its wait.
use crate::{
    Class, Value, ZStr,
    bailout::{self, Bailout},
    sys::{
        IS_RESOURCE, swr_zend_event_pause_shutdown, swr_zend_event_property,
        swr_zend_event_restore_shutdown, swr_zend_event_socket_fd, swr_zend_instanceof,
        swr_zend_last_fatal_error, swr_zend_stream_from_zval, swr_zend_stream_select_fd, zend_long,
    },
};
use std::ffi::{c_int, c_void};
use std::ptr;

impl Value {
    /// `instanceof_function(Z_OBJCE_P(value), class)`: false for a non-object or
    /// a class not registered.
    #[must_use]
    pub fn instance_of(&self, class: &Class) -> bool {
        let (Some(ce), parent) = (self.class(), class.entry()) else {
            return false;
        };
        // SAFETY: a live object's class and a registered one.
        !parent.is_null() && unsafe { swr_zend_instanceof(ce, parent) }
    }

    /// A stream resource's descriptor (`php_stream_cast` for select), if it
    /// is one and has one.
    /// # Errors
    /// The stream's wrapper bailed out.
    pub fn stream_fd(&self) -> Result<Option<c_int>, Bailout> {
        if self.kind() != IS_RESOURCE {
            return Ok(None);
        }
        // SAFETY: a resource zval; the no-verify lookup only reads its tag.
        let stream = unsafe { swr_zend_stream_from_zval(self.as_ptr()) };
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
        let mut found = ptr::null_mut();
        // SAFETY: a live object and name; the shim contains a magic-method bailout.
        if unsafe { swr_zend_event_property(self.as_ptr(), name.as_ptr(), &raw mut found) } != 0 {
            return Err(Bailout);
        }
        // SAFETY: null, or the initialized zval the lookup lends.
        let found = unsafe { Self::from_ptr(found) };
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

/// `php_swoole_is_fatal_error()`: the last error was fatal.
#[must_use]
pub fn last_fatal_error() -> bool {
    let (mut message, mut file, mut line) = (ptr::null(), ptr::null(), 0);
    // SAFETY: writable slots; the executor's last error is only read.
    unsafe { swr_zend_last_fatal_error(&raw mut message, &raw mut file, &raw mut line) }
}

/// Clear the executor's in-shutdown flag, as a loop waiting in a shutdown
/// function does: whether it was set, for [`restore_shutdown`].
#[must_use]
pub fn pause_shutdown() -> bool {
    // SAFETY: an SDK leaf that edits executor flags without running PHP.
    unsafe { swr_zend_event_pause_shutdown() != 0 }
}

/// Set the in-shutdown flag again when [`pause_shutdown`] found it set.
pub fn restore_shutdown(was: bool) {
    // SAFETY: as above.
    unsafe { swr_zend_event_restore_shutdown(u8::from(was)) };
}

struct Slot<T> {
    body: fn() -> T,
    out: Option<T>,
}

/// # Safety
/// `arg` is `contain`'s slot.
unsafe extern "C" fn run<T: Copy>(arg: *mut c_void) {
    // SAFETY: as above, borrowed for this call.
    let slot = unsafe { &mut *arg.cast::<Slot<T>>() };
    slot.out = Some((slot.body)());
}

/// `body()` inside `zend_try`. A bailout jumps over `body` and what it calls,
/// as over any frame below one that raises it ([`bailout::finish`]), so those
/// frames own nothing that needs a drop when PHP may bail out.
/// # Errors
/// `body` bailed out.
pub fn contain<T: Copy>(body: fn() -> T) -> Result<T, Bailout> {
    let mut slot = Slot { body, out: None };
    // SAFETY: `run` writes the slot, which outlives the trap and owns nothing to drop.
    unsafe { bailout::trap(run::<T>, (&raw mut slot).cast()) }?;
    slot.out.ok_or(Bailout)
}
