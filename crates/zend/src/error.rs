//! Throwing from a handler; the handler then returns and Zend unwinds.
use crate::{
    Call, Class, ClassEntry,
    bailout::{self, Bailout},
    helpers::CText,
    sys::{zend_class_entry, zend_long},
};
use std::ffi::{CStr, CString, c_int};

unsafe extern "C" {
    /// Zend's `TypeError` and `ValueError`: registered at startup, before any
    /// extension code runs, and never written again.
    safe static zend_ce_type_error: *mut zend_class_entry;
    safe static zend_ce_value_error: *mut zend_class_entry;
    /// `zend_get_type_by_const(kind)` for a value's type (`IS_NULL` to
    /// `IS_RESOURCE`): one of Zend's static names, or null (`None`) for any
    /// other code. It reads nothing else.
    safe fn swr_zend_type_name(kind: u32) -> Option<CText<'static>>;
    /// `zend_throw_exception(class, message, code)` inside `zend_try`
    /// (nonzero for a bailout): an exception of a live class (Zend's own
    /// `Exception` for none, or for a class that is no `Throwable`). Zend copies
    /// the message.
    safe fn swr_zend_try_facade_throw(
        class: Option<ClassEntry<'_>>,
        message: CText<'_>,
        code: zend_long,
    ) -> c_int;
    /// `php_error_docref(NULL, level, "%s", message)` inside `zend_try`
    /// (nonzero for a bailout): the error handler it runs is PHP code.
    safe fn swr_zend_try_error(level: c_int, message: CText<'_>) -> c_int;
    /// `zend_error(level, "%s", message)`, as [`swr_zend_try_error`].
    safe fn swr_zend_try_zend_error(level: c_int, message: CText<'_>) -> c_int;
    /// `zend_throw_error(NULL, "%s", message)` inside `zend_try` (nonzero for a
    /// bailout): an `Error`; Zend copies the message.
    safe fn swr_zend_try_facade_throw_error(message: CText<'_>) -> c_int;
}

/// `zend_throw_error(NULL, "%s", message)`: an `Error`. With a frame executing, Zend
/// only sets the exception; with none it reports a fatal error and bails out, which
/// is deferred.
pub fn throw_error(message: &CStr) {
    bailout::defer(bailout::trapped(swr_zend_try_facade_throw_error(
        message.into(),
    )));
}

/// `zend_type_error("%s", message)`: a `TypeError`, as [`throw_error`].
pub fn type_error(message: &CStr) {
    throw_core(zend_ce_type_error, message);
}

/// `zend_value_error("%s", message)`: a `ValueError`, as [`throw_error`].
pub fn value_error(message: &CStr) {
    throw_core(zend_ce_value_error, message);
}

/// `zend_throw_exception(class, message, 0)` of one of Zend's own classes, as
/// `zend_type_error` and `zend_value_error` throw theirs, inside `zend_try`; a
/// bailout (no frame executing) is deferred.
fn throw_core(class: *mut zend_class_entry, message: &CStr) {
    // A class Zend registered at startup, which lives for the process.
    let class = ClassEntry::live(class);
    bailout::defer(bailout::trapped(swr_zend_try_facade_throw(
        class,
        message.into(),
        0,
    )));
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
    let class = class.and_then(Class::class_entry);
    bailout::trapped(swr_zend_try_facade_throw(class, message.into(), code))
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
    bailout::trapped(swr_zend_try_error(level, message.into()))
}

/// `zend_error(level, "%s", message)` (no function prefix), as [`report`].
/// # Errors
/// It bailed out.
pub fn zend_error(level: i32, message: &CStr) -> Result<(), Bailout> {
    if bailout::pending() {
        return Err(Bailout);
    }
    bailout::trapped(swr_zend_try_zend_error(level, message.into()))
}
