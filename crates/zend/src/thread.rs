//! A PHP thread's life in Zend (crates/phpthread's `run`): TSRM's state for a
//! new thread ([`Entered`]), its request with the steps around its script
//! ([`Steps`]), the stdio streams its prologue opens ([`Stdio`]), the exit
//! that ends it ([`exit`]), and the main request each thread starts from.
//! Zend's setjmp, TSRM and request macros are zend-sys shims (`shim_thread.h`);
//! each is declared where a receiver or argument proves its precondition.
use crate::{Owned, Request, Value, sys::php_stream};
use std::{
    ffi::{CStr, CString, c_int},
    marker::PhantomData,
    ptr::NonNull,
    sync::{Mutex, PoisonError},
};
use swoole_rs_corebase::{Fresh, sys::Text};

/// A stream just opened, whose resource no zval holds yet: only
/// `swr_zend_stream_open` makes one, and closing it or handing it to a zval takes it.
#[repr(transparent)]
struct Opened(NonNull<php_stream>);

unsafe extern "C" {
    /// `php_stream_open_wrapper_ex(path, mode, 0, NULL, NULL)` in the request, or
    /// none. Only [`fd_stream`] and [`OPENING`]'s holder ([`Opening::open`]) call it.
    safe fn swr_zend_stream_open(path: Text<'_>, mode: Text<'_>) -> Option<Opened>;
    /// The stream's resource into `value`, in place of what it held, which is not
    /// released: the zval takes the stream's one reference.
    safe fn swr_zend_stream_to_zval(stream: Opened, value: &Owned);
}

/// `php_swoole_thread_stream_create`'s open: `php://fd/<fd>` (which duplicates
/// the descriptor), as a resource zval, in a request; none if it failed.
#[must_use]
pub fn fd_stream(fd: c_int) -> Option<Owned> {
    let path = CString::new(format!("php://fd/{fd}")).unwrap_or_default();
    let stream = swr_zend_stream_open(path.as_c_str().into(), c"".into())?;
    let out = Owned::default();
    swr_zend_stream_to_zval(stream, &out);
    Some(out)
}

/// A `php://stdin`, `stdout` or `stderr` stream a thread's prologue opened,
/// before its script runs, for no longer than the prologue (`'p`): no user stream
/// wrapper, error handler or constant exists yet, so closing it, casting it or
/// registering it as a constant runs no PHP code. Only [`Prologue::open_stdio`]
/// makes them, once per thread.
#[repr(transparent)]
pub struct Stdio<'p>(Opened, PhantomData<&'p ()>);

/// A [`Stdio`] lent for one call.
#[repr(transparent)]
struct Lent<'a>(NonNull<php_stream>, PhantomData<&'a mut Opened>);

// The prologue's stdio streams (`Stdio`).
unsafe extern "C" {
    /// `php_stream_close`.
    safe fn _php_stream_free(stream: Stdio<'_>, options: c_int) -> c_int;
    /// `php_stream_cast(stream, PHP_STREAM_AS_FD, fd, 0) == SUCCESS`.
    #[link_name = "swr_zend_stream_fd"]
    safe fn stdio_fd(stream: Lent<'_>, fd: &mut c_int) -> bool;
    /// Register the stream's resource as the constant `name`, which takes it.
    #[link_name = "swr_zend_register_stream_constant"]
    safe fn stdio_constant(name: Text<'_>, stream: Stdio<'_>);
    /// `PHP_STREAM_FLAG_NO_CLOSE` on the stream of the constant `name`, when it
    /// names a file stream: it outlives its resource.
    safe fn swr_zend_stream_constant_no_close(name: Text<'_>);
}

impl Stdio<'_> {
    /// The stream's descriptor in `fd`: whether the cast succeeded.
    pub fn fd(&mut self, fd: &mut c_int) -> bool {
        stdio_fd(Lent((self.0).0, PhantomData), fd)
    }

    /// Registers the streams as `STDIN`, `STDOUT` and `STDERR`, none of which exists
    /// yet, each `PHP_STREAM_FLAG_NO_CLOSE`: it outlives its resource.
    pub fn register(streams: [Self; 3]) {
        for (stream, name) in streams.into_iter().zip([c"STDIN", c"STDOUT", c"STDERR"]) {
            stdio_constant(name.into(), stream);
            swr_zend_stream_constant_no_close(name.into());
        }
    }
}

/// Opening `php://stdout` and `php://stderr` sets PHP's process-global
/// `cli_out` and `cli_err`: one thread at a time, the one holding this lock.
static OPENING: Mutex<Opening> = Mutex::new(Opening(()));

/// The right to open the stdio streams, which [`OPENING`] holds.
struct Opening(());

impl Opening {
    /// `path` opened in `mode`, or none.
    #[expect(
        clippy::unused_self,
        reason = "the receiver proves the caller holds `OPENING`"
    )]
    fn open<'p>(&mut self, path: &CStr, mode: &CStr) -> Option<Stdio<'p>> {
        let stream = swr_zend_stream_open(path.into(), mode.into())?;
        Some(Stdio(stream, PhantomData))
    }
}

/// The steps of a thread's request around its script, inside the script's
/// `zend_first_try` (the thread's script scope).
pub trait Steps {
    /// Prepare the request and fill the argument slot when invoking an internal function.
    fn prologue(&mut self, prologue: Prologue<'_>, params: Option<&mut Owned>);
    /// Whether the caller transfers the result out of this request.
    fn wants_result(&self) -> bool;
    /// Copy the successful result before the request shuts down.
    fn capture(&mut self, result: &Value);
    /// After the script: `exited` if an `exit()` the thread's reactor has not seen
    /// is pending. The thread-exit mark is cleared after this returns.
    fn epilogue(&mut self, exited: bool);
}

impl crate::local::Php<'_> {
    /// Resolve a lowercase function or public static internal method without autoloading.
    #[must_use]
    pub fn internal_thread_entry(self, name: &crate::ZStr) -> bool {
        unsafe extern "C" {
            safe fn swr_zend_thread_is_internal(name: &crate::ZStr) -> bool;
        }
        swr_zend_thread_is_internal(name)
    }
}

/// A thread's request before its script runs, for the prologue's call: only
/// [`Entered::request`] makes one. A bailout raised in the prologue owns nothing.
pub struct Prologue<'a> {
    /// Whether `$argv` is set.
    argv: bool,
    _call: PhantomData<&'a mut *mut ()>,
}

impl<'a> Prologue<'a> {
    /// `$argv` (a copy of `argv`, which stays the caller's) and `$argc` in the
    /// request's symbol table, the first time; then nothing.
    pub fn set_argv(&mut self, argv: &Value, argc: i64) {
        unsafe extern "C" {
            /// Replacing an entry in the symbol table runs no PHP code: the
            /// prologue's (`&mut Prologue`, once) holds no `$argv` or `$argc` yet.
            #[link_name = "swr_zend_thread_set_argv"]
            safe fn set_argv(argv: &Value, argc: i64);
        }
        if !std::mem::replace(&mut self.argv, true) {
            set_argv(argv, argc);
        }
    }

    /// `php://stdin` (`rb`), `php://stdout` and `php://stderr` (`wb`) opened
    /// (`php_stream_open_wrapper_ex(path, mode, 0, NULL, NULL)`) under [`OPENING`];
    /// none, with those that opened closed, unless all three did.
    #[must_use]
    pub fn open_stdio(self) -> Option<[Stdio<'a>; 3]> {
        let mut opening = OPENING.lock().unwrap_or_else(PoisonError::into_inner);
        let streams = [
            (c"php://stdin", c"rb"),
            (c"php://stdout", c"wb"),
            (c"php://stderr", c"wb"),
        ]
        .map(|(path, mode)| opening.open(path, mode));
        drop(opening);
        let [Some(stdin), Some(stdout), Some(stderr)] = streams else {
            streams.into_iter().flatten().for_each(|stream| {
                _php_stream_free(stream, crate::sys::PHP_STREAM_FREE_CLOSE.cast_signed());
            });
            return None;
        };
        Some([stdin, stdout, stderr])
    }
}

/// A thread's exit status (`EG(exit_status)`), or 1 if its request didn't start.
#[derive(Clone, Copy)]
pub struct ExitStatus(pub c_int);

/// This thread inside TSRM (`swr_zend_thread_enter`) until [`Entered::leave`]: only
/// [`Entered::enter`] makes one, from the proof that PHP never ran on the thread.
/// Neither `Clone` nor `Send`.
pub struct Entered(PhantomData<*mut ()>);

/// The steps of one request, behind one thin pointer for C's `void *context`.
struct Context<'a>(&'a mut dyn Steps);

type Step = unsafe extern "C" fn(&mut Context<'_>);

crate::exports! {
extern "C" fn prologue(context: &mut Context<'_>, params: Option<&mut Owned>) {
    context.0.prologue(Prologue {
        argv: false,
        _call: PhantomData,
    }, params);
}
}

crate::exports! {
extern "C" fn capture(context: &mut Context<'_>, value: &Value) {
    context.0.capture(value);
}
}

extern "C" fn epilogue(context: &mut Context<'_>) {
    context.0.epilogue(guard_pending() != 0);
    guard_clear();
}

impl Entered {
    /// TSRM's state for this new thread.
    #[must_use]
    pub fn enter(_fresh: Fresh) -> Self {
        unsafe extern "C" {
            /// `ts_resource(0)` and the thread's PHP globals, on a new thread
            /// (`_fresh`, which this takes).
            safe fn swr_zend_thread_enter();
        }
        swr_zend_thread_enter();
        Self(PhantomData)
    }

    /// The thread's request: `file` as the primary script, with the main
    /// request's script path `path` and `argc`, which Zend keeps only while the
    /// request runs, inside this call. The script's `zend_first_try` (the thread's
    /// script scope) runs `steps`' prologue first and its epilogue after.
    pub fn request(
        &self,
        file: &CStr,
        internal: bool,
        path: Option<&CStr>,
        argc: c_int,
        steps: &mut dyn Steps,
    ) -> ExitStatus {
        #[expect(
            improper_ctypes,
            reason = "C passes the `Context` pointer on to the steps it was given and never reads it"
        )]
        unsafe extern "C" {
            /// A request on a thread inside TSRM (`&self`), between requests: it
            /// keeps none of its arguments past its return.
            safe fn swr_zend_thread_request(
                file: Text<'_>,
                internal: bool,
                path_translated: Option<Text<'_>>,
                argc: c_int,
                prologue: unsafe extern "C" fn(&mut Context<'_>, Option<&mut Owned>),
                capture: Option<unsafe extern "C" fn(&mut Context<'_>, &Value)>,
                epilogue: Step,
                context: &mut Context<'_>,
            ) -> c_int;
        }
        ExitStatus(swr_zend_thread_request(
            file.into(),
            internal,
            path.map(Text::from),
            argc,
            prologue,
            (crate::sys::PHP_VERSION_ID >= 80400 && steps.wants_result()).then_some(capture),
            epilogue,
            &mut Context(steps),
        ))
    }

    /// `ts_free_thread()`, once the thread's request is over.
    pub fn leave(self) {
        unsafe extern "C" {
            /// Frees the TSRM state of a thread that entered (`self`, which this takes).
            safe fn swr_zend_thread_leave();
        }
        swr_zend_thread_leave();
    }
}

// crates/zend-sys' shims that take nothing and can't run PHP or bail out: the calling
// thread's exit mark (`shim_guard.c`), and TSRM's process state.
unsafe extern "C" {
    /// The thread-exit mark: nonzero once `exit()` ran in this thread.
    #[link_name = "swr_zend_guard_pending"]
    safe fn guard_pending() -> c_int;
    /// Sets the thread-exit mark.
    #[link_name = "swr_zend_guard_mark"]
    safe fn guard_mark();
    /// Clears the thread-exit mark.
    #[link_name = "swr_zend_guard_clear"]
    safe fn guard_clear();
    /// `tsrm_is_shutdown()`; false on NTS.
    safe fn swr_zend_tsrm_is_shutdown() -> bool;
    /// `tsrm_api_name()`: a NUL-terminated literal (empty on NTS).
    safe fn swr_zend_tsrm_api_name() -> Text<'static>;
}

/// Request whole-thread exit at the next native boundary, after Rust cleanup.
pub fn defer_exit() {
    guard_mark();
    crate::bailout::Bailout.defer();
}

/// `tsrm_is_shutdown()`; false on NTS.
#[must_use]
pub fn is_shutdown() -> bool {
    swr_zend_tsrm_is_shutdown()
}

/// `tsrm_api_name()`: TSRM's static name (empty on NTS).
#[must_use]
pub fn api_name() -> &'static CStr {
    swr_zend_tsrm_api_name().get()
}

/// In a request (`_request`): SAPI's script path and `argc`, and the global `$argv`,
/// each live for the request, or none.
#[must_use]
pub fn main_request(_request: Request<'_>) -> (Option<&CStr>, c_int, Option<&Value>) {
    unsafe extern "C" {
        safe fn swr_zend_thread_main_request<'r>(
            path: &mut Option<Text<'r>>,
            argc: &mut c_int,
        ) -> Option<&'r Value>;
    }
    let (mut path, mut argc) = (None, 0);
    let argv = swr_zend_thread_main_request(&mut path, &mut argc);
    (path.map(Text::get), argc, argv)
}
