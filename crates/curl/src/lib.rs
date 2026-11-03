//! Swoole's coroutine cURL (upstream's `swoole_curl.cc`, built with
//! `--enable-swoole-curl`): the multi engine behind the native `curl_exec` and
//! `curl_multi_*` functions of its bundled `ext/curl`. A [`Multi`] drives a
//! libcurl multi handle through its socket and timer callbacks: each socket
//! curl watches joins the reactor, each timeout becomes a timer, and the
//! coroutine waiting on the transfer is resumed when either fires.
//!
//! C++ keeps the `swoole::curl::Multi` and `Handle` names `ext/curl` uses, as
//! a facade over these exports, and lends the reactor, timer and coroutine
//! operations ([`Ops`]). The Zend side (`swoole_curl_get_handle`) and the
//! fatal errors of a bound or missing coroutine stay in C++.
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
/// What [`swr_v1_curl_multi_exec`] returns when it must wait without a
/// coroutine to wait in (or one already bound): C++ raises the fatal error.
pub const EXEC_NEEDS_COROUTINE: CURLcode = -1;

/// `swr_v1_curl_ops`: the reactor, timer and coroutine operations C++ lends.
#[repr(C)]
pub struct Ops {
    /// `swoole_event_is_available()`.
    event_available: unsafe extern "C" fn() -> bool,
    /// Install the cURL fd type's reactor handlers unless they are.
    install_handlers: unsafe extern "C" fn(),
    /// `new network::Socket` for `fd` of the cURL fd type, out of the
    /// reactor (`removed`), carrying `object`.
    socket_new: unsafe extern "C" fn(fd: c_int, object: *mut c_void) -> *mut c_void,
    /// `socket->events`, and `socket->removed` into `removed`.
    socket_events: unsafe extern "C" fn(socket: *mut c_void, removed: *mut bool) -> c_int,
    /// `swoole_event_add`.
    event_add: unsafe extern "C" fn(socket: *mut c_void, events: c_int) -> c_int,
    /// `swoole_event_set`.
    event_set: unsafe extern "C" fn(socket: *mut c_void, events: c_int) -> c_int,
    /// `swoole_event_del`.
    event_del: unsafe extern "C" fn(socket: *mut c_void) -> c_int,
    /// Leave the reactor silently if the socket is in it and an event loop
    /// runs, then free it without closing the fd (curl owns it).
    socket_free: unsafe extern "C" fn(socket: *mut c_void),
    /// `swoole_timer_is_available()`.
    timer_available: unsafe extern "C" fn() -> bool,
    /// `swoole_timer_add(ms)`, whose callback is
    /// `swr_v1_curl_multi_on_timer(multi)`.
    timer_add: unsafe extern "C" fn(ms: c_long, multi: *mut c_void) -> *mut c_void,
    /// `swoole_timer_del`.
    timer_del: unsafe extern "C" fn(timer: *mut c_void),
    /// `Coroutine::get_current()`.
    current: unsafe extern "C" fn() -> *mut c_void,
    /// `co->yield_ex(timeout)`, then whether it was canceled.
    yield_: unsafe extern "C" fn(co: *mut c_void, timeout: f64) -> bool,
    /// `co->resume()`.
    resume: unsafe extern "C" fn(co: *mut c_void),
    /// `swoole_event_defer`, whose callback is
    /// `swr_v1_curl_multi_deferred(multi)`.
    defer: unsafe extern "C" fn(multi: *mut c_void),
    /// `swoole_set_last_error(SW_ERROR_CO_CANCELED)`.
    set_canceled: unsafe extern "C" fn(),
    /// `swoole_warning("CURLMSG default")`, as `Multi::read_info`.
    warn_message: unsafe extern "C" fn(),
}

/// `swoole::curl::Handle`: the easy handle's multi while it is in one, and
/// the multi `curl_exec` runs it in. C++ reads and writes `multi`.
#[repr(C)]
pub struct Handle {
    cp: *mut CURL,
    multi: *mut Multi,
    easy_multi: *mut Multi,
}

/// `swoole::curl::Socket`: one socket curl asked to watch.
struct Socket {
    multi: *const Multi,
    /// The reactor's `network::Socket`.
    socket: *mut c_void,
    bitmask: Cell<c_int>,
    sockfd: Cell<curl_socket_t>,
    action: Cell<c_int>,
    deleted: Cell<bool>,
}

/// `swoole::curl::Multi`. Every field is a cell: curl calls back into the
/// same multi from inside its own calls, and other coroutines do while this
/// one waits, so no borrow is held across either.
pub struct Multi {
    handle: *mut CURLM,
    ops: &'static Ops,
    timer: Cell<*mut c_void>,
    /// The coroutine waiting on this multi.
    co: Cell<*mut c_void>,
    running: Cell<c_int>,
    defer_callback: Cell<bool>,
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

/// Free a socket [`Multi::set_event`] made.
/// # Safety
/// `socket` came from `Box::into_raw` and nothing uses it afterwards.
unsafe fn free_socket(socket: *mut Socket) {
    // SAFETY: per the caller.
    drop(unsafe { Box::from_raw(socket) });
}

impl Multi {
    /// `Multi::Multi`: a multi handle whose socket and timer callbacks reach
    /// this multi, which therefore lives in a box until
    /// [`swr_v1_curl_multi_free`].
    fn create(ops: &'static Ops) -> *mut Self {
        let multi = Box::into_raw(Box::new(Self {
            // SAFETY: no preconditions.
            handle: unsafe { curl_multi_init() },
            ops,
            timer: Cell::new(ptr::null_mut()),
            co: Cell::new(ptr::null_mut()),
            running: Cell::new(0),
            defer_callback: Cell::new(false),
            timer_callback: Cell::new(false),
            executing: Cell::new(false),
            active: RefCell::new(Vec::new()),
            release: RefCell::new(Vec::new()),
            sockets: RefCell::new(BTreeMap::new()),
        }));
        // SAFETY: made above.
        let handle = unsafe { (*multi).handle };
        let on_socket: curl_socket_callback = handle_socket;
        let on_timer: curl_multi_timer_callback = handle_timeout;
        // SAFETY: the handle just made; the callbacks match curl's types.
        unsafe { curl_multi_setopt(handle, CURLMOPT_SOCKETFUNCTION, on_socket) };
        // SAFETY: as above.
        unsafe { curl_multi_setopt(handle, CURLMOPT_TIMERFUNCTION, on_timer) };
        // SAFETY: as above; the box outlives the handle.
        unsafe { curl_multi_setopt(handle, CURLMOPT_SOCKETDATA, multi.cast::<c_void>()) };
        // SAFETY: as above.
        unsafe { curl_multi_setopt(handle, CURLMOPT_TIMERDATA, multi.cast::<c_void>()) };
        multi
    }

    fn this(&self) -> *mut c_void {
        ptr::from_ref(self).cast_mut().cast()
    }

    fn wait_event(&self) -> bool {
        !self.timer.get().is_null() || !self.sockets.borrow().is_empty()
    }

    fn add_timer(&self, timeout_ms: c_long) {
        let timer = self.timer.get();
        // SAFETY: C++'s operations.
        if !timer.is_null() && unsafe { (self.ops.timer_available)() } {
            // SAFETY: a timer this multi added and still holds.
            unsafe { (self.ops.timer_del)(timer) };
        }
        // SAFETY: C++'s operations; the timer ends before this multi does.
        self.timer
            .set(unsafe { (self.ops.timer_add)(timeout_ms, self.this()) });
    }

    fn del_timer(&self) {
        let timer = self.timer.get();
        // SAFETY: C++'s operations.
        if !timer.is_null() && unsafe { (self.ops.timer_available)() } {
            // SAFETY: a timer this multi added and still holds.
            unsafe { (self.ops.timer_del)(timer) };
            self.timer.set(ptr::null_mut());
        }
    }

    fn set_timer(&self) {
        let mut timeout_ms: c_long = 0;
        // SAFETY: this multi's handle.
        unsafe { curl_multi_timeout(self.handle, &raw mut timeout_ms) };
        self.on_timeout(timeout_ms);
    }

    /// `Multi::handle_timeout`.
    fn on_timeout(&self, timeout_ms: c_long) -> c_int {
        // SAFETY: C++'s operations.
        if !unsafe { (self.ops.event_available)() } {
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
        // SAFETY: C++'s operations.
        if !unsafe { (self.ops.event_available)() } {
            return -1;
        }
        // SAFETY: as above.
        unsafe { (self.ops.install_handlers)() };
        let socket = if socket.is_null() {
            let socket = Box::into_raw(Box::new(Socket {
                multi: self,
                socket: ptr::null_mut(),
                bitmask: Cell::new(0),
                sockfd: Cell::new(0),
                action: Cell::new(0),
                deleted: Cell::new(false),
            }));
            // SAFETY: this multi's handle, and a socket curl reports.
            if unsafe { curl_multi_assign(self.handle, sockfd, socket.cast()) } != CURLM_OK {
                // SAFETY: made above; curl does not hold it.
                unsafe { free_socket(socket) };
                return -1;
            }
            // SAFETY: C++'s operations; the socket carries the box it reports.
            let reactor = unsafe { (self.ops.socket_new)(sockfd, socket.cast()) };
            // SAFETY: made above, and nothing else references it yet.
            unsafe { (*socket).socket = reactor };
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
        // SAFETY: C++'s operations, over this socket's reactor socket.
        if unsafe { (self.ops.socket_events)(curl_socket.socket, ptr::null_mut()) } == 0 {
            // SAFETY: as above.
            unsafe { (self.ops.event_add)(curl_socket.socket, events) }
        } else {
            // SAFETY: as above.
            unsafe { (self.ops.event_set)(curl_socket.socket, events) }
        }
    }

    /// `Multi::del_event`.
    fn del_event(&self, socket: *mut Socket, sockfd: curl_socket_t) -> c_int {
        self.sockets.borrow_mut().remove(&sockfd);
        // SAFETY: this multi's handle.
        unsafe { curl_multi_assign(self.handle, sockfd, ptr::null_mut()) };
        if socket.is_null() {
            return ERR;
        }
        self.active.borrow_mut().retain(|&active| active != socket);
        // SAFETY: a socket this multi made and has not freed.
        let reactor = unsafe { (*socket).socket };
        // SAFETY: C++'s operations, over that socket's reactor socket.
        unsafe { (self.ops.socket_free)(reactor) };
        if self.executing.get() {
            // SAFETY: as above.
            unsafe { (*socket).deleted.set(true) };
            insert(&self.release, socket);
        } else {
            // SAFETY: curl let go of it above and the selector holds no copy.
            unsafe { free_socket(socket) };
        }
        OK
    }

    fn add_handle(&self, handle: *mut Handle) -> CURLMcode {
        // SAFETY: the caller's live handle.
        let cp = unsafe { (*handle).cp };
        // SAFETY: this multi's handle and a live easy handle.
        let result = unsafe { curl_multi_add_handle(self.handle, cp) };
        if result == CURLM_OK {
            // SAFETY: as above.
            unsafe { (*handle).multi = ptr::from_ref(self).cast_mut() };
        }
        result
    }

    fn remove_handle(&self, handle: *mut Handle) -> CURLMcode {
        // SAFETY: the caller's live handle.
        let cp = unsafe { (*handle).cp };
        // SAFETY: this multi's handle and a live easy handle.
        let result = unsafe { curl_multi_remove_handle(self.handle, cp) };
        // SAFETY: as above.
        unsafe { (*handle).multi = ptr::null_mut() };
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
            let mut removed = false;
            // SAFETY: C++'s operations, over its reactor socket.
            unsafe { (self.ops.socket_events)(curl_socket.socket, &raw mut removed) };
            if removed {
                // SAFETY: as above.
                unsafe {
                    (self.ops.event_add)(curl_socket.socket, events(curl_socket.action.get()))
                };
            }
        }
    }

    /// `Multi::exec`: run one easy handle to completion (`curl_exec`).
    fn exec(&self, handle: *mut Handle) -> CURLcode {
        if self.add_handle(handle) != CURLM_OK {
            return CURLE_FAILED_INIT;
        }
        let mut canceled = false;
        loop {
            self.selector_prepare();
            if self.wait_event() {
                // SAFETY: C++'s operations.
                let co = unsafe { (self.ops.current)() };
                if !self.co.get().is_null() || co.is_null() {
                    return EXEC_NEEDS_COROUTINE;
                }
                self.co.set(co);
                // SAFETY: the running coroutine.
                canceled = unsafe { (self.ops.yield_)(co, -1.0) };
                self.co.set(ptr::null_mut());
                if canceled {
                    // SAFETY: C++'s operations.
                    unsafe { (self.ops.set_canceled)() };
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
            // SAFETY: C++'s operations.
            unsafe { (self.ops.warn_message)() };
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
        let Ok(count) = libc::nfds_t::try_from(fds.len()) else {
            return CURLM_INTERNAL_ERROR;
        };
        // SAFETY: `count` live pollfds.
        if unsafe { libc::poll(fds.as_mut_ptr(), count, 0) } < 0 {
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
        let mut timeout_ms: c_long = -1;
        // SAFETY: this multi's handle.
        let result = unsafe { curl_multi_timeout(self.handle, &raw mut timeout_ms) };
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
            // SAFETY: deleted while executing; nothing references it now.
            unsafe { free_socket(socket) };
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
        unsafe { (self.ops.yield_)(co, timeout) };
        self.co.set(ptr::null_mut());
        self.ready()
    }

    /// How many sockets have events.
    fn ready(&self) -> c_long {
        c_long::try_from(self.active.borrow().len()).unwrap_or(c_long::MAX)
    }

    /// `Multi::callback`: a socket event (or, for `None`, the timeout).
    fn callback(&self, socket: Option<&Socket>) {
        if socket.is_none() {
            self.timer_callback.set(true);
        }
        if self.co.get().is_null() {
            match socket {
                // SAFETY: C++'s operations, over the socket's reactor socket.
                Some(socket) => unsafe {
                    (self.ops.event_del)(socket.socket);
                },
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
        // SAFETY: C++'s operations; the deferred call reaches this multi.
        unsafe { (self.ops.defer)(self.this()) };
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

/// # Safety
/// `multi` came from [`swr_v1_curl_multi_new`] and is not freed.
unsafe fn multi<'a>(multi: *mut Multi) -> &'a Multi {
    // SAFETY: per the caller.
    unsafe { &*multi }
}

/// `new Multi()` over C++'s operations.
/// # Safety
/// `ops` is a static table.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_curl_multi_new(ops: *const Ops) -> *mut Multi {
    // SAFETY: per the caller.
    let Some(ops) = (unsafe { ops.as_ref() }) else {
        return ptr::null_mut();
    };
    Multi::create(ops)
}

/// `delete multi`.
/// # Safety
/// `multi` came from [`swr_v1_curl_multi_new`]; nothing uses it afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_curl_multi_free(multi: *mut Multi) {
    if multi.is_null() {
        return;
    }
    {
        // SAFETY: per the caller.
        let this = unsafe { self::multi(multi) };
        this.del_timer();
        // SAFETY: its handle; the callbacks it runs still find the multi.
        unsafe { curl_multi_cleanup(this.handle) };
    }
    // SAFETY: per the caller.
    drop(unsafe { Box::from_raw(multi) });
}

/// `Multi::get_multi_handle`.
/// # Safety
/// As [`swr_v1_curl_multi_free`], without freeing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_curl_multi_handle(multi: *mut Multi) -> *mut CURLM {
    // SAFETY: per the caller.
    unsafe { self::multi(multi) }.handle
}

/// `Multi::get_running_handles`.
/// # Safety
/// As [`swr_v1_curl_multi_handle`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_curl_multi_running(multi: *mut Multi) -> c_int {
    // SAFETY: per the caller.
    unsafe { self::multi(multi) }.running.get()
}

/// The coroutine waiting on the multi, if any (`Multi::check_bound_co`).
/// # Safety
/// As [`swr_v1_curl_multi_handle`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_curl_multi_co(multi: *mut Multi) -> *mut c_void {
    // SAFETY: per the caller.
    unsafe { self::multi(multi) }.co.get()
}

/// `Multi::add_handle`.
/// # Safety
/// As [`swr_v1_curl_multi_handle`]; `handle` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_curl_multi_add_handle(
    multi: *mut Multi,
    handle: *mut Handle,
) -> CURLMcode {
    // SAFETY: per the caller.
    unsafe { self::multi(multi) }.add_handle(handle)
}

/// `Multi::remove_handle`.
/// # Safety
/// As [`swr_v1_curl_multi_add_handle`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_curl_multi_remove_handle(
    multi: *mut Multi,
    handle: *mut Handle,
) -> CURLMcode {
    // SAFETY: per the caller.
    unsafe { self::multi(multi) }.remove_handle(handle)
}

/// `Multi::perform`, after C++'s `check_bound_co`.
/// # Safety
/// As [`swr_v1_curl_multi_handle`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_curl_multi_perform(multi: *mut Multi) -> CURLMcode {
    // SAFETY: per the caller.
    unsafe { self::multi(multi) }.perform()
}

/// `Multi::select` in the coroutine C++'s `check_bound_co` returned.
/// # Safety
/// As [`swr_v1_curl_multi_handle`]; `co` is the running coroutine.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_curl_multi_select(
    multi: *mut Multi,
    co: *mut c_void,
    timeout: f64,
) -> c_long {
    // SAFETY: per the caller.
    unsafe { self::multi(multi) }.select(co, timeout)
}

/// `Multi::exec`; [`EXEC_NEEDS_COROUTINE`] when it must wait outside a
/// coroutine or while another one waits.
/// # Safety
/// As [`swr_v1_curl_multi_add_handle`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_curl_multi_exec(
    multi: *mut Multi,
    handle: *mut Handle,
) -> CURLcode {
    // SAFETY: per the caller.
    unsafe { self::multi(multi) }.exec(handle)
}

/// The multi's timer fired.
/// # Safety
/// As [`swr_v1_curl_multi_handle`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_curl_multi_on_timer(multi: *mut Multi) {
    // SAFETY: per the caller.
    let this = unsafe { self::multi(multi) };
    this.timer.set(ptr::null_mut());
    this.callback(None);
}

/// The multi's deferred wake-up: resume its waiting coroutine.
/// # Safety
/// As [`swr_v1_curl_multi_handle`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_curl_multi_deferred(multi: *mut Multi) {
    // SAFETY: per the caller.
    let this = unsafe { self::multi(multi) };
    this.defer_callback.set(false);
    let co = this.co.get();
    if !co.is_null() {
        // SAFETY: the coroutine waiting on this multi.
        unsafe { (this.ops.resume)(co) };
    }
}

/// A reactor event (`CURL_CSELECT_*` in `bitmask`) on a cURL socket.
/// # Safety
/// `object` is the reactor socket's `object`, which the multi set.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_curl_socket_event(object: *mut c_void, bitmask: c_int) {
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

#[cfg(test)]
mod tests;
