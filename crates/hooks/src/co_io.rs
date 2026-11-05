//! What the coroutine socket's Rust operations need from C++, installed once
//! (`swr_v1_hook_co_socket_host`): Swoole's thread-local last error and its
//! messages, the coroutine switch, timers, the reactor and the write buffer. Over
//! it, the record's `set_err`, `check_return_value` and `is_available`, and the
//! per-direction timer (`TimerController`).
use crate::co_socket::CoSocket;
use crate::shutdown::{READ, swr_v1_hook_socket_bound_co, swr_v1_hook_socket_event_name};
use core::ffi::{CStr, c_char, c_int, c_long, c_void};
use core::ptr;
use core::sync::atomic::{AtomicPtr, Ordering};

/// `SW_ERROR_CO_SOCKET_CLOSE_WAIT`.
pub(crate) const CLOSE_WAIT_ERROR: c_int = 10019;
/// `SW_WAIT`.
const WAIT: c_int = 2;

/// Native effects. Each takes the record native code handed Rust.
#[repr(C)]
pub struct Host {
    /// `swoole_set_last_error`.
    pub set_last_error: unsafe extern "C" fn(c_int),
    /// `swoole_strerror`.
    pub strerror: unsafe extern "C" fn(c_int) -> *const c_char,
    /// `Coroutine::get_current_safe()`.
    pub current: unsafe extern "C" fn() -> *mut c_void,
    /// A coroutine's id.
    pub cid: unsafe extern "C" fn(*mut c_void) -> c_long,
    /// `Coroutine::print_socket_bound_error(fd, event, cid)`.
    pub bound_error: unsafe extern "C" fn(c_int, *const c_char, c_long),
    /// `swoole_timer_add(timeout, false, timer_callback, socket)`, or null.
    pub timer_add: unsafe extern "C" fn(*mut CoSocket, f64) -> *mut c_void,
    pub timer_del: unsafe extern "C" fn(*mut c_void),
    /// `System::sleep(0.01)`.
    pub sleep: unsafe extern "C" fn(),
    /// `swoole_event_add(events)` when `add`, else `swoole_event_set(events)`;
    /// false, with errno set, on failure.
    pub reactor: unsafe extern "C" fn(*mut CoSocket, u32, bool) -> bool,
    /// Yields `co` with `cancel(event)` as its cancel function.
    pub suspend: unsafe extern "C" fn(*mut CoSocket, *mut c_void, u32),
    /// Moves a write's `length` bytes at `*buffer` to the write buffer unless they
    /// are there already or the socket is zero-copy.
    pub buffer: unsafe extern "C" fn(*mut CoSocket, *mut *mut u8, usize),
    /// `remove_read_event`/`remove_write_event`.
    pub remove: unsafe extern "C" fn(*mut CoSocket, u32),
    /// Resumes a coroutine.
    pub resume: unsafe extern "C" fn(*mut c_void),
    /// `close()`'s release: runs the destructor callback once; the socket may be gone after.
    pub release: unsafe extern "C" fn(*mut CoSocket),
    /// `socket->ssl_shutdown()`.
    pub ssl_shutdown: unsafe extern "C" fn(*mut CoSocket),
    /// `socket->check_liveness()`, errno set when it fails.
    pub liveness: unsafe extern "C" fn(*mut CoSocket) -> bool,
    /// The read String: 0 get, 1 store, 2 extend, 3 reduce; refreshes the view.
    pub read_buffer:
        unsafe extern "C" fn(*mut CoSocket, *mut crate::packet::ReadBuffer, u32, usize),
    /// `Socket::recv/recv_all` (all selects the latter).
    pub recv: unsafe extern "C" fn(*mut CoSocket, *mut u8, usize, bool) -> isize,
    /// The native protocol callback and its trace.
    pub package_length: unsafe extern "C" fn(*mut CoSocket, u32, *mut u32) -> isize,
    /// Packet error diagnostics, with the same kinds as the packet receive.
    pub packet_fail: unsafe extern "C" fn(*mut CoSocket, u32, isize),
}

static HOST: AtomicPtr<Host> = AtomicPtr::new(ptr::null_mut());

/// Installs native code's table, which lives for the process.
///
/// # Safety
/// `host` is a live table that outlives every socket operation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_co_socket_host(host: *const Host) {
    HOST.store(host.cast_mut(), Ordering::Release);
}

/// The installed table.
pub(crate) fn host() -> &'static Host {
    // SAFETY: native code installs a process-lifetime table before any socket
    // operation runs (a static initialiser in socket.cc).
    unsafe { &*HOST.load(Ordering::Acquire) }
}

pub(crate) fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

pub(crate) fn set_errno(value: c_int) {
    #[cfg(target_os = "linux")]
    // SAFETY: a libc call with no preconditions.
    let location = unsafe { libc::__errno_location() };
    #[cfg(target_vendor = "apple")]
    // SAFETY: a libc call with no preconditions.
    let location = unsafe { libc::__error() };
    // SAFETY: the calling thread's errno.
    unsafe { *location = value };
}

/// `set_err(e)`: the record's code and message, errno and the last error.
///
/// # Safety
/// `socket` is a live record, not borrowed elsewhere for the call.
pub(crate) unsafe fn set_err(socket: *mut CoSocket, error: c_int) {
    // SAFETY: as the caller promises.
    unsafe { (*socket).err_code = error };
    set_errno(error);
    // SAFETY: a native effect with no access to the record.
    unsafe { (host().set_last_error)(error) };
    let message = if error == 0 {
        c"".as_ptr()
    } else {
        // SAFETY: as above; the message outlives the record.
        unsafe { (host().strerror)(error) }
    };
    // SAFETY: as the caller promises.
    unsafe { (*socket).err_msg = message };
}

/// `check_return_value`: clear the error after a success; publish errno after a
/// failure that set none.
///
/// # Safety
/// As [`set_err`].
pub(crate) unsafe fn check(socket: *mut CoSocket, result: isize) {
    if result >= 0 {
        // SAFETY: forwarded.
        unsafe { set_err(socket, 0) };
    // SAFETY: as the caller promises.
    } else if unsafe { (*socket).err_code } == 0 {
        // SAFETY: forwarded.
        unsafe { set_err(socket, errno()) };
    }
}

/// `is_available(event)`: no other coroutine holds `event` (`SW_EVENT_NULL`
/// skips that check), the socket is open and not waiting to close.
///
/// # Safety
/// As [`set_err`]; the record's state is live.
pub(crate) unsafe fn available(socket: *mut CoSocket, event: u32) -> bool {
    if event != 0 {
        // SAFETY: forwarded; the report may close the socket, so it is read again after.
        unsafe { check_bound(socket, event) };
    }
    // SAFETY: as the caller promises; read again after the report.
    let record = unsafe { &*socket };
    let error = if record.is_closed() {
        libc::EBADF
    // SAFETY: an open record's network::Socket, not otherwise borrowed.
    } else if unsafe { record.record() }.output.close_wait != 0 {
        CLOSE_WAIT_ERROR
    } else {
        return true;
    };
    // SAFETY: forwarded.
    unsafe { set_err(socket, error) };
    false
}

/// `get_bound_cid(event)`: the id of the coroutine `get_bound_co` finds, or 0.
///
/// # Safety
/// `socket` is a live record.
unsafe fn bound_cid(socket: *const CoSocket, event: u32) -> c_long {
    // SAFETY: as the caller promises.
    let state = unsafe { (*socket).state };
    // SAFETY: the record's live state.
    let co = unsafe { swr_v1_hook_socket_bound_co(state, event) };
    if co.is_null() {
        0
    } else {
        // SAFETY: a live native coroutine.
        unsafe { (host().cid)(co) }
    }
}

/// `get_event_str(event)`.
///
/// # Safety
/// `socket` is a live record.
unsafe fn event_str(socket: *const CoSocket, event: u32) -> &'static CStr {
    // SAFETY: as the caller promises.
    let state = unsafe { (*socket).state };
    // SAFETY: the record's live state.
    match unsafe { swr_v1_hook_socket_event_name(state, event) } {
        0 => c"reading",
        1 => c"writing",
        _ => c"reading or writing",
    }
}

/// `check_bound_co(event)`: report a coroutine with a nonzero id holding `event`.
///
/// # Safety
/// `socket` is a live record.
unsafe fn check_bound(socket: *const CoSocket, event: u32) {
    // SAFETY: as the caller promises.
    let cid = unsafe { bound_cid(socket, event) };
    if cid != 0 {
        // SAFETY: as the caller promises.
        let fd = unsafe { (*socket).sock_fd };
        // SAFETY: as the caller promises.
        let name = unsafe { event_str(socket, event) };
        // SAFETY: a native report (fatal in a running server).
        unsafe { (host().bound_error)(fd, name.as_ptr(), cid) };
    }
}

/// `TimeoutController::has_timedout(kind)` at `now`: the first call starts the
/// clock; later ones set the time left as `kind`'s timeout, or time out
/// (`ETIMEDOUT`) with less than `SW_TIMER_MIN_SEC` left. No limit unless positive.
///
/// # Safety
/// `socket` is a live, open record, not borrowed elsewhere for the call.
unsafe fn timed_out(
    socket: *mut CoSocket,
    timeout: f64,
    startup: &mut f64,
    now: f64,
    kind: c_int,
) -> bool {
    /// `SW_TIMER_MIN_SEC`.
    const MIN_SEC: f64 = 0.001;
    if timeout <= 0.0 || timeout.is_nan() {
        return false;
    }
    if *startup == 0.0 {
        *startup = now;
        return false;
    }
    let left = timeout - (now - *startup);
    if left < MIN_SEC {
        // SAFETY: forwarded.
        unsafe { set_err(socket, libc::ETIMEDOUT) };
        return true;
    }
    // SAFETY: as the caller promises.
    let socket = unsafe { &*socket };
    // SAFETY: an open record's network::Socket, not otherwise borrowed.
    let timeouts = &raw mut unsafe { socket.record() }.timeouts;
    // SAFETY: the record's timeouts.
    unsafe { swoole_rs_reactor::swr_v1_socket_set_timeout(timeouts, left, kind) };
    false
}

macro_rules! export {
    ($(#[$doc:meta])* fn $name:ident($($arg:ident: $ty:ty),*) $(-> $ret:ty)? = $call:expr) => {
        $(#[$doc])*
        ///
        /// # Safety
        /// `socket` is a live record, not borrowed elsewhere for the call.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name($($arg: $ty),*) $(-> $ret)? {
            // SAFETY: as the caller promises.
            unsafe { $call }
        }
    };
}

export!(
    /// `set_err(e)`.
    fn swr_v1_hook_co_socket_set_err(socket: *mut CoSocket, error: c_int) = set_err(socket, error)
);
export!(
    /// `check_return_value(retval)`.
    fn swr_v1_hook_co_socket_check(socket: *mut CoSocket, result: isize) = check(socket, result)
);
export!(
    /// `is_available(event)`.
    fn swr_v1_hook_co_socket_available(socket: *mut CoSocket, event: u32) -> bool = available(socket, event)
);
export!(
    /// `get_bound_cid(event)`.
    fn swr_v1_hook_co_socket_bound_cid(socket: *const CoSocket, event: u32) -> c_long = bound_cid(socket, event)
);
export!(
    /// `check_bound_co(event)`.
    fn swr_v1_hook_co_socket_check_bound(socket: *const CoSocket, event: u32) = check_bound(socket, event)
);
export!(
    /// `get_event_str(event)`, a static string.
    fn swr_v1_hook_co_socket_event_str(socket: *const CoSocket, event: u32) -> *const c_char = event_str(socket, event).as_ptr()
);

/// `has_timedout(kind)` with `startup` its controller's clock and `now` `microtime()`.
///
/// # Safety
/// `socket` is a live, open record, not borrowed elsewhere for the call; `startup`
/// is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_co_socket_timed_out(
    socket: *mut CoSocket,
    timeout: f64,
    startup: *mut f64,
    now: f64,
    kind: c_int,
) -> bool {
    // SAFETY: as the caller promises.
    let startup = unsafe { &mut *startup };
    // SAFETY: as the caller promises.
    unsafe { timed_out(socket, timeout, startup, now, kind) }
}

/// `catch_read_error`/`catch_write_error(errno) == SW_WAIT`.
///
/// # Safety
/// `socket` is an open record.
pub(crate) unsafe fn would_block(socket: *mut CoSocket) -> bool {
    let error = errno();
    // SAFETY: as the caller promises.
    let socket = unsafe { &*socket };
    // SAFETY: an open record's network::Socket, not otherwise borrowed.
    unsafe { socket.record() }.catch_error(error) == WAIT
}

/// A timer slot's value while a negative timeout's wait runs without a timer.
const SENTINEL: *mut c_void = usize::MAX as *mut c_void;

/// `TimerController`: one wait's timer in the socket's slot for its direction.
#[repr(C)]
pub struct Timer {
    socket: *mut CoSocket,
    slot: *mut *mut c_void,
    timeout: f64,
    enabled: bool,
}

impl Timer {
    /// A timer for `event`'s direction (`SW_EVENT_READ`, else writing).
    ///
    /// # Safety
    /// `socket` is a live record; the timer does not outlive it.
    pub(crate) unsafe fn new(socket: *mut CoSocket, event: u32, timeout: f64) -> Self {
        // SAFETY: as the caller promises.
        let state = unsafe { (*socket).state };
        // SAFETY: the record's live state.
        let timers = unsafe { &raw mut (*state).timers };
        // The index is 0 or 1, so the slot stays inside the two-entry array.
        let slot = timers
            .cast::<*mut c_void>()
            .wrapping_add(usize::from(event != READ));
        Self {
            socket,
            slot,
            timeout,
            enabled: false,
        }
    }

    /// `start()`: unless the slot holds a timer already, one firing after
    /// `timeout` (false if none could be added), or the sentinel for a negative
    /// timeout. No timer for a zero timeout.
    pub(crate) fn start(&mut self) -> bool {
        // SAFETY: the live slot (see `new`).
        if self.timeout == 0.0 || !unsafe { *self.slot }.is_null() {
            return true;
        }
        self.enabled = true;
        if self.timeout > 0.0 {
            // SAFETY: a native timer for the live record.
            let node = unsafe { (host().timer_add)(self.socket, self.timeout) };
            // SAFETY: as above.
            unsafe { *self.slot = node };
            return !node.is_null();
        }
        // SAFETY: as above.
        unsafe { *self.slot = SENTINEL };
        true
    }

    /// The destructor: delete the timer this one started and clear the slot.
    pub(crate) fn stop(&mut self) {
        // SAFETY: the live slot (see `new`).
        let node = unsafe { *self.slot };
        if !self.enabled || node.is_null() {
            return;
        }
        if node != SENTINEL {
            // SAFETY: the native timer this slot holds.
            unsafe { (host().timer_del)(node) };
        }
        // SAFETY: as above.
        unsafe { *self.slot = ptr::null_mut() };
    }
}

/// `TimerController(socket, event, timeout)`.
///
/// # Safety
/// `socket` is a live record; the timer does not outlive it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_co_timer_new(
    socket: *mut CoSocket,
    event: u32,
    timeout: f64,
) -> Timer {
    // SAFETY: as the caller promises.
    unsafe { Timer::new(socket, event, timeout) }
}

/// `TimerController::start`.
///
/// # Safety
/// `timer` is live, made by [`swr_v1_hook_co_timer_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_co_timer_start(timer: *mut Timer) -> bool {
    // SAFETY: as the caller promises.
    unsafe { &mut *timer }.start()
}

/// `~TimerController`.
///
/// # Safety
/// As [`swr_v1_hook_co_timer_start`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_co_timer_stop(timer: *mut Timer) {
    // SAFETY: as the caller promises.
    unsafe { &mut *timer }.stop();
}

/// A recording host for the operations' tests. Each thread records its own events.
#[cfg(test)]
pub(crate) mod test_host {
    use super::*;
    use std::cell::{Cell, RefCell};

    type Suspend = Box<dyn FnMut(*mut CoSocket, u32)>;
    type OnResume = Box<dyn FnMut(c_long)>;
    type OnRelease = Box<dyn FnMut(*mut CoSocket)>;

    thread_local! {
        static EVENTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
        pub(crate) static TIMER_OK: Cell<bool> = const { Cell::new(true) };
        pub(crate) static REACTOR_OK: Cell<bool> = const { Cell::new(true) };
        /// What a suspended coroutine does before it is resumed.
        pub(crate) static ON_SUSPEND: RefCell<Option<Suspend>> = RefCell::new(None);
        /// What the bound-coroutine report does after it is recorded.
        pub(crate) static ON_BOUND: RefCell<Option<Box<dyn FnMut()>>> = RefCell::new(None);
        /// What a resumed coroutine (its id) and the destructor callback do.
        pub(crate) static ON_RESUME: RefCell<Option<OnResume>> = RefCell::new(None);
        pub(crate) static ON_RELEASE: RefCell<Option<OnRelease>> = RefCell::new(None);
        /// The liveness probe's answer and errno; the errno a TLS shutdown leaves.
        pub(crate) static PROBE: Cell<(bool, c_int)> = const { Cell::new((true, 0)) };
        pub(crate) static SSL_ERRNO: Cell<c_int> = const { Cell::new(0) };
        static NODES: Cell<usize> = const { Cell::new(0) };
    }

    fn hook<T: ?Sized>(
        slot: &'static std::thread::LocalKey<RefCell<Option<Box<T>>>>,
        run: impl FnOnce(&mut T),
    ) {
        let taken = slot.take();
        if let Some(mut taken) = taken {
            run(&mut taken);
            if slot.with_borrow(Option::is_none) {
                slot.set(Some(taken));
            }
        }
    }

    unsafe extern "C" fn resume(co: *mut c_void) {
        let cid = unsafe { *co.cast::<c_long>() };
        log(format!("resume:{cid}"));
        hook(&ON_RESUME, |f| f(cid));
    }
    unsafe extern "C" fn release(socket: *mut CoSocket) {
        log("release");
        hook(&ON_RELEASE, |f| f(socket));
    }
    unsafe extern "C" fn ssl_shutdown(_: *mut CoSocket) {
        log("ssl-shutdown");
        set_errno(SSL_ERRNO.get());
    }
    unsafe extern "C" fn liveness(_: *mut CoSocket) -> bool {
        log("probe");
        let (alive, error) = PROBE.get();
        set_errno(error);
        alive
    }

    pub(crate) fn log(event: impl Into<String>) {
        EVENTS.with_borrow_mut(|events| events.push(event.into()));
    }

    pub(crate) fn take() -> Vec<String> {
        EVENTS.with_borrow_mut(core::mem::take)
    }

    /// Installs the host and clears this thread's events and settings.
    pub(crate) fn install() {
        // SAFETY: a static table.
        unsafe { swr_v1_hook_co_socket_host(&raw const HOST_TABLE) };
        take();
        TIMER_OK.set(true);
        REACTOR_OK.set(true);
        ON_SUSPEND.set(None);
        ON_BOUND.set(None);
        ON_RESUME.set(None);
        ON_RELEASE.set(None);
        PROBE.set((true, 0));
        SSL_ERRNO.set(0);
    }

    unsafe extern "C" fn set_last_error(error: c_int) {
        log(format!("last-error:{error}"));
    }
    unsafe extern "C" fn strerror(_: c_int) -> *const c_char {
        c"native-error".as_ptr()
    }
    unsafe extern "C" fn current() -> *mut c_void {
        ptr::dangling_mut::<u64>().cast()
    }
    /// A test coroutine is a `c_long` holding its id.
    unsafe extern "C" fn cid(co: *mut c_void) -> c_long {
        let cid = unsafe { *co.cast::<c_long>() };
        log(format!("cid:{cid}"));
        cid
    }
    unsafe extern "C" fn bound_error(fd: c_int, event: *const c_char, cid: c_long) {
        let event = unsafe { CStr::from_ptr(event) }.to_str().unwrap();
        log(format!("bound-error:{fd}:{event}:{cid}"));
        let hook = ON_BOUND.take();
        if let Some(mut hook) = hook {
            hook();
            ON_BOUND.set(Some(hook));
        }
    }
    unsafe extern "C" fn timer_add(_: *mut CoSocket, timeout: f64) -> *mut c_void {
        log(format!("timer:add:{timeout}"));
        if !TIMER_OK.get() {
            return ptr::null_mut();
        }
        NODES.set(NODES.get() + 1);
        ptr::dangling_mut::<u32>().cast()
    }
    unsafe extern "C" fn timer_del(_: *mut c_void) {
        log("timer:delete");
    }
    unsafe extern "C" fn sleep() {
        log("sleep");
    }
    unsafe extern "C" fn reactor(_: *mut CoSocket, events: u32, add: bool) -> bool {
        log(format!("{}:{events}", if add { "add" } else { "set" }));
        REACTOR_OK.get()
    }
    unsafe extern "C" fn suspend(socket: *mut CoSocket, _: *mut c_void, event: u32) {
        log(format!("yield:{event}"));
        let hook = ON_SUSPEND.take();
        if let Some(mut hook) = hook {
            hook(socket, event);
            ON_SUSPEND.set(Some(hook));
        }
    }
    unsafe extern "C" fn buffer(_: *mut CoSocket, _: *mut *mut u8, length: usize) {
        log(format!("buffer:{length}"));
    }
    unsafe extern "C" fn remove(_: *mut CoSocket, event: u32) {
        log(format!("remove:{event}"));
    }

    static HOST_TABLE: Host = Host {
        set_last_error,
        strerror,
        current,
        cid,
        bound_error,
        timer_add,
        timer_del,
        sleep,
        reactor,
        suspend,
        buffer,
        remove,
        resume,
        release,
        ssl_shutdown,
        liveness,
        read_buffer: crate::packet::tests::read_buffer,
        recv: crate::packet::tests::receive,
        package_length: crate::packet::tests::get_length,
        packet_fail: crate::packet::tests::failure,
    };
}

#[cfg(test)]
mod tests {
    use super::test_host::{ON_BOUND, TIMER_OK, install, take};
    use super::*;
    use crate::co_socket::TestSocket;
    use crate::shutdown::{WRITE, swr_v1_hook_socket_bind};

    fn slot_state(socket: &TestSocket) -> usize {
        let node = unsafe { (*socket.state()).timers[0] };
        if node.is_null() {
            0
        } else if node == SENTINEL {
            2
        } else {
            1
        }
    }

    /// The ordered gate's former scenarios 931..1003: upstream's `TimerController`
    /// start (twice in half of them) and destructor over each timeout, starting
    /// slot and `enabled`, with `swoole_timer_add` succeeding or failing.
    #[test]
    fn ported_timer_controller_matrix() {
        install();
        let times = [-1.0, 0.0, 0.002];
        for k in 0..72 {
            let (timeout, start_state) = (times[k % 3], (k / 3) % 3);
            let (timer_ok, enabled, again) =
                ((k / 9) % 2 == 1, (k / 18) % 2 == 1, (k / 36) % 2 == 1);
            let mut socket = TestSocket::new();
            let preset =
                [ptr::null_mut(), ptr::dangling_mut::<u16>().cast(), SENTINEL][start_state];
            unsafe { (*socket.state()).timers[0] = preset };
            TIMER_OK.set(timer_ok);
            take();
            // Upstream: start() adds only into an empty slot for a nonzero timeout.
            let mut expected = Vec::new();
            let mut node = start_state;
            let mut on = enabled;
            let start = |expected: &mut Vec<String>, node: &mut usize, on: &mut bool| {
                if timeout != 0.0 && *node == 0 {
                    *on = true;
                    if timeout > 0.0 {
                        expected.push(format!("timer:add:{timeout}"));
                        *node = usize::from(timer_ok);
                        return timer_ok;
                    }
                    *node = 2;
                }
                true
            };
            let first = start(&mut expected, &mut node, &mut on);
            let second = again.then(|| start(&mut expected, &mut node, &mut on));
            if on && node != 0 {
                if node != 2 {
                    expected.push("timer:delete".into());
                }
                node = 0;
            }
            let mut timer = unsafe { Timer::new(socket.ptr(), READ, timeout) };
            timer.enabled = enabled;
            assert_eq!(timer.start(), first, "scenario {}", k + 931);
            if let Some(second) = second {
                assert_eq!(timer.start(), second, "scenario {}", k + 931);
            }
            timer.stop();
            assert_eq!(take(), expected, "scenario {}", k + 931);
            assert_eq!(slot_state(&socket), node, "scenario {}", k + 931);
            unsafe { (*socket.state()).timers[0] = ptr::null_mut() };
        }
    }

    const MASKS: [u32; 6] = [0, READ, WRITE, READ | WRITE, 1 << 11, u32::MAX];

    /// Binds a reader with id `reader` and a writer with id `writer` per `bound`'s bits.
    fn bind(socket: &TestSocket, bound: usize, ids: &mut [c_long; 2]) {
        let [reader, writer] = ids;
        let co = |on: bool, id: &mut c_long| {
            if on {
                ptr::from_mut(id).cast()
            } else {
                ptr::null_mut()
            }
        };
        unsafe { swr_v1_hook_socket_bind(socket.state(), READ, co(bound & 1 != 0, reader)) };
        unsafe { swr_v1_hook_socket_bind(socket.state(), WRITE, co(bound & 2 != 0, writer)) };
    }

    /// Upstream's `get_event_str`, `get_bound_cid` and `check_bound_co`.
    fn upstream_bound(
        fd: c_int,
        bound: usize,
        [reader, writer]: [c_long; 2],
        event: u32,
    ) -> (c_long, &'static str, Vec<String>) {
        let (read, write) = (bound & 1 != 0, bound & 2 != 0);
        let name = if event == READ {
            "reading"
        } else if event == WRITE {
            "writing"
        } else if read && write {
            "reading or writing"
        } else if read {
            "reading"
        } else {
            "writing"
        };
        let co = if event & READ != 0 && read {
            Some(reader)
        } else if event & WRITE != 0 && write {
            Some(writer)
        } else {
            None
        };
        let mut effects = Vec::new();
        let cid = co.map_or(0, |cid| {
            effects.push(format!("cid:{cid}"));
            cid
        });
        if cid != 0 {
            effects.push(format!("bound-error:{fd}:{name}:{cid}"));
        }
        (cid, name, effects)
    }

    /// The socket policy gate's former scenarios 0..72: the bound coroutine, its id,
    /// the event's name and the bound report, for each binding, id sign and mask.
    #[test]
    fn ported_bound_matrix() {
        install();
        for k in 0..72 {
            let (bound, event) = ((k / 6) % 4, MASKS[k % 6]);
            let ids = [[0, 0], [5, 7], [-5, -7]][k / 24];
            let socket = TestSocket::new();
            let mut cells = ids;
            bind(&socket, bound, &mut cells);
            let (cid, name, effects) = upstream_bound(100, bound, ids, event);
            assert_eq!(
                unsafe { swr_v1_hook_co_socket_bound_cid(&raw const *socket.0, event) },
                cid
            );
            let got = unsafe {
                CStr::from_ptr(swr_v1_hook_co_socket_event_str(&raw const *socket.0, event))
            };
            assert_eq!(got.to_str().unwrap(), name, "scenario {k}");
            take();
            unsafe { swr_v1_hook_co_socket_check_bound(&raw const *socket.0, event) };
            assert_eq!(take(), effects, "scenario {k}");
        }
    }

    /// The socket policy gate's former scenarios 72..216: `is_available` over each
    /// binding, mask and socket state (open, closed, close-wait), with the bound
    /// report recorded or closing the socket. (Its third mode, a report that
    /// throws, has no counterpart: the real report is a fatal error.)
    #[test]
    fn ported_availability_matrix() {
        install();
        for k in 0..144 {
            let (event, bound, state, closes) =
                (MASKS[k % 6], (k / 6) % 4, (k / 24) % 3, k / 72 == 1);
            let mut socket = TestSocket::new();
            let mut ids = [5, 7];
            bind(&socket, bound, &mut ids);
            socket.0.sock_fd = if state == 1 { -1 } else { 100 };
            unsafe { socket.0.record() }.output.close_wait = u8::from(state == 2);
            let target = socket.ptr();
            if closes {
                ON_BOUND.set(Some(Box::new(move || unsafe { (*target).sock_fd = -1 })));
            }
            set_errno(0);
            take();
            let mut expected = Vec::new();
            let mut closed = state == 1;
            if event != 0 {
                let (_, _, effects) = upstream_bound(socket.0.sock_fd, bound, [5, 7], event);
                closed |= closes && effects.len() == 2;
                expected = effects;
            }
            let error = if closed {
                libc::EBADF
            } else if state == 2 {
                CLOSE_WAIT_ERROR
            } else {
                0
            };
            if error != 0 {
                expected.push(format!("last-error:{error}"));
            }
            let available = unsafe { swr_v1_hook_co_socket_available(target, event) };
            assert_eq!(
                (available, socket.0.err_code, errno()),
                (error == 0, error, error),
                "scenario {}",
                k + 72
            );
            assert_eq!(take(), expected, "scenario {}", k + 72);
            ON_BOUND.set(None);
            socket.0.sock_fd = 100;
        }
    }

    /// The socket policy gate's former scenarios 288..324: `check_return_value`
    /// over the result's extremes, a preset error and errno.
    #[test]
    fn ported_check_matrix() {
        install();
        for k in 0..36 {
            let result = [isize::MIN, -1, 0, isize::MAX][k % 4];
            let preset = [0, 9, -9][(k / 4) % 3];
            let system = [0, libc::EINVAL, libc::EAGAIN][k / 12];
            let mut socket = TestSocket::new();
            socket.0.err_code = preset;
            socket.0.err_msg = c"preset".as_ptr();
            set_errno(system);
            take();
            // Upstream: set_err(0) after a success, set_err(errno) if no error is set.
            let set = if result >= 0 {
                Some(0)
            } else if preset == 0 {
                Some(system)
            } else {
                None
            };
            unsafe { swr_v1_hook_co_socket_check(socket.ptr(), result) };
            let message = unsafe { CStr::from_ptr(socket.0.err_msg) }
                .to_str()
                .unwrap();
            let expected = set.map_or((preset, system, "preset", vec![]), |e| {
                (
                    e,
                    e,
                    if e == 0 { "" } else { "native-error" },
                    vec![format!("last-error:{e}")],
                )
            });
            assert_eq!(
                (socket.0.err_code, errno(), message, take()),
                expected,
                "scenario {}",
                k + 288
            );
        }
    }

    /// The socket policy gate's former scenarios 324..348: `set_timeout(timeval)`'s
    /// conversion, upstream's two divisions.
    #[test]
    #[expect(
        clippy::cast_precision_loss,
        reason = "upstream converts both fields to double"
    )]
    fn ported_timeval_conversion() {
        for seconds in [-1i64, 0, 1, 9_007_199_254_740_993] {
            for micros in [-1i64, 0, 1, 999_999, 1_000_000, i64::from(i32::MAX)] {
                let expected = seconds as f64 + micros as f64 / 1000.0 / 1000.0;
                let got = crate::swr_v1_hook_timeval(seconds as f64, micros as f64);
                assert_eq!(got.to_bits(), expected.to_bits(), "{seconds} {micros}");
            }
        }
    }

    /// The socket policy gate's former scenarios 348..420: `has_timedout` over each
    /// timeout, starting clock and start time, three calls half a second apart.
    #[test]
    fn ported_has_timedout_matrix() {
        install();
        let timeouts = [-1.0, 0.0, 0.001, 1.0, f64::NAN, f64::INFINITY];
        for k in 0..72 {
            let (timeout, start) = (timeouts[k % 6], [0.0, -0.0, 1.0, 2.0][(k / 6) % 4]);
            let mut now = 1.0 + f64::from(u32::try_from(k / 24).unwrap()) * 2.0;
            let mut socket = TestSocket::new();
            unsafe { socket.0.record() }.timeouts.read = -7.0;
            let (mut startup, mut model) = (start, start);
            for _ in 0..3 {
                take();
                // Upstream, transcribed.
                let (mut expired, mut set) = (false, None);
                if timeout > 0.0 {
                    if model == 0.0 {
                        model = now;
                    } else {
                        let used = now - model;
                        if timeout - used < 0.001 {
                            expired = true;
                        } else {
                            set = Some(timeout - used);
                        }
                    }
                }
                let before = unsafe { socket.0.record() }.timeouts.read;
                let got = unsafe {
                    swr_v1_hook_co_socket_timed_out(socket.ptr(), timeout, &raw mut startup, now, 4)
                };
                let after = unsafe { socket.0.record() }.timeouts.read;
                assert_eq!(
                    (got, startup.to_bits()),
                    (expired, model.to_bits()),
                    "scenario {}",
                    k + 348
                );
                assert_eq!(
                    after.to_bits(),
                    set.unwrap_or(before).to_bits(),
                    "scenario {}",
                    k + 348
                );
                assert_eq!(
                    take(),
                    if expired {
                        vec![format!("last-error:{}", libc::ETIMEDOUT)]
                    } else {
                        vec![]
                    }
                );
                now += 0.5;
            }
        }
    }

    #[test]
    fn check_clears_or_publishes_errno_unless_an_error_is_set() {
        install();
        let mut socket = TestSocket::new();
        set_errno(libc::EINVAL);
        unsafe { check(socket.ptr(), -1) };
        assert_eq!(socket.0.err_code, libc::EINVAL);
        assert_eq!(
            unsafe { core::ffi::CStr::from_ptr(socket.0.err_msg) },
            c"native-error"
        );
        set_errno(libc::EAGAIN);
        unsafe { check(socket.ptr(), -1) };
        assert_eq!(socket.0.err_code, libc::EINVAL);
        unsafe { check(socket.ptr(), 0) };
        assert_eq!((socket.0.err_code, errno()), (0, 0));
        assert_eq!(unsafe { *socket.0.err_msg }, 0);
        assert_eq!(take(), ["last-error:22", "last-error:0"]);
    }
}

/// Publishes the calling thread's current error without changing its errno or
/// last-error slot. Unlike `set_err(0)`, the zero message comes from strerror.
/// # Safety
/// socket is live and exclusively writable; `last_error` is the caller's TLS value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_co_socket_default_err(
    socket: *mut CoSocket,
    last_error: c_int,
) {
    let error = if last_error == 0 { errno() } else { last_error };
    // SAFETY: the caller lends a live writable socket.
    unsafe { (*socket).err_code = error };
    // SAFETY: installed strerror is a native leaf returning stable text.
    let message = unsafe { (host().strerror)(error) };
    // SAFETY: the socket remains live through the leaf operation.
    unsafe { (*socket).err_msg = message };
}

/// # Safety
/// socket is exclusively writable; message remains readable while published.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_co_socket_borrowed_err(
    socket: *mut CoSocket,
    error: c_int,
    message: *const c_char,
) {
    // SAFETY: the caller lends the live socket.
    unsafe { (*socket).err_code = error };
    set_errno(error);
    // SAFETY: the native TLS setter cannot access or destroy the socket.
    unsafe { (host().set_last_error)(error) };
    // SAFETY: the borrowed message outlives publication.
    unsafe { (*socket).err_msg = message };
}

/// Sets owned error text, publishing error/errno first and the message pointer
/// only after successful assignment. Native cache exceptions are contained and
/// returned as status, to be rethrown after this Rust frame returns.
/// # Safety
/// socket/extra are live and exclusive; bytes are readable for length; the
/// optional native cache only updates `std::string` and catches every exception.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_co_socket_owned_err(
    socket: *mut CoSocket,
    error: c_int,
    bytes: *const c_char,
    length: usize,
) -> c_int {
    // SAFETY: the caller lends a live writable socket.
    unsafe { (*socket).err_code = error };
    set_errno(error);
    // SAFETY: the installed native leaf writes only the calling thread's error.
    unsafe { (host().set_last_error)(error) };
    // SAFETY: the metadata owner outlives this synchronized call.
    let extra = unsafe { (*socket).extra };
    // SAFETY: its first stable slot owns the error string; take no Rust borrow.
    let slot = unsafe { &raw const (*extra).strings[0] };
    // SAFETY: readable bytes; the native cache contains all exceptions below Rust.
    let status = unsafe { swoole_rs_corebase::swr_v1_core_string_set(slot, bytes, length) };
    if status == 0 {
        // SAFETY: successful assignment leaves stable terminated bytes in the slot.
        let view = unsafe { swoole_rs_corebase::swr_v1_core_string_view(slot) };
        // SAFETY: publish only after the complete string assignment succeeds.
        unsafe { (*socket).err_msg = view.data };
    }
    status
}
