//! Throwing from a handler; the handler then returns and Zend unwinds.
use crate::{
    Call, Class,
    bailout::{self, Bailout},
    helpers::CText,
    sys::{zend_long, zend_throw_error, zend_type_error},
};
use std::{
    ffi::{CStr, CString},
    ptr,
};

unsafe extern "C" {
    /// `zend_get_type_by_const(kind)` for a value's type (`IS_NULL` to
    /// `IS_RESOURCE`): one of Zend's static names, or null (`None`) for any
    /// other code. It reads nothing else.
    safe fn swr_zend_type_name(kind: u32) -> Option<CText<'static>>;
}

/// `zend_throw_error(NULL, "%s", message)`: an `Error`. Like [`Class::throw`], from a
/// handler or a callback Zend runs under one: with a frame executing, Zend only sets
/// the exception.
pub fn throw_error(message: &CStr) {
    // SAFETY: "%s" takes the NUL-terminated message, which Zend copies.
    unsafe { zend_throw_error(ptr::null_mut(), c"%s".as_ptr(), message.as_ptr()) };
}

/// `zend_type_error("%s", message)`: a `TypeError`, as [`throw_error`].
pub fn type_error(message: &CStr) {
    // SAFETY: "%s" takes the NUL-terminated message, which Zend copies.
    unsafe { zend_type_error(c"%s".as_ptr(), message.as_ptr()) };
}

/// `zend_throw_exception(class, message, code)` inside `zend_try`: an exception
/// of `class` (Zend's `Exception` for None, or a class not registered).
/// # Errors
/// Zend bailed out: with no frame executing it reports the exception at once.
pub fn throw_trapped(
    class: Option<&Class>,
    message: &CStr,
    code: zend_long,
) -> Result<(), Bailout> {
    let entry = class.map_or(ptr::null_mut(), Class::entry);
    // SAFETY: a registered exception class or null; Zend copies the message,
    // and the shim traps a bailout.
    bailout::trapped(unsafe {
        crate::sys::swr_zend_try_facade_throw(entry, message.as_ptr(), code)
    })
}

/// `zend_get_type_by_const(kind)`: Zend's name of a value's type (`IS_NULL` to
/// `IS_RESOURCE`), or None for any other code.
#[must_use]
pub fn type_name(kind: u32) -> Option<&'static CStr> {
    swr_zend_type_name(kind).map(CText::get)
}

impl Call {
    /// `zend_throw_error(NULL, "%s", message)`: an `Error`, from this handler.
    pub fn throw_error(&self, message: &CStr) {
        throw_error(message);
    }

    /// `zend_throw_exception(class, message, code)`, from this handler.
    /// # Panics
    /// `class` is not registered.
    pub fn throw(&self, class: &Class, message: &CStr, code: zend_long) {
        class.throw(message, code);
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
