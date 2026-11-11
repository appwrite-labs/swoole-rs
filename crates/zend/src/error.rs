//! Throwing from a handler; the handler then returns and Zend unwinds.
use crate::{
    Call, Class,
    bailout::{self, Bailout},
    sys::{zend_class_entry, zend_long, zend_throw_error, zend_throw_exception},
};
use std::{
    ffi::{CStr, CString},
    ptr,
};

/// `zend_throw_exception(ce, message, code)`.
/// # Safety
/// Called from a handler, with a registered exception class.
pub unsafe fn throw_exception(ce: *mut zend_class_entry, message: &CStr, code: zend_long) {
    // SAFETY: per the caller; Zend copies the message.
    unsafe { zend_throw_exception(ce, message.as_ptr(), code) };
}

impl Call {
    /// `zend_throw_error(NULL, "%s", message)`: an `Error`, from this handler.
    pub fn throw_error(&self, message: &CStr) {
        // SAFETY: a handler's frame is executing; "%s" takes the NUL-terminated message.
        unsafe { zend_throw_error(ptr::null_mut(), c"%s".as_ptr(), message.as_ptr()) };
    }

    /// `zend_throw_exception(class, message, code)`, from this handler.
    /// # Panics
    /// `class` is not registered.
    pub fn throw(&self, class: &Class, message: &CStr, code: zend_long) {
        let entry = class.entry();
        assert!(!entry.is_null(), "throw a registered class");
        // SAFETY: a handler's frame is executing, and the class is registered; Zend
        // copies the message.
        unsafe { zend_throw_exception(entry, message.as_ptr(), code) };
    }

    /// Swoole's "Constructor of %s can only be called once" `Error`, for `$this`.
    /// # Panics
    /// The handler has no `$this`.
    pub fn throw_constructed_twice(&self) {
        let name = self.class_name();
        assert!(name.is_some(), "a constructor's $this");
        let Some(name) = name else { return };
        let message = [
            b"Constructor of ",
            name.to_bytes(),
            b" can only be called once",
        ]
        .concat();
        self.throw_error(&CString::new(message).unwrap_or_default());
    }
}

/// `E_WARNING`.
#[expect(clippy::cast_possible_wrap, reason = "error levels are small")]
pub const E_WARNING: i32 = crate::sys::E_WARNING as i32;
/// `E_ERROR`: Zend bails out after reporting it.
#[expect(clippy::cast_possible_wrap, reason = "error levels are small")]
pub const E_ERROR: i32 = crate::sys::E_ERROR as i32;

/// A message formatted on the stack (truncated at 1023 bytes). It owns nothing,
/// so a Zend error that longjmps past it skips no drop.
#[derive(Clone, Copy)]
pub struct Message {
    buffer: [u8; 1024],
    length: usize,
}

impl Default for Message {
    fn default() -> Self {
        Self::new()
    }
}

impl Message {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            buffer: [0; 1024],
            length: 0,
        }
    }

    /// Append raw bytes, as `%s` of a C string would.
    pub fn push(&mut self, bytes: &[u8]) {
        let room = self.buffer.len() - 1 - self.length;
        let take = bytes.len().min(room);
        let free = self.buffer.get_mut(self.length..self.length + take);
        if let (Some(free), Some(bytes)) = (free, bytes.get(..take)) {
            free.copy_from_slice(bytes);
            self.length += take;
        }
    }

    /// The text so far, NUL-terminated.
    #[must_use]
    pub fn as_cstr(&self) -> &CStr {
        CStr::from_bytes_until_nul(&self.buffer).unwrap_or(c"")
    }
}

impl std::fmt::Write for Message {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        self.push(text.as_bytes());
        Ok(())
    }
}

/// `format!` into a [`Message`].
#[macro_export]
macro_rules! message {
    ($($arg:tt)*) => {{
        let mut message = $crate::Message::new();
        let _ = ::std::fmt::Write::write_fmt(&mut message, format_args!($($arg)*));
        message
    }};
}

/// `php_error_docref(NULL, level, "%s", message)` inside `zend_try`: a user
/// error handler runs PHP code, which can bail out (as `E_ERROR` always
/// does). Nothing is reported while a bailout is pending. Like the values this
/// crate makes, it runs on a PHP thread, from a handler or a callback Zend runs.
/// # Errors
/// It bailed out; defer it ([`crate::bailout::defer`]) or return it.
pub fn report(level: i32, message: &CStr) -> Result<(), Bailout> {
    if bailout::pending() {
        return Err(Bailout);
    }
    // SAFETY: on a PHP thread; the shim formats "%s" with the NUL-terminated message.
    bailout::trapped(unsafe { crate::sys::swr_zend_try_error(level, message.as_ptr()) })
}

/// `zend_error(level, "%s", message)` (no function prefix), as [`report`].
/// # Errors
/// It bailed out.
pub fn zend_error(level: i32, message: &CStr) -> Result<(), Bailout> {
    if bailout::pending() {
        return Err(Bailout);
    }
    // SAFETY: on a PHP thread, inside `zend_try`; "%s" takes the NUL-terminated message.
    bailout::trapped(unsafe { crate::sys::swr_zend_try_zend_error(level, message.as_ptr()) })
}
