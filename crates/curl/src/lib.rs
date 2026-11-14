//! Swoole's coroutine cURL (upstream's `swoole_curl.cc`, built with
//! `--enable-swoole-curl`): the multi engine behind the native `curl_exec` and
//! `curl_multi_*` functions of its bundled `ext/curl`. A [`Multi`] drives a
//! libcurl multi handle through its socket and timer callbacks: each socket
//! curl watches joins the reactor, each timeout becomes a timer, and the
//! coroutine waiting on the transfer is resumed when either fires.
//!
//! The bundled `ext/curl` calls these exports directly (`swoole_rs_curl.h`).
//! The reactor, timer and coroutine operations are `host`'s; the tests fake
//! them. `swoole_curl_get_handle` reads the PHP objects (the `php` feature).
#![expect(
    clippy::disallowed_types,
    reason = "existing uses; tools/budget.py counts them down (docs/migration.md#data-layout)"
)]
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    ffi::{c_int, c_long, c_void},
    ptr,
    rc::{Rc, Weak},
};
use swoole_rs_corebase::log::{self, set_last_error};
use swoole_rs_curl_sys::{
    CURL, CURL_CSELECT_ERR, CURL_CSELECT_IN, CURL_CSELECT_OUT, CURL_POLL_IN, CURL_POLL_INOUT,
    CURL_POLL_OUT, CURL_POLL_REMOVE, CURL_SOCKET_TIMEOUT, CURLE_ABORTED_BY_CALLBACK,
    CURLE_FAILED_INIT, CURLE_OK, CURLINFO_PRIVATE, CURLM, CURLM_INTERNAL_ERROR, CURLM_OK,
    CURLMOPT_SOCKETDATA, CURLMOPT_SOCKETFUNCTION, CURLMOPT_TIMERDATA, CURLMOPT_TIMERFUNCTION,
    CURLMSG_DONE, CURLMcode, CURLOPT_PRIVATE, CURLcode, LiveMulti, MultiHandle, curl_easy_getinfo,
    curl_easy_reset, curl_easy_setopt, curl_multi_add_handle, curl_multi_cleanup,
    curl_multi_info_read, curl_multi_init, curl_multi_remove_handle, curl_multi_setopt,
    curl_multi_socket_action, curl_multi_timeout, curl_socket_t,
};

/// `SW_EVENT_READ` and `SW_EVENT_WRITE`.
const READ: c_int = swoole_rs_corebase::constants::EventType::SW_EVENT_READ as c_int;
const WRITE: c_int = swoole_rs_corebase::constants::EventType::SW_EVENT_WRITE as c_int;
/// `SW_OK` and `SW_ERR`.
const OK: c_int = swoole_rs_corebase::constants::ResultCode::SW_OK as c_int;
const ERR: c_int = swoole_rs_corebase::constants::ResultCode::SW_ERR as c_int;
/// `SW_ERROR_CO_CANCELED`.
const CO_CANCELED: c_int = 10017;
/// What [`swr_v1_curl_multi_exec`] returns when it must wait without a
/// coroutine to wait in (or one already bound): C++ raises the fatal error.
pub const EXEC_NEEDS_COROUTINE: CURLcode = -1;

/// A live easy handle, as the bundled `ext/curl` hands over its `ch->cp`. Only an
/// export's parameter makes one (and `php.rs`, reading a live `php_curl`'s), where the
/// C caller's contract stands for the constructor, as it does for a `&T` parameter
/// there. A `CURL *`'s layout, and neither `Send` nor `Sync`.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Easy(*mut CURL);

impl Easy {
    /// `CURLINFO_PRIVATE`: the easy handle's Swoole handle, or null without one.
    fn handle(self) -> *mut Handle {
        let mut handle: *mut Handle = ptr::null_mut();
        // SAFETY: the live easy handle; CURLINFO_PRIVATE writes a pointer.
        if unsafe { curl_easy_getinfo(self.0, CURLINFO_PRIVATE, &raw mut handle) } == CURLE_OK {
            handle
        } else {
            ptr::null_mut()
        }
    }

    /// Sets `CURLINFO_PRIVATE`, which [`Easy::handle`] reads back: null, or the
    /// Swoole handle [`swr_v1_curl_create_handle`] made for this easy handle.
    fn set_private(self, handle: *mut Handle) {
        // SAFETY: the live easy handle; CURLOPT_PRIVATE stores a pointer and reads nothing.
        unsafe { curl_easy_setopt(self.0, CURLOPT_PRIVATE, handle.cast::<c_void>()) };
    }
}

/// `struct swr_v1_curl_handle`: the easy handle's multi while it is in one,
/// and the multi `curl_exec` runs it in. C++ reads and writes `multi`.
#[repr(C)]
pub struct Handle {
    cp: *mut CURL,
    multi: *mut Multi,
    easy_multi: *mut Multi,
}

/// `swoole::curl::Socket`: one socket curl asked to watch. The multi's map
/// owns it, and the selector's copies keep one it removes while curl runs;
/// the reactor socket carries its address until it is freed.
struct Socket {
    /// The multi that made it, gone once that multi is freed: a socket curl
    /// never removed outlives it in the reactor.
    multi: Weak<Multi>,
    /// The reactor's `network::Socket`, freed when curl removes it.
    socket: host::ReactorSocket,
    bitmask: Cell<c_int>,
    sockfd: Cell<curl_socket_t>,
    action: Cell<c_int>,
    deleted: Cell<bool>,
}

/// `struct swr_v1_curl_multi`. Every field is a cell: curl calls back into the
/// same multi from inside its own calls, and other coroutines do while this
/// one waits, so no borrow is held across either. C++ holds one reference
/// ([`swoole_curl_multi_new`]) and a queued wake-up another; the socket and
/// timer callbacks reach it through [`Multi::this`].
pub struct Multi {
    /// This multi, for what reaches it later: its sockets, timer and wake-up.
    this: Weak<Multi>,
    /// None when libcurl could make no handle: its calls then refuse the null one.
    handle: Option<MultiHandle>,
    /// The id of the pending timer.
    timer: Cell<Option<swoole_rs_timer::Held>>,
    /// The coroutine waiting on this multi.
    co: Cell<*mut c_void>,
    running: Cell<c_int>,
    defer_callback: Cell<bool>,
    /// C++ let go of it while a deferred wake-up was queued: the wake-up
    /// closes it.
    released: Cell<bool>,
    /// The selector: a timeout to report to curl and sockets with events (a
    /// set, in arrival order).
    timer_callback: Cell<bool>,
    /// The sockets curl watches, by fd: what `curl_multi_assign` would carry.
    sockets: RefCell<BTreeMap<curl_socket_t, Rc<Socket>>>,
    active: RefCell<Vec<Rc<Socket>>>,
}

/// `Multi::get_event`: curl's poll action as reactor events.
fn events(action: c_int) -> c_int {
    match action {
        CURL_POLL_IN => READ,
        CURL_POLL_OUT => WRITE,
        CURL_POLL_INOUT => READ | WRITE,
        _ => 0,
    }
}

/// `translate_events_to_poll`.
fn poll_events(events: c_int) -> i16 {
    let mut poll = 0;
    if events & READ != 0 {
        poll |= libc::POLLIN;
    }
    if events & WRITE != 0 {
        poll |= libc::POLLOUT;
    }
    poll
}

/// The `CURL_CSELECT_*` bits of a `poll` result.
fn select_bits(revents: i16) -> c_int {
    let mut bitmask = 0;
    if revents & (libc::POLLIN | libc::POLLHUP) != 0 {
        bitmask |= CURL_CSELECT_IN;
    }
    if revents & libc::POLLOUT != 0 {
        bitmask |= CURL_CSELECT_OUT;
    }
    if revents & (libc::POLLERR | libc::POLLNVAL) != 0 {
        bitmask |= CURL_CSELECT_ERR;
    }
    bitmask
}

fn insert(set: &RefCell<Vec<Rc<Socket>>>, socket: &Rc<Socket>) {
    let mut set = set.borrow_mut();
    if !set.iter().any(|member| Rc::ptr_eq(member, socket)) {
        set.push(Rc::clone(socket));
    }
}

impl Multi {
    /// `Multi::Multi`: a multi handle whose socket and timer callbacks reach
    /// this multi, which therefore stays where it is until [`Multi::close`].
    fn create() -> Rc<Self> {
        let made = curl_multi_init();
        let handle = made.as_ref().map_or(ptr::null_mut(), MultiHandle::as_ptr);
        let multi = Rc::new_cyclic(|this| Self {
            this: this.clone(),
            handle: made,
            timer: Cell::new(None),
            co: Cell::new(ptr::null_mut()),
            running: Cell::new(0),
            defer_callback: Cell::new(false),
            released: Cell::new(false),
            timer_callback: Cell::new(false),
            sockets: RefCell::new(BTreeMap::new()),
            active: RefCell::new(Vec::new()),
        });
        let data = Rc::as_ptr(&multi).cast_mut().cast::<c_void>();
        for option in [CURLMOPT_SOCKETDATA, CURLMOPT_TIMERDATA] {
            // SAFETY: the handle just made (curl refuses a null one), and the multi,
            // which outlives it: only `Multi::close` cleans the handle up, before the
            // last reference to the multi goes.
            unsafe { curl_multi_setopt(handle, option, data) };
        }
        let on_socket: OnSocket = handle_socket;
        let on_timer: OnTimer = handle_timeout;
        // SAFETY: as above. Curl calls the callback with its own arguments and the
        // socket data set above, the live multi, which is what `OnSocket` takes.
        unsafe { curl_multi_setopt(handle, CURLMOPT_SOCKETFUNCTION, on_socket) };
        // SAFETY: as above, with the timer data, for `OnTimer`.
        unsafe { curl_multi_setopt(handle, CURLMOPT_TIMERFUNCTION, on_timer) };
        multi
    }

    /// The libcurl handle, or null without one, for the calls that may run callbacks.
    fn raw(&self) -> *mut CURLM {
        let handle = self.handle.as_ref();
        handle.map_or(ptr::null_mut(), MultiHandle::as_ptr)
    }

    /// The libcurl handle, live while the multi is borrowed.
    fn live(&self) -> Option<LiveMulti<'_>> {
        self.handle.as_ref().map(MultiHandle::live)
    }

    fn wait_event(&self) -> bool {
        self.timer.get().is_some() || !self.sockets.borrow().is_empty()
    }

    fn add_timer(&self, timeout_ms: c_long) {
        self.del_timer();
        self.timer.set(host::timer_add(timeout_ms, self));
    }

    fn del_timer(&self) {
        if let Some(timer) = self.timer.get()
            && host::timer_available()
        {
            host::timer_del(timer);
            self.timer.set(None);
        }
    }

    /// `curl_multi_timeout`: its result and the timeout, `unset` if curl wrote none.
    fn timeout(&self, unset: c_long) -> (CURLMcode, c_long) {
        let mut timeout_ms = unset;
        let result = curl_multi_timeout(self.live(), &mut timeout_ms);
        (result, timeout_ms)
    }

    fn set_timer(&self) {
        self.on_timeout(self.timeout(0).1);
    }

    /// `Multi::handle_timeout`.
    fn on_timeout(&self, timeout_ms: c_long) -> c_int {
        if !host::event_available() {
            return -1;
        }
        if timeout_ms < 0 {
            if self.timer.get().is_none() {
                self.add_timer(1000);
            } else {
                self.del_timer();
            }
        } else {
            // 0 means "call socket_action now": do it in a moment.
            self.add_timer(timeout_ms.max(1));
        }
        0
    }

    /// `Multi::set_event`: whether the socket is in the reactor for `action`.
    fn set_event(&self, sockfd: curl_socket_t, action: c_int) -> bool {
        if !host::event_available() {
            return false;
        }
        host::install_handlers();
        let known = self.sockets.borrow().get(&sockfd).cloned();
        let socket = known.unwrap_or_else(|| {
            let socket = Rc::new(Socket {
                multi: self.this.clone(),
                socket: host::ReactorSocket::new(sockfd),
                bitmask: Cell::new(0),
                sockfd: Cell::new(0),
                action: Cell::new(0),
                deleted: Cell::new(false),
            });
            // The reactor socket carries the socket it reports.
            socket
                .socket
                .set_object(Rc::as_ptr(&socket).cast_mut().cast());
            self.sockets.borrow_mut().insert(sockfd, Rc::clone(&socket));
            socket
        });
        socket.sockfd.set(sockfd);
        socket.action.set(action);
        let events = events(action);
        if socket.socket.events() == 0 {
            socket.socket.add(events)
        } else {
            socket.socket.set(events)
        }
    }

    /// `Multi::del_event`: whether curl's socket was known.
    fn del_event(&self, sockfd: curl_socket_t) -> bool {
        let Some(socket) = self.sockets.borrow_mut().remove(&sockfd) else {
            return false;
        };
        self.active
            .borrow_mut()
            .retain(|active| !Rc::ptr_eq(active, &socket));
        // SAFETY: the socket's reactor socket, which nothing uses afterwards: it
        // is out of the map, and the selector skips a deleted socket.
        unsafe { socket.socket.free() };
        socket.deleted.set(true);
        true
    }

    fn add_handle(&self, handle: &mut Handle) -> CURLMcode {
        // SAFETY: this multi's handle and the handle's live easy handle.
        let result = unsafe { curl_multi_add_handle(self.raw(), handle.cp) };
        if result == CURLM_OK {
            handle.multi = ptr::from_ref(self).cast_mut();
        }
        result
    }

    fn remove_handle(&self, handle: &mut Handle) -> CURLMcode {
        // SAFETY: this multi's handle and the handle's live easy handle.
        let result = unsafe { curl_multi_remove_handle(self.raw(), handle.cp) };
        handle.multi = ptr::null_mut();
        if result == CURLM_OK {
            // Refresh the running count even if no socket event follows.
            self.timer_callback.set(true);
        }
        result
    }

    /// `Multi::selector_prepare`: put back in the reactor the sockets a
    /// waiter-less event took out of it.
    fn selector_prepare(&self) {
        let sockets: Vec<Rc<Socket>> = self.sockets.borrow().values().cloned().collect();
        for socket in sockets {
            if socket.socket.removed() {
                socket.socket.add(events(socket.action.get()));
            }
        }
    }

    /// `Multi::exec`: run one easy handle to completion (`curl_exec`).
    fn exec(&self, handle: &mut Handle) -> CURLcode {
        if self.add_handle(handle) != CURLM_OK {
            return CURLE_FAILED_INIT;
        }
        let mut canceled = false;
        loop {
            self.selector_prepare();
            if self.wait_event() {
                let co = host::current();
                if !self.co.get().is_null() || co.is_null() {
                    return EXEC_NEEDS_COROUTINE;
                }
                self.co.set(co);
                canceled = host::yield_(co, -1.0);
                self.co.set(ptr::null_mut());
                if canceled {
                    set_last_error(CO_CANCELED);
                    break;
                }
            }
            self.selector_finish();
            if self.running.get() == 0 {
                break;
            }
            self.set_timer();
        }
        self.del_timer();
        let result = self.read_info();
        self.remove_handle(handle);
        if canceled {
            CURLE_ABORTED_BY_CALLBACK
        } else {
            result
        }
    }

    /// `Multi::read_info`: the first finished transfer's result.
    fn read_info(&self) -> CURLcode {
        let mut pending = 0;
        loop {
            let message = curl_multi_info_read(self.live(), &mut pending);
            // SAFETY: null or a message curl keeps until the next call.
            let Some(message) = (unsafe { message.as_ref() }) else {
                return CURLE_OK;
            };
            if message.msg == CURLMSG_DONE {
                return message.result();
            }
            log::warning(c"Multi::read_info", format_args!("CURLMSG default"));
        }
    }

    /// `Multi::selector_poll`: what the sockets have ready now, without
    /// waiting, for callers that don't `curl_multi_select`.
    fn selector_poll(&self) -> CURLMcode {
        let mut fds: Vec<libc::pollfd> = self
            .sockets
            .borrow()
            .iter()
            .map(|(&fd, socket)| libc::pollfd {
                fd,
                events: poll_events(events(socket.action.get())),
                revents: 0,
            })
            .collect();
        if fds.is_empty() {
            return CURLM_OK;
        }
        if swoole_rs_corebase::sys::poll(&mut fds, 0) < 0 {
            return if std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                CURLM_OK
            } else {
                CURLM_INTERNAL_ERROR
            };
        }
        for fd in fds.iter().filter(|fd| fd.revents != 0) {
            let Some(socket) = self.sockets.borrow().get(&fd.fd).cloned() else {
                continue;
            };
            socket
                .bitmask
                .set(socket.bitmask.get() | select_bits(fd.revents));
            insert(&self.active, &socket);
        }
        CURLM_OK
    }

    /// `Multi::perform` (`curl_multi_exec`), once C++ checked the coroutine.
    fn perform(&self) -> CURLMcode {
        let (result, timeout_ms) = self.timeout(-1);
        if result != CURLM_OK {
            return result;
        }
        // Adding a handle schedules an immediate timeout that starts the socket API.
        if timeout_ms == 0 {
            self.timer_callback.set(true);
        }
        if !self.timer_callback.get() && self.active.borrow().is_empty() {
            let result = self.selector_poll();
            if result != CURLM_OK {
                return result;
            }
        }
        let result = self.selector_finish();
        if result == CURLM_OK && self.running.get() > 0 {
            self.set_timer();
        } else {
            self.del_timer();
        }
        result
    }

    fn socket_action(&self, sockfd: curl_socket_t, bitmask: c_int) -> CURLMcode {
        // SAFETY: this multi's handle; curl writes the running count.
        unsafe { curl_multi_socket_action(self.raw(), sockfd, bitmask, self.running.as_ptr()) }
    }

    /// `Multi::selector_finish`: report the timeout and the sockets' events.
    fn selector_finish(&self) -> CURLMcode {
        self.del_timer();
        let mut result = CURLM_OK;
        if self.timer_callback.replace(false) {
            result = self.socket_action(CURL_SOCKET_TIMEOUT, 0);
        }
        while result == CURLM_OK && !self.active.borrow().is_empty() {
            // curl may remove sockets while it runs: work on a copy, which
            // keeps what it removes until the loop moves past it.
            let active = std::mem::take(&mut *self.active.borrow_mut());
            for socket in active {
                if socket.deleted.get() {
                    continue;
                }
                let bitmask = socket.bitmask.replace(0);
                let action = self.socket_action(socket.sockfd.get(), bitmask);
                if action != CURLM_OK {
                    result = action;
                    break;
                }
            }
        }
        self.active.borrow_mut().clear();
        result
    }

    /// `Multi::select` (`curl_multi_select`) in coroutine `co`: how many
    /// sockets have events.
    fn select(&self, co: *mut c_void, timeout: f64) -> c_long {
        if !self.active.borrow().is_empty() || self.timer_callback.get() {
            return self.ready();
        }
        if timeout == 0.0 {
            if self.selector_poll() != CURLM_OK {
                return -1;
            }
            return self.ready();
        }
        self.selector_prepare();
        self.set_timer();
        // No events and no timers: nothing would wake the coroutine.
        if !self.wait_event() {
            return 0;
        }
        self.co.set(co);
        host::yield_(co, timeout);
        self.co.set(ptr::null_mut());
        self.ready()
    }

    /// How many sockets have events.
    fn ready(&self) -> c_long {
        c_long::try_from(self.active.borrow().len()).unwrap_or(c_long::MAX)
    }

    /// `Multi::callback`: a socket event (or, for `None`, the timeout).
    fn callback(&self, socket: Option<&Socket>) {
        if self.released.get() {
            return;
        }
        if socket.is_none() {
            self.timer_callback.set(true);
        }
        if self.co.get().is_null() {
            match socket {
                Some(socket) => {
                    socket.socket.del();
                }
                None => self.del_timer(),
            }
            return;
        }
        // The map's copy: the reactor reports only the sockets in it.
        if let Some(socket) =
            socket.and_then(|socket| self.sockets.borrow().get(&socket.sockfd.get()).cloned())
        {
            insert(&self.active, &socket);
        }
        host::defer(self);
    }
}

/// `curl_socket_callback` and `curl_multi_timer_callback` as curl calls the ones
/// [`Multi::create`] sets: with the multi it set as their data, live while its handle is.
type OnSocket = extern "C" fn(*mut CURL, curl_socket_t, c_int, &Multi, *mut c_void) -> c_int;
type OnTimer = extern "C" fn(*mut CURLM, c_long, &Multi) -> c_int;

/// `Multi::handle_socket`, curl's socket callback.
extern "C" fn handle_socket(
    _easy: *mut CURL,
    sockfd: curl_socket_t,
    action: c_int,
    multi: &Multi,
    _socketp: *mut c_void,
) -> c_int {
    let done = match action {
        CURL_POLL_IN | CURL_POLL_OUT | CURL_POLL_INOUT => multi.set_event(sockfd, action),
        CURL_POLL_REMOVE => multi.del_event(sockfd),
        _ => swoole_rs_corebase::fatal("curl asked for an unknown poll action"),
    };
    if done { OK } else { ERR }
}

/// `Multi::handle_timeout`, curl's timer callback.
extern "C" fn handle_timeout(_multi: *mut CURLM, timeout_ms: c_long, multi: &Multi) -> c_int {
    multi.on_timeout(timeout_ms)
}

impl Multi {
    /// `delete multi`'s work, before its last reference goes: the timer, then the
    /// handle, whose callbacks still find the multi.
    fn close(&self) {
        self.del_timer();
        // SAFETY: its handle, which nothing uses again: the caller drops the last
        // reference to the multi next.
        unsafe { curl_multi_cleanup(self.raw()) };
        // As upstream's `delete multi`, the sockets curl didn't remove stay:
        // a reactor socket may still carry one's address.
        #[expect(
            clippy::mem_forget,
            reason = "a reactor socket may still carry one's address"
        )]
        std::mem::forget(self.sockets.take());
    }
}

/// `Multi::release`: C++ lets go of `multi`, which closes, or, while a deferred
/// wake-up is queued, is left to the wake-up, which holds it.
fn release(multi: Rc<Multi>) {
    if multi.defer_callback.get() {
        multi.released.set(true);
    } else {
        multi.close();
    }
    // The last reference, unless a wake-up holds one.
    drop(multi);
}

/// `Multi::release` of the multi `swoole_curl_multi_new` gave C++.
/// # Safety
/// `multi` is null or came from `swoole_curl_multi_new`; nothing uses it afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_curl_multi_free(multi: *mut Multi) {
    if multi.is_null() {
        return;
    }
    // SAFETY: per the caller, the reference `swoole_curl_multi_new` gave up.
    release(unsafe { Rc::from_raw(multi) });
}

/// `Multi::get_multi_handle`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_curl_multi_handle(multi: &Multi) -> *mut CURLM {
    multi.raw()
}

/// `Multi::get_running_handles`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_curl_multi_running(multi: &Multi) -> c_int {
    multi.running.get()
}

/// `Multi::add_handle`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_curl_multi_add_handle(multi: &Multi, handle: &mut Handle) -> CURLMcode {
    multi.add_handle(handle)
}

/// `Multi::remove_handle`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_curl_multi_remove_handle(multi: &Multi, handle: &mut Handle) -> CURLMcode {
    multi.remove_handle(handle)
}

/// `Multi::perform`, after `swoole_curl_check_bound_co`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_curl_multi_perform(multi: &Multi) -> CURLMcode {
    multi.perform()
}

/// `Multi::select` in the coroutine `swoole_curl_check_bound_co` returned: the
/// running one, which the yield checks (it aborts otherwise) before anything uses `co`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_curl_multi_select(multi: &Multi, co: *mut c_void, timeout: f64) -> c_long {
    multi.select(co, timeout)
}

/// `Multi::exec`; [`EXEC_NEEDS_COROUTINE`] when it must wait outside a
/// coroutine or while another one waits.
pub fn swr_v1_curl_multi_exec(multi: &Multi, handle: &mut Handle) -> CURLcode {
    multi.exec(handle)
}

/// The multi's deferred wake-up: resume its waiting coroutine. True when
/// [`swr_v1_curl_multi_free`] released the multi meanwhile: close it now.
fn multi_deferred(this: &Multi) -> bool {
    this.defer_callback.set(false);
    if this.released.get() {
        return true;
    }
    let co = this.co.get();
    if !co.is_null() {
        // SAFETY: the coroutine waiting on this multi.
        unsafe { host::resume(co) };
    }
    false
}

/// A reactor event (`CURL_CSELECT_*` in `bitmask`) on a cURL socket.
fn socket_event(socket: &Socket, bitmask: c_int) {
    socket.bitmask.set(socket.bitmask.get() | bitmask);
    if let Some(multi) = socket.multi.upgrade() {
        multi.callback(Some(socket));
    }
}

/// `swoole::curl::get_handle`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_curl_get_handle(cp: Easy) -> *mut Handle {
    cp.handle()
}

/// `swoole::curl::create_handle`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_curl_create_handle(cp: Easy) -> *mut Handle {
    let handle = Box::into_raw(Box::new(Handle {
        cp: cp.0,
        multi: ptr::null_mut(),
        easy_multi: ptr::null_mut(),
    }));
    cp.set_private(handle);
    handle
}

/// `swoole::curl::destroy_handle`.
/// # Safety
/// No transfer of `cp` runs, and nothing uses its Swoole handle afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_curl_destroy_handle(cp: Easy) {
    let handle = cp.handle();
    if handle.is_null() {
        return;
    }
    // SAFETY: the handle create_handle made, which only this frees.
    let handle = unsafe { Box::from_raw(handle) };
    // SAFETY: its own exec multi, or null.
    unsafe { swr_v1_curl_multi_free(handle.easy_multi) };
    cp.set_private(ptr::null_mut());
}

/// `swoole_curl_easy_reset`: `curl_easy_reset`, keeping the handle.
/// # Safety
/// As `curl_easy_reset`: no transfer of `cp` runs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_curl_easy_reset(cp: Easy) {
    let handle = cp.handle();
    // SAFETY: per the caller.
    unsafe { curl_easy_reset(cp.0) };
    // The easy handle keeps the Swoole handle it held.
    cp.set_private(handle);
}

#[cfg(not(test))]
mod host;
#[cfg(not(test))]
pub use host::{swoole_curl_check_bound_co, swoole_curl_easy_perform, swoole_curl_multi_new};
#[cfg(all(feature = "php", not(test)))]
mod php;
#[cfg(all(feature = "php", not(test)))]
pub use php::swoole_curl_get_handle;
#[cfg(test)]
mod tests;
#[cfg(test)]
use tests::host;
