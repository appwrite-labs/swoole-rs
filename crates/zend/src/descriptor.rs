//! What a descriptor candidate (`php_swoole_convert_to_fd`) reads from a value,
//! and the executor state an event loop checks around its wait.
use crate::helpers::CText;
use crate::{
    Array, Class, Value, ZStr,
    bailout::{self, Bailout},
    sys::{IS_RESOURCE, php_stream_context, zend_long},
};
use std::ffi::{CStr, CString, c_int, c_void};
use std::{marker::PhantomData, ptr, ptr::NonNull};

unsafe extern "C" {
    /// The trapped cast writes the stream and descriptor to local output slots.
    safe fn swr_zend_stream_socketd(
        value: &Value,
        stream: &mut Option<NonNull<crate::sys::php_stream>>,
        fd: &mut c_int,
    ) -> c_int;

    /// `php_stream_from_zval_no_verify`: the stream a resource zval names, or
    /// null for any other value or resource. It reads the zval, the resource's
    /// type and the two stream type numbers PHP registers at startup.
    safe fn swr_zend_stream_from_zval(value: &Value) -> *mut crate::sys::php_stream;
    /// `php_stream_read` into `page` from the stream a resource zval names, inside
    /// `zend_try`: the bytes read, 0 at the end, -1 for no stream, -2 for a bailout.
    /// It writes at most `size` bytes, which the caller passes as the page's length.
    safe fn swr_zend_try_stream_zval_read(
        value: &Value,
        page: &mut [u8; STREAM_PAGE],
        size: usize,
    ) -> zend_long;
    /// `zend_read_property_ex` of an object zval under its own class, inside
    /// `zend_try` (nonzero for a bailout): `found` set and `out` written when
    /// the property holds an int; nothing for any other value.
    safe fn swr_zend_event_property_long(
        object: &Value,
        name: &ZStr,
        out: &mut zend_long,
        found: &mut bool,
    ) -> c_int;
    /// `php_stream_cast(.., PHP_STREAM_AS_FD_FOR_SELECT ..)` of the stream a
    /// resource zval names, inside `zend_try`: 0 with `fd` written, 1 for no
    /// stream or descriptor, 2 when a wrapper bailed out.
    safe fn swr_zend_stream_select_fd(value: &Value, fd: &mut c_int) -> c_int;
    /// Whether the executor's last error was fatal; if so its message, its
    /// file and its line are written, the strings as PHP keeps them until the
    /// next error. It reads the executor's globals only. Private: only
    /// [`fatal_parts`] calls it, and lends the strings no longer than its call.
    safe fn swr_zend_last_fatal_error<'a>(
        message: &mut Option<CText<'a>>,
        file: &mut Option<CText<'a>>,
        line: &mut c_int,
    ) -> bool;
    /// An ext/sockets `Socket`'s descriptor, with `closed` set when it was
    /// closed; -2 for any other value, or without ext/sockets. It reads the
    /// class table and the value only.
    safe fn swr_zend_event_socket_fd(value: &Value, closed: &mut u8) -> c_int;
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
    /// `Z_ARRVAL(context->options)`, null (`None`) when they are no array.
    safe fn swr_zend_stream_context_options<'a>(context: StreamContext<'a>) -> Option<&'a Array>;
}

impl crate::Owned {
    /// Close a socket's imported stream, leaving the resource in its slot on bailout.
    /// An empty slot means the caller still owns the descriptor.
    /// # Errors
    /// The stream wrapper bailed out during close.
    pub fn close_imported_stream(slot: &std::cell::Cell<Self>) -> Result<bool, Bailout> {
        use swoole_rs_corebase::CellExt;
        let Some(stream) =
            slot.with_taken(|held| (held.kind() == IS_RESOURCE).then(|| held.stream()))
        else {
            return Ok(false);
        };
        // SAFETY: the restored slot owns the resource. No PHP runs between lookup
        // and close, and the shim traps any bailout from closing the stream.
        if !stream.is_null() && unsafe { crate::sys::swr_zend_try_stream_free(stream) } != 0 {
            return Err(Bailout);
        }
        // The close invalidated this resource. Discard its slot without a destructor.
        let _closed_resource = slot.take().into_zval();
        Ok(true)
    }
}

/// The bytes [`Value::read_stream`] asks a stream for at a time: PHP's own chunk size.
pub const STREAM_PAGE: usize = 8192;

/// A `php_stream_context` that lives for `'a`. Rust never makes one: it is a
/// parameter, or a return value, of this type on the C boundary, where Zend's
/// non-null `php_stream_context *` (a factory's argument, a stream's context)
/// stands for the constructor, as a `&T` there does.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct StreamContext<'a>(
    NonNull<php_stream_context>,
    PhantomData<&'a php_stream_context>,
);

impl<'a> StreamContext<'a> {
    #[must_use]
    pub fn as_ptr(self) -> *mut php_stream_context {
        self.0.as_ptr()
    }

    /// The context's options (`wrapper => [name => value]`), none when they are
    /// no array. They live until PHP code changes the context.
    #[must_use]
    pub fn options(self) -> Option<&'a Array> {
        swr_zend_stream_context_options(self)
    }
}

/// Outcome of casting a PHP resource with `PHP_STREAM_AS_SOCKETD`.
pub enum StreamSocket {
    Descriptor(c_int),
    InvalidResource,
    CastFailed,
}

impl Value {
    /// Cast the resource to a socket descriptor, preserving Zend's resource error.
    /// # Errors
    /// The stream wrapper bailed out during the cast.
    pub fn stream_socket(&self) -> Result<StreamSocket, Bailout> {
        let mut stream = None;
        let mut fd = -1;
        match swr_zend_stream_socketd(self, &mut stream, &mut fd) {
            0 => Ok(StreamSocket::Descriptor(fd)),
            2 => Err(Bailout),
            _ if stream.is_none() => Ok(StreamSocket::InvalidResource),
            _ => Ok(StreamSocket::CastFailed),
        }
    }

    /// Disable stream read buffering, trapping a user stream wrapper's bailout.
    /// # Errors
    /// The stream wrapper bailed out while changing the read buffer.
    pub fn unbuffer_stream(&self) -> Result<(), Bailout> {
        contain_with(|| {
            unsafe extern "C" {
                // Only called inside contain_with, which traps the wrapper's bailout.
                safe fn swr_zend_stream_unbuffer(value: &Value);
            }
            swr_zend_stream_unbuffer(self);
        })
    }

    /// `instanceof_function(Z_OBJCE_P(value), class)`: false for a non-object or
    /// a class not registered.
    #[must_use]
    pub fn instance_of(&self, class: &Class) -> bool {
        match (self.class_entry(), class.class_entry()) {
            (Some(own), Some(parent)) => own.instance_of(parent),
            _ => false,
        }
    }

    /// `php_stream_from_zval_no_verify`: the stream a resource this holds
    /// names, or null for any other value or resource. A pointer: the
    /// resource, not this borrow, keeps the stream.
    #[must_use]
    pub fn stream(&self) -> *mut crate::sys::php_stream {
        swr_zend_stream_from_zval(self)
    }

    /// `php_stream_read` of the stream a resource this holds names, on the request's
    /// thread: the bytes now at the front of `page`, empty at the end of the stream or
    /// after a read error; `None` for any other value or resource.
    /// # Errors
    /// The stream's wrapper bailed out.
    pub fn read_stream<'a>(
        &self,
        page: &'a mut [u8; STREAM_PAGE],
    ) -> Result<Option<&'a [u8]>, Bailout> {
        let count = swr_zend_try_stream_zval_read(self, page, STREAM_PAGE);
        match usize::try_from(count) {
            Ok(count) => Ok(Some(page.split_at(count.min(STREAM_PAGE)).0)),
            Err(_) if count == -1 => Ok(None),
            Err(_) => Err(Bailout),
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
        let mut fd = -1;
        match swr_zend_stream_select_fd(self, &mut fd) {
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
        let (mut long, mut found) = (0, false);
        if swr_zend_event_property_long(self, name, &mut long, &mut found) != 0 {
            return Err(Bailout);
        }
        Ok(found.then_some(long))
    }

    /// An ext/sockets `Socket`'s descriptor and whether it was closed, if this
    /// is one and PHP has ext/sockets.
    #[must_use]
    pub fn php_socket_fd(&self) -> Option<(c_int, bool)> {
        self.object()?;
        let mut closed = 0;
        let fd = swr_zend_event_socket_fd(self, &mut closed);
        (fd != -2).then_some((fd, closed != 0))
    }
}

/// `f` of the executor's last error, when it was fatal: its message and file,
/// which PHP keeps until the next error, and its line.
fn fatal_parts<R>(f: impl FnOnce([&CStr; 2], c_int) -> R) -> Option<R> {
    let (mut message, mut file, mut line) = (None, None, 0);
    if !swr_zend_last_fatal_error(&mut message, &mut file, &mut line) {
        return None;
    }
    Some(f([message?.get(), file?.get()], line))
}

/// `php_swoole_is_fatal_error()`: the last error was fatal.
#[must_use]
pub fn last_fatal_error() -> bool {
    fatal_parts(|_, _| ()).is_some()
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
    fatal_parts(|[message, file], line| FatalError {
        message: message.to_owned(),
        file: file.to_owned(),
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
/// as over any frame below one that raises it ([`crate::exports`]), so those
/// frames own nothing that needs a drop when PHP may bail out.
/// # Errors
/// `body` bailed out.
pub fn contain<T: Copy>(body: fn() -> T) -> Result<T, Bailout> {
    let mut out = None;
    contain_with(|| out = Some(body()))?;
    out.ok_or(Bailout)
}

/// [`contain`] of a closure that needs no drop: it captures borrows and plain
/// values, and hands its result back through one.
/// # Errors
/// `body` bailed out.
pub fn contain_with<F: FnOnce()>(body: F) -> Result<(), Bailout> {
    /// A borrowed stack closure behind the shim's one opaque pointer.
    struct Context<'a>(&'a mut dyn FnMut());
    extern "C" fn run(context: &mut Context<'_>) {
        (context.0)();
    }
    #[expect(
        improper_ctypes,
        reason = "C passes the context pointer back to run and never reads its Rust fields"
    )]
    unsafe extern "C" {
        /// The native trap keeps both borrows live and calls `run` with this
        /// same context, synchronously, without retaining either argument.
        #[link_name = "swr_zend_try"]
        safe fn try_body(run: extern "C" fn(&mut Context<'_>), context: &mut Context<'_>) -> c_int;
    }
    const {
        assert!(
            !std::mem::needs_drop::<F>(),
            "a bailout skips the drop of what the trap holds"
        );
    };
    let mut slot = Some(body);
    let mut once = || {
        if let Some(body) = slot.take() {
            body();
        }
    };
    bailout::trapped(try_body(run, &mut Context(&mut once)))?;
    // A Rust body returns normally after deferring an escape. Report it to
    // the owner without jumping back through the body's cleanup.
    bailout::outcome(())
}
