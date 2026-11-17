// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 ext-src/swoole_curl.cc (Swoole contributors).
//! What a [`Multi`] needs of the process: the reactor (`corebase::reactor`), this
//! thread's timer (crates/timer) and coroutine scheduler (crates/corebase/src/coroutine),
//! and the C entry points the bundled `ext/curl` calls besides the multi's own.
//! The engine uses an explicit operation table in every build.
use crate::{EXEC_NEEDS_COROUTINE, Idle, Multi};
use std::cell::Cell;
use std::ffi::{CStr, c_int, c_long, c_void};
use std::io::Write;
use std::{ptr, rc::Rc};
use swoole_rs_corebase::coroutine::Resume;
use swoole_rs_corebase::reactor::Events;
use swoole_rs_corebase::reactor::{SocketRecord, event_api};
use swoole_rs_curl_sys::{
    CURL_CSELECT_ERR, CURL_CSELECT_IN, CURL_CSELECT_OUT, CURLE_FAILED_INIT, CURLcode,
};
use swoole_rs_tls::event_socket::EventSocket;

/// Effects borrowed by a multi for its whole lifetime. Socket records and their
/// ownership are shared by native and recording callbacks.
pub(crate) struct Ops {
    pub event_available: fn() -> bool,
    pub install_handlers: fn(),
    pub socket_new: fn(c_int) -> EventSocket,
    pub socket_add: fn(&EventSocket, Events) -> bool,
    pub socket_set: fn(&EventSocket, Events) -> bool,
    pub socket_del: fn(&EventSocket) -> bool,
    pub socket_release: fn(EventSocket),
    pub timer_available: fn() -> bool,
    pub timer_add: fn(c_long, &Multi) -> Option<swoole_rs_timer::Held>,
    pub timer_del: fn(swoole_rs_timer::Held),
    pub current: fn() -> *mut c_void,
    pub current_cid: fn() -> c_long,
    pub yield_: fn(*mut c_void, f64, &mut dyn FnMut(swoole_rs_corebase::coroutine::Waker)) -> bool,
    pub wake: fn(swoole_rs_corebase::coroutine::Waker),
    pub defer: fn(&Multi),
}

pub(crate) static NATIVE: Ops = Ops {
    event_available,
    install_handlers,
    socket_new,
    socket_add: EventSocket::add,
    socket_set: EventSocket::set,
    socket_del: EventSocket::del,
    socket_release: EventSocket::release,
    timer_available,
    timer_add,
    timer_del,
    current,
    current_cid,
    yield_,
    wake,
    defer,
};

fn socket_new(fd: c_int) -> EventSocket {
    EventSocket::adopt(Box::new(SocketRecord::with_fd(fd, FD_CO_CURL)))
}

/// Upstream's `PHP_SWOOLE_FD_CO_CURL`: `SW_FD_STREAM_CLIENT + 10`.
const FD_CO_CURL: c_int = 17 + 10;
/// `SW_ERROR_CO_OUT_OF_COROUTINE` and `SW_ERROR_CO_HAS_BEEN_BOUND`.
const CO_OUT_OF_COROUTINE: c_int = 10001;
const CO_HAS_BEEN_BOUND: c_int = 10002;

/// The reactor's handler for the cURL fd type: `BITMASK` happened on the
/// socket whose object is a [`crate::Socket`].
fn on_event<const BITMASK: c_int>(
    _: swoole_rs_corebase::reactor::Running<'_>,
    event: &mut swoole_rs_corebase::reactor::Event,
) -> c_int {
    let Some(socket) = event.socket() else {
        return -1;
    };
    let object = socket.read(|socket| socket.object).cast::<crate::Socket>();
    // SAFETY: the multi set the object, its socket, which it keeps live while the
    // reactor socket is registered.
    crate::socket_event(unsafe { &*object }, BITMASK);
    0
}

pub(crate) fn event_available() -> bool {
    event_api::swoole_event_is_available()
}

/// Install the cURL fd type's reactor handlers unless they are.
pub(crate) fn install_handlers() {
    if event_api::isset_handler(FD_CO_CURL, Events::READ) {
        return;
    }
    let read: swoole_rs_corebase::reactor::Handler = on_event::<CURL_CSELECT_IN>;
    for (event, handler) in [
        (Events::READ, read),
        (Events::WRITE, on_event::<CURL_CSELECT_OUT>),
        (Events::ERROR, on_event::<CURL_CSELECT_ERR>),
    ] {
        event_api::set_handler(FD_CO_CURL, event, Some(handler));
    }
}

/// A reactor `network::Socket` of the cURL fd type over curl's fd, owned from
/// [`ReactorSocket::new`] until [`ReactorSocket::free`]; a freed one does nothing. The
/// multi touches it only on the thread whose event loop it registers with.
pub(crate) struct ReactorSocket {
    socket: Cell<Option<EventSocket>>,
    ops: &'static Ops,
}

impl ReactorSocket {
    /// Out of the reactor, carrying no object yet.
    pub(crate) fn new(fd: c_int, ops: &'static Ops) -> Self {
        Self {
            socket: Cell::new(Some((ops.socket_new)(fd))),
            ops,
        }
    }

    /// `f` of the record while it is owned, else the default.
    fn with<R: Default>(&self, f: impl FnOnce(&EventSocket) -> R) -> R {
        let socket = self.socket.take();
        let result = socket.as_ref().map_or_else(R::default, f);
        self.socket.set(socket);
        result
    }

    /// Carry `object`, which the reactor handlers report.
    pub(crate) fn set_object(&self, object: *mut c_void) {
        self.with(|socket| socket.socket().edit(|socket| socket.object = object));
    }

    /// The events it is registered for.
    pub(crate) fn events(&self) -> Events {
        self.with(EventSocket::events)
    }

    /// Whether it is out of the reactor.
    pub(crate) fn removed(&self) -> bool {
        self.with(EventSocket::removed)
    }

    pub(crate) fn add(&self, events: Events) -> bool {
        self.with(|socket| (self.ops.socket_add)(socket, events))
    }

    pub(crate) fn set(&self, events: Events) -> bool {
        self.with(|socket| (self.ops.socket_set)(socket, events))
    }

    pub(crate) fn del(&self) -> bool {
        self.with(self.ops.socket_del)
    }

    /// Leave the reactor silently if in it, then free the socket without
    /// closing the fd, which curl owns.
    pub(crate) fn free(&self) {
        let Some(socket) = self.socket.take() else {
            return;
        };
        if !socket.events().is_empty() && (self.ops.event_available)() {
            socket.socket().edit(|socket| socket.silent_remove = true);
            _ = (self.ops.socket_del)(&socket);
        }
        socket.socket().edit(|socket| socket.fd = -1);
        (self.ops.socket_release)(socket);
    }
}

pub(crate) fn timer_available() -> bool {
    swoole_rs_timer::swoole_timer_is_available()
}

/// A one-shot timer of `ms` that reaches `multi` through its own `Weak`: its id, or none.
pub(crate) fn timer_add(ms: c_long, multi: &Multi) -> Option<swoole_rs_timer::Held> {
    let multi = multi.this.clone();
    swoole_rs_timer::add_owned(ms, false, move || {
        if let Some(multi) = multi.upgrade() {
            multi.timer.set(None);
            multi.callback(None);
        }
    })
}

/// `swoole_timer_del` of the timer [`timer_add`] returned, by its id.
pub(crate) use swoole_rs_timer::cancel_held as timer_del;

/// `Coroutine::get_current()`.
pub(crate) fn current() -> *mut c_void {
    swoole_rs_corebase::coroutine::current_native()
}

/// `Coroutine::get_current_cid()`.
pub(crate) use swoole_rs_corebase::coroutine::current_cid;

/// `co->yield_ex(timeout)` of `co`, the running coroutine (aborting otherwise), then
/// whether it was canceled.
pub(crate) fn yield_(
    co: *mut c_void,
    timeout: f64,
    arm: &mut dyn FnMut(swoole_rs_corebase::coroutine::Waker),
) -> bool {
    let code = swoole_rs_corebase::coroutine::yield_timed_armed(co, timeout, arm);
    // `yield_ex`: the last error of a timeout or a cancel.
    !swoole_rs_corebase::coroutine::yield_resumed(code) && code == Resume::Canceled
}

/// `co->resume()` of a coroutine parked where `waker` was made, if it still is.
pub(crate) fn wake(waker: swoole_rs_corebase::coroutine::Waker) {
    waker.wake();
}

/// A multi's queued wake-up, which holds the multi. `defer` marks the multi before it
/// queues one, and `swr_v1_curl_multi_free` leaves a marked multi to its wake-up.
struct Wake(Rc<Multi>);

impl event_api::Deferred for Wake {
    fn run(self) {
        if crate::multi_deferred(&self.0) {
            // Released while the wake-up was queued: this is its last reference.
            self.0.close();
        }
    }

    /// A reactor freed first runs no wake-up, as `swoole_event_defer`'s queue,
    /// which leaks the multi.
    fn dispose(self) {
        #[expect(
            clippy::mem_forget,
            reason = "upstream's queue leaks the multi a wake-up it never runs holds"
        )]
        std::mem::forget(self.0);
    }
}

/// `swoole_event_defer` of the multi's wake-up, unless one is queued; nothing without
/// an event loop.
pub(crate) fn defer(multi: &Multi) {
    if multi.defer_callback.replace(true) {
        return;
    }
    if let Some(multi) = multi.this.upgrade() {
        _ = event_api::swoole_event_defer_value(Wake(multi));
    }
}

/// `swoole_curl_multi_new`: a multi for `curl_multi_init`, whose reference C++
/// holds until `swr_v1_curl_multi_free`.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_curl_multi_new() -> *mut Multi {
    Rc::into_raw(Multi::create()).cast_mut()
}

/// The fatal error a coroutine may not operate a multi with: its code and message.
pub(crate) type Unbound<'b> = (c_int, &'b CStr);

/// The coroutine that may operate `multi`: the current one, unless none runs or
/// another one waits on the multi. The error's message is NUL-terminated in
/// `buffer`, on the caller's stack: the fatal error may longjmp past that frame.
pub(crate) fn bound_co<'b>(
    multi: &Multi,
    buffer: &'b mut [u8; 128],
) -> Result<*mut c_void, Unbound<'b>> {
    let Some(waiter) = multi.co.get() else {
        let co = (multi.host.current)();
        if co.is_null() {
            return Err((CO_OUT_OF_COROUTINE, c"API must be called in the coroutine"));
        }
        return Ok(co);
    };
    swoole_rs_corebase::coroutine::set_socket_bound_cid(waiter.cid);
    let (mut text, _) = buffer.split_at_mut(127);
    // A longer message is cut short.
    write!(
        text,
        "This cURL handle is currently executing in coroutine#{}, cannot be operated",
        waiter.cid
    )
    .unwrap_or_default();
    let message = CStr::from_bytes_until_nul(buffer).unwrap_or(c"");
    Err((CO_HAS_BEEN_BOUND, message))
}

/// `swoole_curl_check_bound_co`: the current coroutine, or null after the
/// fatal error of a multi another coroutine waits on.
/// # Safety
/// The fatal error may longjmp: the caller's frames own nothing that needs a drop.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_curl_check_bound_co(multi: &Multi) -> *mut c_void {
    let mut buffer = [0; 128];
    let (code, message) = match bound_co(multi, &mut buffer) {
        Ok(co) => return co,
        Err(unbound) => unbound,
    };
    // SAFETY: Swoole's fatal error hook; nothing here needs a drop.
    unsafe { swoole_rs_corebase::fatal_error(code, message) };
    ptr::null_mut()
}

/// `swoole_curl_easy_perform`: `curl_easy_perform` in the handle's own multi;
/// `CURLE_FAILED_INIT` without a Swoole handle.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_curl_easy_perform(cp: Idle<'_>) -> CURLcode {
    let Some(handle) = cp.0.handle() else {
        return CURLE_FAILED_INIT;
    };
    // The handle's own multi, held while it runs the handle.
    let multi = handle.easy_multi.take().unwrap_or_else(Multi::create);
    handle.easy_multi.set(Some(Rc::clone(&multi)));
    let result = multi.exec(handle);
    let mut buffer = [0; 128];
    let unbound = if result == EXEC_NEEDS_COROUTINE {
        bound_co(&multi, &mut buffer).err()
    } else {
        None
    };
    // The fatal error may longjmp past this frame, so it gives up its `Rc` first.
    drop(multi);
    if let Some((code, message)) = unbound {
        // SAFETY: Swoole's fatal error hook; this frame now owns nothing that needs a drop.
        unsafe { swoole_rs_corebase::fatal_error(code, message) };
    }
    result
}
