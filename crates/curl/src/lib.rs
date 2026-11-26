//! Swoole's coroutine cURL (upstream's `swoole_curl.cc`, built with
//! `--enable-swoole-curl`): the multi engine behind the native `curl_exec` and
//! `curl_multi_*` functions. A [`Multi`] drives a
//! libcurl multi handle through its socket and timer callbacks: each socket
//! curl watches joins the reactor, each timeout becomes a timer, and the
//! coroutine waiting on the transfer is resumed when either fires.
//!
//! Each multi carries its reactor, timer and coroutine operations; tests inject
//! recording callbacks into the same engine. `swr_v1_curl_check_handle` reads
//! the request state and reports PHP errors (the `php` feature).
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
    cell::Cell,
    collections::BTreeMap,
    ffi::{c_int, c_long, c_void},
    marker::PhantomData,
    ptr,
    rc::{Rc, Weak},
};
use swoole_rs_corebase::CellExt;
use swoole_rs_corebase::log::{self, set_last_error};
use swoole_rs_corebase::reactor::Events;
use swoole_rs_corebase::timer::Deadline;
use swoole_rs_curl_sys::{
    CURL, CURL_POLL_IN, CURL_POLL_INOUT, CURL_POLL_OUT, CURL_POLL_REMOVE, CURL_SOCKET_TIMEOUT,
    CURLE_ABORTED_BY_CALLBACK, CURLE_FAILED_INIT, CURLE_OK, CURLINFO_PRIVATE, CURLM, CURLM_OK,
    CURLMOPT_SOCKETDATA, CURLMOPT_SOCKETFUNCTION, CURLMOPT_TIMERDATA, CURLMOPT_TIMERFUNCTION,
    CURLMSG_DONE, CURLMcode, CURLOPT_PRIVATE, CURLcode, LiveMulti, MultiHandle, curl_easy_getinfo,
    curl_easy_setopt, curl_multi_cleanup, curl_multi_info_read, curl_multi_init, curl_multi_setopt,
    curl_multi_timeout, curl_socket_t,
};
#[cfg(any(feature = "php", test))]
use swoole_rs_curl_sys::{
    CURL_CSELECT_ERR, CURL_CSELECT_IN, CURL_CSELECT_OUT, CURLM_INTERNAL_ERROR,
};

/// `SW_OK` and `SW_ERR`.
const OK: c_int = swoole_rs_corebase::constants::ResultCode::SW_OK as c_int;
const ERR: c_int = swoole_rs_corebase::constants::ResultCode::SW_ERR as c_int;
/// `SW_ERROR_CO_CANCELED`.
const CO_CANCELED: c_int = swoole_rs_error::ErrorCode::SW_ERROR_CO_CANCELED as c_int;
/// What [`swr_v1_curl_multi_exec`] returns when it must wait without a
/// coroutine to wait in (or one already bound): C++ raises the fatal error.
pub const EXEC_NEEDS_COROUTINE: CURLcode = -1;

/// A live easy handle for `'a`, borrowed from a PHP cURL object's `ch->cp`.
/// Only an export's parameter makes one, where the C caller's contract stands for the constructor, as it does for a `&T`
/// parameter there. A `CURL *`'s layout, and neither `Send` nor `Sync`.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Easy<'a>(*mut CURL, PhantomData<&'a CURL>);

impl<'a> Easy<'a> {
    /// The easy handle's Swoole handle, which lives until the unsafe
    /// [`swr_v1_curl_destroy_handle`], whose caller promises nothing uses it
    /// afterwards: within `'a`, no export frees it.
    fn handle(self) -> Option<&'a Handle> {
        // SAFETY: null or the live handle create_handle made, as above.
        unsafe { self.handle_ptr().as_ref() }
    }

    /// `CURLINFO_PRIVATE`: the easy handle's Swoole handle, or null without one.
    fn handle_ptr(self) -> *mut Handle {
        let mut handle: *mut Handle = ptr::null_mut();
        // SAFETY: the live easy handle; CURLINFO_PRIVATE writes a pointer.
        if unsafe { curl_easy_getinfo(self.0, CURLINFO_PRIVATE, &raw mut handle) } == CURLE_OK {
            handle
        } else {
            ptr::null_mut()
        }
    }

    /// What `CURLINFO_PRIVATE` holds now, to set back with [`Easy::set_private`].
    fn private(self) -> Private {
        Private(self.handle_ptr())
    }

    /// Sets `CURLINFO_PRIVATE`, which [`Easy::handle`] reads back.
    fn set_private(self, handle: Private) {
        // SAFETY: the live easy handle; CURLOPT_PRIVATE stores a pointer and reads nothing.
        unsafe { curl_easy_setopt(self.0, CURLOPT_PRIVATE, handle.0.cast::<c_void>()) };
    }
}

/// What an easy handle's `CURLINFO_PRIVATE` may hold: null, or a Swoole handle
/// [`swr_v1_curl_create_handle`] made for it that [`swr_v1_curl_destroy_handle`]
/// has not freed. Only [`Private::NONE`], [`Private::new`] and [`Easy::private`]
/// make one.
#[derive(Clone, Copy)]
struct Private(*mut Handle);

impl Private {
    const NONE: Self = Self(ptr::null_mut());

    /// A new Swoole handle, which only [`swr_v1_curl_destroy_handle`] frees.
    fn new(handle: Handle) -> Self {
        Self(Box::into_raw(Box::new(handle)))
    }
}

/// `struct swr_v1_curl_handle`: the easy handle's multi while it is in one,
/// and the multi `curl_exec` runs it in. C++ only asks whether `multi` is set.
// `repr(C)`: C's `swr_v1_curl_handle` (`native/hooks/swoole_rs_curl.h`), used by the native cURL adapter.
#[repr(C)]
pub struct Handle {
    cp: *mut CURL,
    /// The multi the easy handle is in, which nothing reaches once it is freed.
    multi: Cell<Option<Weak<Multi>>>,
    /// The handle's reference to its exec multi, which [`swr_v1_curl_destroy_handle`]
    /// releases. C++ never reads it: to C it is an opaque pointer of the same size.
    easy_multi: Cell<Option<Rc<Multi>>>,
}

const _: () = assert!(
    size_of::<Cell<Option<Rc<Multi>>>>() == size_of::<*mut Multi>()
        && size_of::<Cell<Option<Weak<Multi>>>>() == size_of::<*mut Multi>(),
    "C's `void *multi` and `void *easy_multi`"
);

impl Handle {
    /// A handle for `cp`, in `multi`'s set if any, which only
    /// [`swr_v1_curl_destroy_handle`] frees.
    fn create(cp: Easy<'_>, multi: Option<Weak<Multi>>) -> *mut Self {
        let handle = Private::new(Self {
            cp: cp.0,
            multi: Cell::new(multi),
            easy_multi: Cell::new(None),
        });
        cp.set_private(handle);
        handle.0
    }

    /// The multi the easy handle is in, while that multi lives.
    fn multi(&self) -> Option<Rc<Multi>> {
        self.multi
            .with_taken(|multi| multi.as_ref().and_then(Weak::upgrade))
    }

    /// A pushed handle inherits its parent's multi.
    #[cfg(feature = "php")]
    fn multi_ref(&self) -> Option<Weak<Multi>> {
        self.multi.with_taken(|multi| multi.clone())
    }

    /// The easy handle it was made for, which outlives it: ext/curl destroys the
    /// handle before it cleans the easy handle up.
    fn easy(&self) -> Easy<'_> {
        Easy(self.cp, PhantomData)
    }
}

// libcurl's declarations, with the handles typed as live. Each adds or removes the
// easy handle, or drives the transfers of those added, and may run callbacks
// meanwhile: the multi's socket and timer callbacks, which take the multi by shared
// reference, and the easy handles' own, which ext/curl set under the contract `Easy`
// carries. `curl_multi_socket_action` writes the running count into its cell, which
// those callbacks only read through the multi. Without a multi handle libcurl
// answers `CURLM_BAD_HANDLE`.
unsafe extern "C" {
    safe fn curl_multi_add_handle(multi: Option<LiveMulti<'_>>, easy: Easy<'_>) -> CURLMcode;
    safe fn curl_multi_remove_handle(multi: Option<LiveMulti<'_>>, easy: Easy<'_>) -> CURLMcode;
    safe fn curl_multi_socket_action(
        multi: Option<LiveMulti<'_>>,
        s: curl_socket_t,
        ev_bitmask: c_int,
        running_handles: &Cell<c_int>,
    ) -> CURLMcode;
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
/// one waits, so nothing is taken out across either. C++ holds one reference
/// (in `curl_multi_init`) and a queued wake-up another; the socket and
/// timer callbacks reach it through [`Multi::this`].
pub struct Multi {
    /// This multi, for what reaches it later: its sockets, timer and wake-up.
    this: Weak<Multi>,
    host: &'static host::Ops,
    /// None when libcurl could make no handle: its calls then refuse the null one.
    handle: Option<MultiHandle>,
    /// The id of the pending timer.
    timer: Cell<Option<swoole_rs_corebase::timer::Held>>,
    /// The coroutine waiting on this multi.
    co: Cell<Option<Waiter>>,
    running: Cell<c_int>,
    defer_callback: Cell<bool>,
    /// C++ let go of it while a deferred wake-up was queued: the wake-up
    /// closes it.
    released: Cell<bool>,
    /// The selector: a timeout to report to curl and sockets with events (a
    /// set, in arrival order).
    timer_callback: Cell<bool>,
    /// The sockets curl watches, by fd: what `curl_multi_assign` would carry.
    /// It and `active` are taken out and put back around plain map and
    /// vector work only, never around a libcurl call, which runs the socket and
    /// timer callbacks.
    sockets: Cell<SocketMap>,
    active: Cell<Vec<Rc<Socket>>>,
}

type SocketMap = BTreeMap<curl_socket_t, Rc<Socket>>;

/// `Multi::get_event`: curl's poll action as reactor events.
fn events(action: c_int) -> Events {
    match action {
        CURL_POLL_IN => Events::READ,
        CURL_POLL_OUT => Events::WRITE,
        CURL_POLL_INOUT => Events::READ | Events::WRITE,
        _ => Events::NONE,
    }
}

/// `translate_events_to_poll`.
#[cfg(any(feature = "php", test))]
fn poll_events(events: Events) -> i16 {
    let mut poll = 0;
    if events.intersects(Events::READ) {
        poll |= libc::POLLIN;
    }
    if events.intersects(Events::WRITE) {
        poll |= libc::POLLOUT;
    }
    poll
}

struct ReactorUnavailable;

/// The `CURL_CSELECT_*` bits of a `poll` result.
#[cfg(any(feature = "php", test))]
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

impl Multi {
    /// Adds `socket` to the active sockets, a set.
    fn activate(&self, socket: &Rc<Socket>) {
        self.active.with_taken(|active| {
            if !active.iter().any(|member| Rc::ptr_eq(member, socket)) {
                active.push(Rc::clone(socket));
            }
        });
    }

    fn has_active(&self) -> bool {
        self.active.with_taken(|active| !active.is_empty())
    }

    /// `Multi::Multi`: a multi handle whose socket and timer callbacks reach
    /// this multi, which therefore stays where it is until [`Multi::close`].
    fn create() -> Rc<Self> {
        Self::with_host(&host::NATIVE)
    }

    fn with_host(host: &'static host::Ops) -> Rc<Self> {
        let made = curl_multi_init();
        let handle = made.as_ref().map_or(ptr::null_mut(), MultiHandle::as_ptr);
        let multi = Rc::new_cyclic(|this| Self {
            this: this.clone(),
            host,
            handle: made,
            timer: Cell::new(None),
            co: Cell::new(None),
            running: Cell::new(0),
            defer_callback: Cell::new(false),
            released: Cell::new(false),
            timer_callback: Cell::new(false),
            sockets: Cell::default(),
            active: Cell::default(),
        });
        let data = Rc::as_ptr(&multi).cast_mut().cast::<c_void>();
        let on_socket: OnSocket = handle_socket;
        let on_timer: OnTimer = handle_timeout;
        for (option, value) in [
            (CURLMOPT_SOCKETDATA, data),
            (CURLMOPT_TIMERDATA, data),
            (CURLMOPT_SOCKETFUNCTION, on_socket as *mut c_void),
            (CURLMOPT_TIMERFUNCTION, on_timer as *mut c_void),
        ] {
            // SAFETY: the handle just made (curl refuses a null one), and for each
            // option the one pointer it reads: the multi, which outlives the handle
            // (only `Multi::close` cleans the handle up, before the last reference to
            // the multi goes), or the callback, which curl calls with its own
            // arguments and that multi as its data, what `OnSocket` and `OnTimer`
            // take. A function pointer passes as a data pointer does (POSIX).
            unsafe { curl_multi_setopt(handle, option, value) };
        }
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
        self.timer.get().is_some() || !self.sockets.with_taken(|sockets| sockets.is_empty())
    }

    fn add_timer(&self, timeout_ms: c_long) {
        self.del_timer();
        self.timer.set((self.host.timer_add)(timeout_ms, self));
    }

    fn del_timer(&self) {
        if let Some(timer) = self.timer.get()
            && (self.host.timer_available)()
        {
            (self.host.timer_del)(timer);
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
        self.on_timeout(self.timeout(0).1).unwrap_or(());
    }

    /// `Multi::handle_timeout`.
    fn on_timeout(&self, timeout_ms: c_long) -> Result<(), ReactorUnavailable> {
        if !(self.host.event_available)() {
            return Err(ReactorUnavailable);
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
        Ok(())
    }

    /// `Multi::set_event`: whether the socket is in the reactor for `action`.
    fn set_event(&self, sockfd: curl_socket_t, action: c_int) -> bool {
        if !(self.host.event_available)() {
            return false;
        }
        (self.host.install_handlers)();
        let known = self
            .sockets
            .with_taken(|sockets| sockets.get(&sockfd).cloned());
        let socket = known.unwrap_or_else(|| {
            let socket = Rc::new(Socket {
                multi: self.this.clone(),
                socket: host::ReactorSocket::new(sockfd, self.host),
                bitmask: Cell::new(0),
                sockfd: Cell::new(0),
                action: Cell::new(0),
                deleted: Cell::new(false),
            });
            // The reactor socket reports to the socket while it lives.
            socket.socket.set_owner(Rc::downgrade(&socket));
            let replaced = self
                .sockets
                .with_taken(|sockets| sockets.insert(sockfd, Rc::clone(&socket)));
            drop(replaced);
            socket
        });
        socket.sockfd.set(sockfd);
        socket.action.set(action);
        let events = events(action);
        if socket.socket.has_interest() {
            socket.socket.set(events)
        } else {
            socket.socket.add(events)
        }
    }

    /// `Multi::del_event`: whether curl's socket was known.
    fn del_event(&self, sockfd: curl_socket_t) -> bool {
        let Some(socket) = self.sockets.with_taken(|sockets| sockets.remove(&sockfd)) else {
            return false;
        };
        // A set: at most one copy, dropped once the vector is back.
        let copy = self.active.with_taken(|active| {
            let index = active
                .iter()
                .position(|active| Rc::ptr_eq(active, &socket))?;
            Some(active.remove(index))
        });
        drop(copy);
        // Out of the map, and the selector skips a deleted socket.
        socket.socket.free();
        socket.deleted.set(true);
        true
    }

    fn add_handle(&self, handle: &Handle) -> CURLMcode {
        let result = curl_multi_add_handle(self.live(), handle.easy());
        if result == CURLM_OK {
            handle.multi.set(Some(self.this.clone()));
        }
        result
    }

    fn remove_handle(&self, handle: &Handle) -> CURLMcode {
        let result = curl_multi_remove_handle(self.live(), handle.easy());
        handle.multi.set(None);
        if result == CURLM_OK {
            // Refresh the running count even if no socket event follows.
            self.timer_callback.set(true);
        }
        result
    }

    /// `Multi::selector_prepare`: put back in the reactor the sockets a
    /// waiter-less event took out of it.
    fn selector_prepare(&self) {
        let sockets: Vec<Rc<Socket>> = self
            .sockets
            .with_taken(|sockets| sockets.values().cloned().collect());
        for socket in sockets {
            if socket.socket.removed() {
                socket.socket.add(events(socket.action.get()));
            }
        }
    }

    /// `Multi::exec`: run one easy handle to completion (`curl_exec`).
    fn exec(&self, handle: &Handle) -> CURLcode {
        if self.add_handle(handle) != CURLM_OK {
            return CURLE_FAILED_INIT;
        }
        let mut canceled = false;
        loop {
            self.selector_prepare();
            if self.wait_event() {
                let co = (self.host.current)();
                let Some(waiter) = Waiter::running(co, (self.host.current_cid)())
                    .filter(|_| self.co.get().is_none())
                else {
                    return EXEC_NEEDS_COROUTINE;
                };
                self.co.set(Some(waiter));
                // Upstream's `yield_ex(-1)`: curl's own timers end the transfer.
                canceled = (self.host.yield_)(co, Deadline::never(), &mut |waker| {
                    self.co.set(Some(waiter.parked(waker)));
                });
                self.co.set(None);
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

    /// Copy the completion fields before another libcurl operation can invalidate the message.
    fn next_message(&self, pending: &mut c_int) -> Option<(c_int, *mut CURL, CURLcode)> {
        let message = curl_multi_info_read(self.live(), pending);
        // SAFETY: libcurl returns null or a live message until the next multi operation.
        let message = unsafe { message.as_ref() }?;
        Some((message.msg, message.easy_handle, message.result()))
    }

    /// `Multi::read_info`: the first finished transfer's result.
    fn read_info(&self) -> CURLcode {
        while let Some((kind, _, result)) = self.next_message(&mut 0) {
            if kind == CURLMSG_DONE {
                return result;
            }
            log::warning(c"Multi::read_info", format_args!("CURLMSG default"));
        }
        CURLE_OK
    }

    /// `Multi::selector_poll`: what the sockets have ready now, without
    /// waiting, for callers that don't `curl_multi_select`.
    #[cfg(any(feature = "php", test))]
    fn selector_poll(&self) -> CURLMcode {
        let mut fds: Vec<libc::pollfd> = self.sockets.with_taken(|sockets| {
            sockets
                .iter()
                .map(|(&fd, socket)| libc::pollfd {
                    fd,
                    events: poll_events(events(socket.action.get())),
                    revents: 0,
                })
                .collect()
        });
        if fds.is_empty() {
            return CURLM_OK;
        }
        if let Err(error) = swoole_rs_corebase::sys::poll(&mut fds, 0) {
            return if error.raw_os_error() == Some(libc::EINTR) {
                CURLM_OK
            } else {
                CURLM_INTERNAL_ERROR
            };
        }
        for fd in fds.iter().filter(|fd| fd.revents != 0) {
            let Some(socket) = self
                .sockets
                .with_taken(|sockets| sockets.get(&fd.fd).cloned())
            else {
                continue;
            };
            socket
                .bitmask
                .set(socket.bitmask.get() | select_bits(fd.revents));
            self.activate(&socket);
        }
        CURLM_OK
    }

    /// `Multi::perform` (`curl_multi_exec`), once C++ checked the coroutine.
    #[cfg(any(feature = "php", test))]
    fn perform(&self) -> CURLMcode {
        let (result, timeout_ms) = self.timeout(-1);
        if result != CURLM_OK {
            return result;
        }
        // Adding a handle schedules an immediate timeout that starts the socket API.
        if timeout_ms == 0 {
            self.timer_callback.set(true);
        }
        if !self.timer_callback.get() && !self.has_active() {
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
        curl_multi_socket_action(self.live(), sockfd, bitmask, &self.running)
    }

    /// `Multi::selector_finish`: report the timeout and the sockets' events.
    fn selector_finish(&self) -> CURLMcode {
        self.del_timer();
        let mut result = CURLM_OK;
        if self.timer_callback.replace(false) {
            result = self.socket_action(CURL_SOCKET_TIMEOUT, 0);
        }
        while result == CURLM_OK && self.has_active() {
            // curl may remove sockets while it runs: work on a copy, which
            // keeps what it removes until the loop moves past it.
            let active = self.active.take();
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
        drop(self.active.take());
        result
    }

    /// `Multi::select` (`curl_multi_select`) in coroutine `co`: how many
    /// sockets have events.
    #[cfg(feature = "php")]
    fn select(&self, co: *mut c_void, timeout: f64) -> c_long {
        if self.has_active() || self.timer_callback.get() {
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
        let waiter = Waiter::running(co, (self.host.current_cid)());
        self.co.set(waiter);
        let mut arm = |waker| self.co.set(waiter.map(|waiter| waiter.parked(waker)));
        (self.host.yield_)(co, Deadline::from_timeout(timeout), &mut arm);
        self.co.set(None);
        self.ready()
    }

    /// How many sockets have events.
    #[cfg(any(feature = "php", test))]
    fn ready(&self) -> c_long {
        let ready = self.active.with_taken(|active| active.len());
        c_long::try_from(ready).unwrap_or(c_long::MAX)
    }

    /// `Multi::callback`: a socket event (or, for `None`, the timeout).
    fn callback(&self, socket: Option<&Socket>) {
        if self.released.get() {
            return;
        }
        if socket.is_none() {
            self.timer_callback.set(true);
        }
        if self.co.get().is_none() {
            match socket {
                Some(socket) => {
                    socket.socket.del();
                }
                None => self.del_timer(),
            }
            return;
        }
        // The map's copy: the reactor reports only the sockets in it.
        let sockfd = socket.map(|socket| socket.sockfd.get());
        if let Some(socket) = self
            .sockets
            .with_taken(|sockets| sockets.get(&sockfd?).cloned())
        {
            self.activate(&socket);
        }
        (self.host.defer)(self);
    }
}

/// The coroutine waiting on a multi, which is the running one as it starts to wait:
/// its id, and its park's waker once it parks.
#[derive(Clone, Copy)]
struct Waiter {
    cid: c_long,
    waker: Option<swoole_rs_corebase::coroutine::Waker>,
}

impl Waiter {
    /// `co`, the running coroutine, or none outside one.
    fn running(co: *mut c_void, cid: c_long) -> Option<Self> {
        (!co.is_null()).then_some(Self { cid, waker: None })
    }

    /// The waiter, parked: `waker` wakes it.
    const fn parked(self, waker: swoole_rs_corebase::coroutine::Waker) -> Self {
        Self {
            waker: Some(waker),
            ..self
        }
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
    multi.on_timeout(timeout_ms).map_or(ERR, |()| OK)
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

/// One strong reference transferred from Rust to a PHP multi handle at creation
/// and returned to [`swr_v1_curl_multi_free`] at destruction.
#[repr(transparent)]
pub struct Released(ptr::NonNull<Multi>);

/// `Multi::release` of the multi the PHP object owns; nothing for null.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_curl_multi_free(multi: Option<Released>) {
    let Some(Released(multi)) = multi else {
        return;
    };
    // SAFETY: the reference attached during curl_multi_init, as the type carries.
    release(unsafe { Rc::from_raw(multi.as_ptr()) });
}

/// `Multi::exec`; [`EXEC_NEEDS_COROUTINE`] when it must wait outside a
/// coroutine or while another one waits.
pub fn swr_v1_curl_multi_exec(multi: &Multi, handle: &Handle) -> CURLcode {
    multi.exec(handle)
}

/// The multi's deferred wake-up: resume its waiting coroutine. True when
/// [`swr_v1_curl_multi_free`] released the multi meanwhile: close it now.
fn multi_deferred(this: &Multi) -> bool {
    this.defer_callback.set(false);
    if this.released.get() {
        return true;
    }
    if let Some(waker) = this.co.get().and_then(|waiter| waiter.waker) {
        (this.host.wake)(waker);
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
pub extern "C" fn swr_v1_curl_get_handle(cp: Easy<'_>) -> *mut Handle {
    cp.handle_ptr()
}

/// `swoole::curl::create_handle`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_curl_create_handle(cp: Easy<'_>) -> *mut Handle {
    Handle::create(cp, None)
}

/// `if (handle->multi) multi->remove_handle(handle)`: the handle leaves the multi
/// it is in, while that multi lives.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_curl_handle_leave_multi(handle: &mut Handle) {
    if let Some(multi) = handle.multi() {
        multi.remove_handle(handle);
    }
}

/// An easy handle on which no transfer runs, and whose Swoole handle nothing else
/// uses, during the call: the parameter of the exports that reset or run it, where
/// the C caller's contract stands for the constructor, as for [`Easy`].
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Idle<'a>(Easy<'a>);

/// An [`Idle`] easy handle whose Swoole handle nothing uses after the call:
/// [`swr_v1_curl_destroy_handle`]'s parameter, made as [`Idle`] is.
#[repr(transparent)]
pub struct Retired<'a>(Easy<'a>);

unsafe extern "C" {
    /// libcurl's, with the handle typed as idle: it clears the handle's options and
    /// state, which no transfer is using.
    safe fn curl_easy_reset(easy: Idle<'_>);
}

/// `swoole::curl::destroy_handle`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_curl_destroy_handle(cp: Retired<'_>) {
    let Retired(cp) = cp;
    let handle = cp.handle_ptr();
    if handle.is_null() {
        return;
    }
    // SAFETY: the handle create_handle made, which only this frees.
    let handle = unsafe { Box::from_raw(handle) };
    if let Some(multi) = handle.easy_multi.take() {
        release(multi);
    }
    cp.set_private(Private::NONE);
}

/// `swoole_curl_easy_reset`: `curl_easy_reset`, keeping the handle.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_curl_easy_reset(cp: Idle<'_>) {
    let handle = cp.0.private();
    curl_easy_reset(cp);
    // The easy handle keeps the Swoole handle it held.
    cp.0.set_private(handle);
}

mod host;
pub use host::{swoole_curl_check_bound_co, swoole_curl_easy_perform};
#[cfg(feature = "php")]
mod callbacks;
#[cfg(feature = "php")]
mod getinfo;
#[cfg(feature = "php")]
mod php;
#[cfg(feature = "php")]
mod php_handle;
#[cfg(feature = "php")]
mod php_multi;
#[cfg(feature = "php")]
mod postfields;
#[cfg(feature = "php")]
mod registration;
#[cfg(feature = "php")]
pub use registration::minit;
#[cfg(feature = "php")]
mod setopt;
#[cfg(feature = "php")]
pub use php::swr_v1_curl_check_handle;
#[cfg(test)]
#[path = "../../../tests/modules/hooks/curl.rs"]
mod tests;
