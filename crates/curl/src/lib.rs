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
};
use swoole_rs_corebase::log::{self, set_last_error};
use swoole_rs_curl_sys::{
    CURL, CURL_CSELECT_ERR, CURL_CSELECT_IN, CURL_CSELECT_OUT, CURL_POLL_IN, CURL_POLL_INOUT,
    CURL_POLL_OUT, CURL_POLL_REMOVE, CURL_SOCKET_TIMEOUT, CURLE_ABORTED_BY_CALLBACK,
    CURLE_FAILED_INIT, CURLE_OK, CURLINFO_PRIVATE, CURLM, CURLM_INTERNAL_ERROR, CURLM_OK,
    CURLMOPT_SOCKETDATA, CURLMOPT_SOCKETFUNCTION, CURLMOPT_TIMERDATA, CURLMOPT_TIMERFUNCTION,
    CURLMSG_DONE, CURLMcode, CURLOPT_PRIVATE, CURLcode, curl_easy_getinfo, curl_easy_reset,
    curl_easy_setopt, curl_multi_add_handle, curl_multi_assign, curl_multi_cleanup,
    curl_multi_info_read, curl_multi_init, curl_multi_remove_handle, curl_multi_setopt,
    curl_multi_socket_action, curl_multi_timeout, curl_multi_timer_callback, curl_socket_callback,
    curl_socket_t,
};

/// `SW_EVENT_READ` and `SW_EVENT_WRITE`.
const READ: c_int = 1 << 9;
const WRITE: c_int = 1 << 10;
/// `SW_OK` and `SW_ERR`.
const OK: c_int = 0;
const ERR: c_int = -1;
/// `SW_ERROR_CO_CANCELED`.
const CO_CANCELED: c_int = 10017;
/// What [`swr_v1_curl_multi_exec`] returns when it must wait without a
/// coroutine to wait in (or one already bound): C++ raises the fatal error.
pub const EXEC_NEEDS_COROUTINE: CURLcode = -1;

/// `struct swr_v1_curl_handle`: the easy handle's multi while it is in one,
/// and the multi `curl_exec` runs it in. C++ reads and writes `multi`.
#[repr(C)]
pub struct Handle {
    cp: *mut CURL,
    multi: *mut Multi,
    easy_multi: *mut Multi,
}

/// `swoole::curl::Socket`: one socket curl asked to watch.
struct Socket {
    multi: *const Multi,
    /// The reactor's `network::Socket`, freed when curl removes it.
    socket: host::ReactorSocket,
    bitmask: Cell<c_int>,
    sockfd: Cell<curl_socket_t>,
    action: Cell<c_int>,
    deleted: Cell<bool>,
}

/// `struct swr_v1_curl_multi`. Every field is a cell: curl calls back into the
/// same multi from inside its own calls, and other coroutines do while this
/// one waits, so no borrow is held across either.
pub struct Multi {
    handle: *mut CURLM,
    timer: Cell<*mut c_void>,
    /// The coroutine waiting on this multi.
    co: Cell<*mut c_void>,
    running: Cell<c_int>,
    defer_callback: Cell<bool>,
    /// Freed while a deferred wake-up was queued: the wake-up frees it.
    released: Cell<bool>,
    /// The selector: a timeout to report to curl, sockets with events (a
    /// set, in arrival order), and sockets curl removed while it ran.
    timer_callback: Cell<bool>,
    executing: Cell<bool>,
    active: RefCell<Vec<*mut Socket>>,
    release: RefCell<Vec<*mut Socket>>,
    sockets: RefCell<BTreeMap<curl_socket_t, *mut Socket>>,
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

fn insert(set: &RefCell<Vec<*mut Socket>>, socket: *mut Socket) {
    let mut set = set.borrow_mut();
    if !set.contains(&socket) {
        set.push(socket);
    }
}

impl Multi {
    /// `Multi::Multi`: a multi handle whose socket and timer callbacks reach
    /// this multi, which therefore lives in a box until
    /// [`swr_v1_curl_multi_free`].
    fn create() -> *mut Self {
        // SAFETY: no preconditions.
        let handle = unsafe { curl_multi_init() };
        let multi = Box::into_raw(Box::new(Self {
            handle,
            timer: Cell::new(ptr::null_mut()),
            co: Cell::new(ptr::null_mut()),
            running: Cell::new(0),
            defer_callback: Cell::new(false),
            released: Cell::new(false),
            timer_callback: Cell::new(false),
            executing: Cell::new(false),
            active: RefCell::new(Vec::new()),
            release: RefCell::new(Vec::new()),
            sockets: RefCell::new(BTreeMap::new()),
        }));
        let on_socket: curl_socket_callback = handle_socket;
        let on_timer: curl_multi_timer_callback = handle_timeout;
        // SAFETY: the handle just made; the callbacks match curl's types.
        unsafe { curl_multi_setopt(handle, CURLMOPT_SOCKETFUNCTION, on_socket) };
        // SAFETY: as above.
        unsafe { curl_multi_setopt(handle, CURLMOPT_TIMERFUNCTION, on_timer) };
        for data in [CURLMOPT_SOCKETDATA, CURLMOPT_TIMERDATA] {
            // SAFETY: as above; the box outlives the handle.
            unsafe { curl_multi_setopt(handle, data, multi.cast::<c_void>()) };
        }
        multi
    }

    fn wait_event(&self) -> bool {
        !self.timer.get().is_null() || !self.sockets.borrow().is_empty()
    }

    fn add_timer(&self, timeout_ms: c_long) {
        self.del_timer();
        self.timer.set(host::timer_add(timeout_ms, self));
    }

    fn del_timer(&self) {
        let timer = self.timer.get();
        if !timer.is_null() && host::timer_available() {
            // SAFETY: a timer this multi added and still holds.
            unsafe { host::timer_del(timer) };
            self.timer.set(ptr::null_mut());
        }
    }

    /// `curl_multi_timeout`: its result and the timeout, `unset` if curl wrote none.
    fn timeout(&self, unset: c_long) -> (CURLMcode, c_long) {
        let mut timeout_ms = unset;
        // SAFETY: this multi's handle, live until `swr_v1_curl_multi_free`.
        let result = unsafe { curl_multi_timeout(self.handle, &raw mut timeout_ms) };
        (result, timeout_ms)
    }

    /// `curl_multi_assign`: `socket` is what curl's socket callback gets for `sockfd`.
    fn assign(&self, sockfd: curl_socket_t, socket: *mut Socket) -> CURLMcode {
        // SAFETY: this multi's handle, live until `swr_v1_curl_multi_free`; curl
        // stores the pointer and never reads through it.
        unsafe { curl_multi_assign(self.handle, sockfd, socket.cast()) }
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
            if self.timer.get().is_null() {
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

    /// `Multi::set_event`.
    fn set_event(&self, socket: *mut Socket, sockfd: curl_socket_t, action: c_int) -> c_int {
        if !host::event_available() {
            return -1;
        }
        host::install_handlers();
        let socket = if socket.is_null() {
            let reactor = host::ReactorSocket::new(sockfd);
            let socket = Box::into_raw(Box::new(Socket {
                multi: self,
                socket: reactor,
                bitmask: Cell::new(0),
                sockfd: Cell::new(0),
                action: Cell::new(0),
                deleted: Cell::new(false),
            }));
            if self.assign(sockfd, socket) != CURLM_OK {
                // SAFETY: made above, and never registered.
                unsafe { reactor.free() };
                // SAFETY: boxed above; curl does not hold it.
                drop(unsafe { Box::from_raw(socket) });
                return -1;
            }
            // The reactor socket carries the box it reports.
            reactor.set_object(socket.cast());
            self.sockets.borrow_mut().insert(sockfd, socket);
            socket
        } else {
            socket
        };
        // SAFETY: a socket this multi made and has not freed.
        let curl_socket = unsafe { &*socket };
        curl_socket.sockfd.set(sockfd);
        curl_socket.action.set(action);
        let events = events(action);
        if curl_socket.socket.events() == 0 {
            curl_socket.socket.add(events)
        } else {
            curl_socket.socket.set(events)
        }
    }

    /// `Multi::del_event`.
    fn del_event(&self, socket: *mut Socket, sockfd: curl_socket_t) -> c_int {
        self.sockets.borrow_mut().remove(&sockfd);
        self.assign(sockfd, ptr::null_mut());
        if socket.is_null() {
            return ERR;
        }
        self.active.borrow_mut().retain(|&active| active != socket);
        // SAFETY: a socket this multi made and has not freed.
        let curl_socket = unsafe { &*socket };
        // SAFETY: that socket's reactor socket, which nothing uses afterwards.
        unsafe { curl_socket.socket.free() };
        if self.executing.get() {
            curl_socket.deleted.set(true);
            insert(&self.release, socket);
        } else {
            // SAFETY: the box `set_event` made; curl let go of it above and the
            // selector holds no copy.
            drop(unsafe { Box::from_raw(socket) });
        }
        OK
    }

    fn add_handle(&self, handle: &mut Handle) -> CURLMcode {
        // SAFETY: this multi's handle and the handle's live easy handle.
        let result = unsafe { curl_multi_add_handle(self.handle, handle.cp) };
        if result == CURLM_OK {
            handle.multi = ptr::from_ref(self).cast_mut();
        }
        result
    }

    fn remove_handle(&self, handle: &mut Handle) -> CURLMcode {
        // SAFETY: this multi's handle and the handle's live easy handle.
        let result = unsafe { curl_multi_remove_handle(self.handle, handle.cp) };
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
        let sockets: Vec<*mut Socket> = self.sockets.borrow().values().copied().collect();
        for socket in sockets {
            // SAFETY: a socket in this multi's map.
            let curl_socket = unsafe { &*socket };
            if curl_socket.socket.removed() {
                curl_socket.socket.add(events(curl_socket.action.get()));
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
                // SAFETY: the running coroutine.
                canceled = unsafe { host::yield_(co, -1.0) };
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
            // SAFETY: this multi's handle.
            let message = unsafe { curl_multi_info_read(self.handle, &raw mut pending) };
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
            .map(|(&fd, &socket)| libc::pollfd {
                fd,
                // SAFETY: a socket in this multi's map.
                events: poll_events(events(unsafe { (*socket).action.get() })),
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
            let Some(&socket) = self.sockets.borrow().get(&fd.fd) else {
                continue;
            };
            // SAFETY: a socket in this multi's map.
            let curl_socket = unsafe { &*socket };
            curl_socket
                .bitmask
                .set(curl_socket.bitmask.get() | select_bits(fd.revents));
            insert(&self.active, socket);
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
        unsafe { curl_multi_socket_action(self.handle, sockfd, bitmask, self.running.as_ptr()) }
    }

    /// `Multi::selector_finish`: report the timeout and the sockets' events.
    fn selector_finish(&self) -> CURLMcode {
        self.del_timer();
        self.executing.set(true);
        let mut result = CURLM_OK;
        if self.timer_callback.replace(false) {
            result = self.socket_action(CURL_SOCKET_TIMEOUT, 0);
        }
        while result == CURLM_OK && !self.active.borrow().is_empty() {
            // curl may remove sockets while it runs: work on a copy.
            let active = std::mem::take(&mut *self.active.borrow_mut());
            for socket in active {
                // SAFETY: freeing waits for `release` while executing.
                let curl_socket = unsafe { &*socket };
                if curl_socket.deleted.get() {
                    continue;
                }
                let bitmask = curl_socket.bitmask.replace(0);
                let action = self.socket_action(curl_socket.sockfd.get(), bitmask);
                if action != CURLM_OK {
                    result = action;
                    break;
                }
            }
        }
        self.executing.set(false);
        self.active.borrow_mut().clear();
        for socket in std::mem::take(&mut *self.release.borrow_mut()) {
            // SAFETY: the box `set_event` made, deleted while executing; nothing
            // references it now.
            drop(unsafe { Box::from_raw(socket) });
        }
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
        // SAFETY: the running coroutine.
        unsafe { host::yield_(co, timeout) };
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
        if let Some(socket) = socket {
            insert(&self.active, ptr::from_ref(socket).cast_mut());
        }
        if self.defer_callback.replace(true) {
            return;
        }
        // SAFETY: the loop runs this callback; the multi waits on the deferral.
        unsafe { host::defer(self) };
    }
}

/// `Multi::handle_socket`, curl's socket callback.
unsafe extern "C" fn handle_socket(
    _easy: *mut CURL,
    sockfd: curl_socket_t,
    action: c_int,
    userp: *mut c_void,
    socketp: *mut c_void,
) -> c_int {
    // SAFETY: the multi this callback was set on.
    let multi = unsafe { &*userp.cast::<Multi>() };
    match action {
        CURL_POLL_IN | CURL_POLL_OUT | CURL_POLL_INOUT => {
            multi.set_event(socketp.cast(), sockfd, action)
        }
        CURL_POLL_REMOVE => multi.del_event(socketp.cast(), sockfd),
        _ => std::process::abort(),
    }
}

/// `Multi::handle_timeout`, curl's timer callback.
unsafe extern "C" fn handle_timeout(
    _multi: *mut CURLM,
    timeout_ms: c_long,
    userp: *mut c_void,
) -> c_int {
    // SAFETY: the multi this callback was set on.
    unsafe { &*userp.cast::<Multi>() }.on_timeout(timeout_ms)
}

/// `Multi::release`: `delete multi`, or, while a deferred wake-up is
/// queued, leave that to the wake-up.
/// # Safety
/// `multi` came from `swoole_curl_multi_new`; nothing uses it afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_curl_multi_free(multi: *mut Multi) {
    if multi.is_null() {
        return;
    }
    {
        // SAFETY: per the caller.
        let this = unsafe { &*multi };
        if this.defer_callback.get() {
            this.released.set(true);
            return;
        }
        this.del_timer();
        // SAFETY: its handle; the callbacks it runs still find the multi.
        unsafe { curl_multi_cleanup(this.handle) };
    }
    // SAFETY: per the caller.
    drop(unsafe { Box::from_raw(multi) });
}

/// `Multi::get_multi_handle`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_curl_multi_handle(multi: &Multi) -> *mut CURLM {
    multi.handle
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

/// `Multi::select` in the coroutine `swoole_curl_check_bound_co` returned.
/// # Safety
/// `co` is the running coroutine.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_curl_multi_select(
    multi: &Multi,
    co: *mut c_void,
    timeout: f64,
) -> c_long {
    multi.select(co, timeout)
}

/// `Multi::exec`; [`EXEC_NEEDS_COROUTINE`] when it must wait outside a
/// coroutine or while another one waits.
pub fn swr_v1_curl_multi_exec(multi: &Multi, handle: &mut Handle) -> CURLcode {
    multi.exec(handle)
}

/// The multi's deferred wake-up: resume its waiting coroutine. True when
/// [`swr_v1_curl_multi_free`] released the multi meanwhile: free it now.
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
/// # Safety
/// `object` is the reactor socket's `object`, which the multi set.
unsafe fn socket_event(object: *mut c_void, bitmask: c_int) {
    // SAFETY: per the caller.
    let socket = unsafe { &*object.cast::<Socket>() };
    socket.bitmask.set(socket.bitmask.get() | bitmask);
    // SAFETY: the multi that made the socket outlives it.
    unsafe { &*socket.multi }.callback(Some(socket));
}

/// `swoole::curl::get_handle`.
/// # Safety
/// `cp` is a live easy handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_curl_get_handle(cp: *mut CURL) -> *mut Handle {
    let mut handle: *mut Handle = ptr::null_mut();
    // SAFETY: per the caller; CURLINFO_PRIVATE writes a pointer.
    if unsafe { curl_easy_getinfo(cp, CURLINFO_PRIVATE, &raw mut handle) } == CURLE_OK {
        handle
    } else {
        ptr::null_mut()
    }
}

/// `swoole::curl::create_handle`.
/// # Safety
/// As [`swr_v1_curl_get_handle`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_curl_create_handle(cp: *mut CURL) -> *mut Handle {
    let handle = Box::into_raw(Box::new(Handle {
        cp,
        multi: ptr::null_mut(),
        easy_multi: ptr::null_mut(),
    }));
    // SAFETY: per the caller.
    unsafe { curl_easy_setopt(cp, CURLOPT_PRIVATE, handle.cast::<c_void>()) };
    handle
}

/// `swoole::curl::destroy_handle`.
/// # Safety
/// As [`swr_v1_curl_get_handle`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_curl_destroy_handle(cp: *mut CURL) {
    // SAFETY: per the caller.
    let handle = unsafe { swr_v1_curl_get_handle(cp) };
    if handle.is_null() {
        return;
    }
    // SAFETY: the handle create_handle made, which only this frees.
    let handle = unsafe { Box::from_raw(handle) };
    // SAFETY: its own exec multi, or null.
    unsafe { swr_v1_curl_multi_free(handle.easy_multi) };
    // SAFETY: per the caller.
    unsafe { curl_easy_setopt(cp, CURLOPT_PRIVATE, ptr::null_mut::<c_void>()) };
}

/// `swoole_curl_easy_reset`: `curl_easy_reset`, keeping the handle.
/// # Safety
/// As [`swr_v1_curl_get_handle`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_curl_easy_reset(cp: *mut CURL) {
    // SAFETY: per the caller.
    let handle = unsafe { swr_v1_curl_get_handle(cp) };
    // SAFETY: as above.
    unsafe { curl_easy_reset(cp) };
    // SAFETY: as above.
    unsafe { curl_easy_setopt(cp, CURLOPT_PRIVATE, handle.cast::<c_void>()) };
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
