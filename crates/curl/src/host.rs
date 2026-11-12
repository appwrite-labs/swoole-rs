// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 ext-src/swoole_curl.cc (Swoole contributors).
//! What a [`Multi`] needs of the process: the reactor (crates/reactor), this
//! thread's timer (crates/timer) and coroutine scheduler (crates/coroutine),
//! and the C entry points the bundled `ext/curl` calls besides the multi's own.
//! The tests swap in a fake with the same functions.
use crate::{EXEC_NEEDS_COROUTINE, Multi, swr_v1_curl_get_handle};
use std::ffi::{CStr, c_int, c_long, c_void};
use std::io::Write;
use std::ptr;
use swoole_rs_coroutine::{Coroutine, RC_CANCELED, scheduler};
use swoole_rs_curl_sys::{CURL, CURL_CSELECT_ERR, CURL_CSELECT_IN, CURL_CSELECT_OUT, CURLcode};
use swoole_rs_reactor::{Socket, SocketRecord, event_api};

/// Upstream's `PHP_SWOOLE_FD_CO_CURL`: `SW_FD_STREAM_CLIENT + 10`.
const FD_CO_CURL: c_int = 17 + 10;
/// `SW_EVENT_READ`, `SW_EVENT_WRITE` and `SW_EVENT_ERROR`.
const EVENT_READ: c_int = 1 << 9;
const EVENT_WRITE: c_int = 1 << 10;
const EVENT_ERROR: c_int = 1 << 11;
/// `SW_ERROR_CO_OUT_OF_COROUTINE` and `SW_ERROR_CO_HAS_BEEN_BOUND`.
const CO_OUT_OF_COROUTINE: c_int = 10001;
const CO_HAS_BEEN_BOUND: c_int = 10002;

/// The reactor's handler for the cURL fd type: `BITMASK` happened on the
/// socket whose object is a [`crate::Socket`].
extern "C" fn on_event<const BITMASK: c_int>(
    _: swoole_rs_reactor::Running<'_>,
    event: &mut swoole_rs_reactor::Event,
) -> c_int {
    let Some(socket) = event.socket() else {
        return -1;
    };
    let object = socket.read(|socket| socket.object);
    // SAFETY: the multi set the object and keeps it live while the socket is registered.
    unsafe { crate::socket_event(object, BITMASK) };
    0
}

pub(crate) fn event_available() -> bool {
    event_api::swoole_event_is_available()
}

/// Install the cURL fd type's reactor handlers unless they are.
pub(crate) fn install_handlers() {
    if event_api::swoole_event_isset_handler(FD_CO_CURL, EVENT_READ) {
        return;
    }
    for (event, handler) in [
        (
            EVENT_READ,
            on_event::<CURL_CSELECT_IN> as swoole_rs_reactor::Handler,
        ),
        (EVENT_WRITE, on_event::<CURL_CSELECT_OUT>),
        (EVENT_ERROR, on_event::<CURL_CSELECT_ERR>),
    ] {
        // SAFETY: `on_event` needs only the live event of a cURL socket.
        unsafe { event_api::swoole_event_set_handler(FD_CO_CURL, event, Some(handler)) };
    }
}

/// A reactor `network::Socket` of the cURL fd type over curl's fd. It is
/// live from [`ReactorSocket::new`] until [`ReactorSocket::free`], and the
/// multi touches it only on the thread whose event loop it registers with.
#[derive(Clone, Copy)]
pub(crate) struct ReactorSocket(Socket<'static>);

impl ReactorSocket {
    /// Out of the reactor, carrying no object yet.
    pub(crate) fn new(fd: c_int) -> Self {
        let record = SocketRecord::with_fd(fd, FD_CO_CURL).into_raw();
        // SAFETY: the new record, live until `free`, and touched only through this handle
        // and the reactor on this thread.
        let socket = unsafe { Socket::from_ptr(record) };
        Self(socket.unwrap_or_else(|| std::process::abort()))
    }

    fn record(self) -> *mut SocketRecord {
        self.0.as_ptr()
    }

    /// Carry `object`, which the reactor handlers report.
    pub(crate) fn set_object(self, object: *mut c_void) {
        self.0.edit(|socket| socket.object = object);
    }

    /// The events it is registered for.
    pub(crate) fn events(self) -> c_int {
        self.0.read(|socket| socket.events)
    }

    /// Whether it is out of the reactor.
    pub(crate) fn removed(self) -> bool {
        self.0.read(|socket| socket.removed != 0)
    }

    pub(crate) fn add(self, events: c_int) -> c_int {
        // SAFETY: a live record; the multi registers it while the loop is available.
        unsafe { event_api::swoole_event_add(self.record(), events) }
    }

    pub(crate) fn set(self, events: c_int) -> c_int {
        // SAFETY: as above.
        unsafe { event_api::swoole_event_set(self.record(), events) }
    }

    pub(crate) fn del(self) -> c_int {
        // SAFETY: as above.
        unsafe { event_api::swoole_event_del(self.record()) }
    }

    /// Leave the reactor silently if in it, then free the socket without
    /// closing the fd, which curl owns.
    /// # Safety
    /// Nothing uses this socket, or a copy of it, afterwards.
    pub(crate) unsafe fn free(self) {
        if self.events() != 0 && event_available() {
            self.0.edit(|socket| socket.silent_remove = 1);
            self.del();
        }
        self.0.edit(|socket| socket.fd = -1);
        // SAFETY: per the caller; with fd -1 it closes nothing.
        unsafe { swoole_rs_tls::upstream::socket_free(self.record()) };
    }
}

pub(crate) fn timer_available() -> bool {
    swoole_rs_timer::swoole_timer_is_available()
}

/// The multi's timer fired.
///
/// # Safety
/// `multi` is the live `Multi` that armed the timer.
unsafe extern "C-unwind" fn on_timer(multi: *mut c_void) {
    // SAFETY: per the caller; the multi deletes its timer before it is freed.
    let multi = unsafe { &*multi.cast::<Multi>() };
    multi.timer.set(std::ptr::null_mut());
    multi.callback(None);
}

/// A one-shot timer of `ms` that reaches `multi`, which deletes it, unless it
/// fired, before it is freed.
pub(crate) fn timer_add(ms: c_long, multi: &Multi) -> *mut c_void {
    let multi = ptr::from_ref(multi).cast_mut().cast();
    // SAFETY: `on_timer` takes the multi, which deletes the timer before it goes.
    unsafe { swoole_rs_timer::swr_v1_timer_add_plain(ms, false, on_timer, multi) }
}

/// # Safety
/// `timer` is pending.
pub(crate) unsafe fn timer_del(timer: *mut c_void) {
    // SAFETY: per the caller.
    unsafe { swoole_rs_timer::swoole_timer_del(timer.cast()) };
}

/// `Coroutine::get_current()`.
pub(crate) fn current() -> *mut c_void {
    swoole_rs_coroutine::current_native()
}

/// `co->yield_ex(timeout)`, then whether it was canceled.
/// # Safety
/// `co` is the running coroutine; the caller's frames own nothing with a destructor.
pub(crate) unsafe fn yield_(co: *mut c_void, timeout: f64) -> bool {
    // SAFETY: per the caller.
    let code = unsafe { swoole_rs_coroutine::yield_timed(co, timeout) };
    // `yield_ex`: the last error of a timeout or a cancel.
    !swoole_rs_coroutine::yield_resumed(code) && code == RC_CANCELED
}

/// # Safety
/// `co` is a live, suspended coroutine.
pub(crate) unsafe fn resume(co: *mut c_void) {
    // SAFETY: per the caller.
    unsafe { swoole_rs_coroutine::resume(co) };
}

unsafe extern "C-unwind" fn deferred(multi: *mut c_void) {
    // SAFETY: the multi waits on the deferral it queued.
    if crate::multi_deferred(unsafe { &*multi.cast::<Multi>() }) {
        // SAFETY: released while the deferral was queued; nothing else uses it.
        unsafe { crate::swr_v1_curl_multi_free(multi.cast()) };
    }
}

/// `swoole_event_defer` of the multi's wake-up.
/// # Safety
/// The event loop is available and `multi` outlives the deferral.
pub(crate) unsafe fn defer(multi: *const Multi) {
    // SAFETY: `deferred` takes the multi, which outlives the deferral as the caller promises.
    unsafe { swoole_rs_reactor::swoole_event_defer(deferred, multi.cast_mut().cast()) };
}

/// `swoole_curl_multi_new`: a multi for `curl_multi_init`.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_curl_multi_new() -> *mut Multi {
    Multi::create()
}

/// `swoole_curl_check_bound_co`: the current coroutine, or null after the
/// fatal error of a multi another coroutine waits on.
/// # Safety
/// The fatal error may longjmp: the caller's frames own nothing that needs a drop.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_curl_check_bound_co(multi: &Multi) -> *mut c_void {
    let co = multi.co.get();
    // SAFETY: the coroutine waiting on the multi is live.
    if let Some(co) = unsafe { co.cast::<Coroutine>().as_ref() } {
        // SAFETY: this thread's scheduler.
        unsafe { (*scheduler()).socket_bound_cid = co.cid };
        // On the stack, NUL-terminated: the fatal error may longjmp past this frame.
        let mut buffer = [0u8; 128];
        let (mut text, _) = buffer.split_at_mut(127);
        // A longer message is cut short.
        write!(
            text,
            "This cURL handle is currently executing in coroutine#{}, cannot be operated",
            co.cid
        )
        .unwrap_or_default();
        let message = CStr::from_bytes_until_nul(&buffer).unwrap_or(c"");
        // SAFETY: Swoole's fatal error hook; nothing here needs a drop.
        unsafe { swoole_rs_corebase::fatal_error(CO_HAS_BEEN_BOUND, message) };
        return std::ptr::null_mut();
    }
    let co = current();
    if co.is_null() {
        // SAFETY: as above; nothing here needs a drop.
        unsafe {
            swoole_rs_corebase::fatal_error(
                CO_OUT_OF_COROUTINE,
                c"API must be called in the coroutine",
            );
        };
    }
    co
}

/// `swoole_curl_easy_perform`: `curl_easy_perform` in the handle's own multi.
/// # Safety
/// `cp` is a live easy handle with a Swoole handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_curl_easy_perform(cp: *mut CURL) -> CURLcode {
    // SAFETY: per the caller.
    let handle = unsafe { swr_v1_curl_get_handle(cp) };
    // SAFETY: per the caller, the easy handle's live Swoole handle.
    let handle = unsafe { &mut *handle };
    if handle.easy_multi.is_null() {
        handle.easy_multi = Multi::create();
    }
    // SAFETY: the handle's own multi, which lives as long as the handle.
    let multi = unsafe { &*handle.easy_multi };
    let result = multi.exec(handle);
    if result == EXEC_NEEDS_COROUTINE {
        // SAFETY: this frame owns nothing that needs a drop.
        unsafe { swoole_curl_check_bound_co(multi) };
    }
    result
}
