//! Each backend's whole wait loop in one call (#336, #391). Rust owns the loop and the readiness
//! order; native code supplies the kernel wait and the effects upstream reaches through `Reactor`.
use crate::SocketRecord;
use crate::handlers::{self, Handlers};
use std::ffi::{c_int, c_void};

pub const POLL: u32 = 0;
pub const EPOLL: u32 = 1;
pub const KQUEUE: u32 = 2;

/// Native effects for one `wait()`. Every function receives `context`.
#[repr(C)]
pub struct WaitOps {
    pub context: *mut c_void,
    pub backend: u32,
    /// The backend's result array: `pollfd`, `epoll_event` or `kevent` entries.
    pub events: *const c_void,
    /// poll only: the socket each `pollfd` entry was built from.
    pub sockets: *const *mut c_void,
    /// `Reactor::running` and `once`, read fresh on every use.
    pub running: *const bool,
    pub once: *const bool,
    /// The kernel wait. Returns its result; poll also stores how many entries it waited on.
    pub wait: unsafe extern "C" fn(*mut c_void, *mut c_int) -> c_int,
    pub before_wait: unsafe extern "C" fn(*mut c_void),
    pub begin: unsafe extern "C" fn(*mut c_void),
    pub catch_error: unsafe extern "C" fn(*mut c_void) -> c_int,
    pub end: unsafe extern "C" fn(*mut c_void, c_int),
    /// epoll with `io_uring` only: `interrupt` runs the EINTR handler when `interrupted`.
    pub interrupted: Option<unsafe extern "C" fn(*mut c_void) -> c_int>,
    pub interrupt: Option<unsafe extern "C" fn(*mut c_void)>,
    pub wait_failed: unsafe extern "C" fn(*mut c_void),
    pub reactor_id: i16,
    /// The reactor's handler tables; each handler runs as `handler(reactor, event)`.
    pub handlers: *const Handlers,
    pub reactor: *mut c_void,
    pub event: *mut Event,
    pub handler_failed: unsafe extern "C" fn(*mut c_void, u32),
    pub remove: unsafe extern "C" fn(*mut c_void, *mut c_void),
    /// kqueue only: dispatch a signal entry, and warn about an unknown filter.
    pub signal: Option<unsafe extern "C" fn(*mut c_void)>,
    pub unknown: Option<unsafe extern "C" fn(*mut c_void, c_int)>,
}

/// Upstream's `swoole::Event`.
#[repr(C)]
pub struct Event {
    pub fd: c_int,
    pub reactor_id: i16,
    pub fd_type: c_int,
    pub socket: *mut c_void,
}

const SW_EVENT_ONCE: c_int = 1 << 12;

const READ: u32 = 1;
const WRITE: u32 = 2;
const HANGUP: u32 = 4;

fn mask(read: bool, write: bool, hangup: bool) -> u32 {
    let mut mask = 0;
    if read {
        mask |= READ;
    }
    if write {
        mask |= WRITE;
    }
    if hangup {
        mask |= HANGUP;
    }
    mask
}

/// Upstream's `wait()` for `ops.backend`: 0 when the loop stops (and when poll fails uncaught),
/// -1 when epoll or kqueue fail uncaught.
/// # Safety
/// `ops` is live for the call: `events` (and poll's `sockets`) hold the entries the kernel wait
/// reports, the field pointers stay readable, and every function accepts `context` and the
/// sockets and signals stored in the entries.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_run(ops: *const WaitOps) -> i32 {
    // SAFETY: per the contract, `ops` is live for this call.
    let ops = unsafe { &*ops };
    let context = ops.context;
    // SAFETY: the native effect accepts `context`.
    unsafe { (ops.before_wait)(context) };
    // SAFETY: the field pointers stay readable for the call; handlers may change them.
    while unsafe { ops.running.read() } {
        // SAFETY: the native effect accepts `context`.
        unsafe { (ops.begin)(context) };
        let mut entries: c_int = 0;
        // SAFETY: the native kernel wait accepts `context` and a live entry count.
        let ready = unsafe { (ops.wait)(context, &raw mut entries) };
        match ready.cmp(&0) {
            core::cmp::Ordering::Less => {
                // SAFETY: the native effect accepts `context` and reads this thread's errno.
                if unsafe { (ops.catch_error)(context) } == 0 {
                    // SAFETY: the native effect accepts `context`.
                    unsafe { (ops.wait_failed)(context) };
                    return if ops.backend == POLL { 0 } else { -1 };
                }
                if let (Some(interrupted), Some(interrupt)) = (ops.interrupted, ops.interrupt) {
                    // SAFETY: both native effects accept `context`.
                    if unsafe { interrupted(context) } != 0 {
                        // SAFETY: the native effect accepts `context`.
                        unsafe { interrupt(context) };
                    }
                }
            }
            core::cmp::Ordering::Equal => {
                // SAFETY: the native effect accepts `context`.
                unsafe { (ops.end)(context, 1) };
                // SAFETY: the field pointer stays readable for the call.
                if unsafe { ops.once.read() } {
                    break;
                }
                continue;
            }
            core::cmp::Ordering::Greater => {
                let count = if ops.backend == POLL { entries } else { ready };
                for index in 0..count.unsigned_abs() as usize {
                    // SAFETY: the kernel filled the first `count` entries of `events`.
                    unsafe { entry(ops, index) };
                }
            }
        }
        // SAFETY: the native effect accepts `context`.
        unsafe { (ops.end)(context, 0) };
        // SAFETY: the field pointer stays readable for the call.
        if unsafe { ops.once.read() } {
            break;
        }
    }
    0
}

/// Decode and dispatch entry `index`.
/// # Safety
/// `index` is below the count the kernel wait reported.
unsafe fn entry(ops: &WaitOps, index: usize) {
    match ops.backend {
        POLL => {
            let pollfd = ops.events.cast::<libc::pollfd>().wrapping_add(index);
            // SAFETY: the caller checked `index` against poll's entry count.
            let pollfd = unsafe { pollfd.read() };
            if pollfd.revents == 0 {
                return;
            }
            let revents = pollfd.revents;
            let mask = mask(
                revents & libc::POLLIN != 0,
                revents & libc::POLLOUT != 0,
                revents & (libc::POLLHUP | libc::POLLERR) != 0,
            );
            let socket = ops.sockets.wrapping_add(index);
            // SAFETY: poll's sockets array has an entry for each pollfd entry.
            let socket = unsafe { socket.read() };
            ready(ops, socket, pollfd.fd, mask);
        }
        #[cfg(target_os = "linux")]
        EPOLL => {
            let event = ops.events.cast::<libc::epoll_event>().wrapping_add(index);
            // SAFETY: the caller checked `index` against the kernel's ready count.
            let event = unsafe { event.read_unaligned() };
            let events = event.events;
            let mask = mask(
                events & libc::EPOLLIN as u32 != 0,
                events & libc::EPOLLOUT as u32 != 0,
                events & (libc::EPOLLRDHUP | libc::EPOLLERR | libc::EPOLLHUP) as u32 != 0,
            );
            #[expect(
                clippy::cast_possible_truncation,
                reason = "the token is a pointer stored in u64"
            )]
            ready(ops, event.u64 as usize as *mut c_void, -1, mask);
        }
        #[cfg(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        ))]
        KQUEUE => {
            let event = ops.events.cast::<libc::kevent>().wrapping_add(index);
            // SAFETY: the caller checked `index` against the kernel's ready count.
            let event = unsafe { event.read() };
            kqueue(ops, event.udata.cast(), c_int::from(event.filter));
        }
        _ => {}
    }
}

/// poll and epoll: read, write, error (unless read or write already ran), then a oneshot removal.
fn ready(ops: &WaitOps, socket: *mut c_void, fd: c_int, mask: u32) {
    let context = ops.context;
    let fd_type = prepare(ops, socket, fd, mask & HANGUP != 0);
    let live = || registered(socket);
    for (kind, bit) in [(0, READ), (1, WRITE)] {
        if mask & bit != 0 && live() && handle(ops, kind, fd_type) < 0 {
            // SAFETY: the native effect accepts `context` and the prepared event.
            unsafe { (ops.handler_failed)(context, kind) };
        }
    }
    if mask & HANGUP != 0 && live() {
        // The read or write handler has already seen the error or hangup.
        if mask & (READ | WRITE) != 0 {
            return;
        }
        if handle(ops, 2, fd_type) < 0 {
            // SAFETY: the native effect accepts `context` and the prepared event.
            unsafe { (ops.handler_failed)(context, 2) };
        }
    }
    oneshot(ops, socket, live);
}

/// kqueue: one socket entry per filter, or a signal, as upstream's `ReactorKqueue::wait`.
#[cfg(any(
    target_os = "macos",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
fn kqueue(ops: &WaitOps, udata: *mut c_void, filter: c_int) {
    let context = ops.context;
    if udata.is_null() {
        return;
    }
    if filter == c_int::from(libc::EVFILT_READ) || filter == c_int::from(libc::EVFILT_WRITE) {
        let fd_type = prepare(ops, udata, -1, false);
        let live = || registered(udata);
        if !live() {
            return;
        }
        let kind = u32::from(filter != c_int::from(libc::EVFILT_READ));
        if handle(ops, kind, fd_type) < 0 {
            // SAFETY: the native effect accepts `context` and the prepared event.
            unsafe { (ops.handler_failed)(context, kind) };
        }
        oneshot(ops, udata, live);
    } else if filter == c_int::from(libc::EVFILT_SIGNAL) {
        if let Some(signal) = ops.signal {
            // SAFETY: `udata` is the registered native signal; the effect accepts it.
            unsafe { signal(udata) };
        }
    } else if let Some(unknown) = ops.unknown {
        // SAFETY: the native effect accepts `context`.
        unsafe { unknown(context, filter) };
    }
}

/// Runs the prepared event's read (0), write (1) or error (2) handler, as upstream's
/// `get_handler`/`get_error_handler`; a missing handler fails like one returning
/// `SW_ERR` rather than calling null.
fn handle(ops: &WaitOps, kind: u32, fd_type: c_int) -> c_int {
    let kind = match kind {
        0 => handlers::READ,
        1 => handlers::WRITE,
        _ => handlers::ERROR,
    };
    // SAFETY: the native reactor's tables stay live while it waits.
    let Some(handler) = (unsafe { &*ops.handlers }).dispatch(kind, fd_type) else {
        return -1;
    };
    // SAFETY: a native `ReactorHandler`, given its reactor and the prepared event.
    unsafe { handler(ops.reactor, ops.event.cast()) }
}

/// Fills the event for a ready socket, as each upstream backend did: `fd` is poll's entry
/// fd (-1: the socket's own), and `hangup` marks `event_hup`. Returns the fd type.
fn prepare(ops: &WaitOps, socket: *mut c_void, fd: c_int, hangup: bool) -> c_int {
    let record = socket.cast::<SocketRecord>();
    // SAFETY: `socket` is a registered socket record; the loop reads single fields.
    let fd_type = unsafe { (*record).fd_type };
    let fd = if fd < 0 {
        // SAFETY: as above.
        unsafe { (*record).fd }
    } else {
        fd
    };
    let event = Event {
        fd,
        reactor_id: ops.reactor_id,
        fd_type,
        socket,
    };
    // SAFETY: `ops.event` is the backend's own event, not otherwise borrowed now.
    unsafe { ops.event.write(event) };
    if hangup {
        // SAFETY: as above.
        unsafe { (*record).event_hup = 1 };
    }
    fd_type
}

/// Whether `socket` is still registered (`!socket->removed`).
fn registered(socket: *mut c_void) -> bool {
    // SAFETY: `socket` stays live while its handlers run.
    unsafe { (*socket.cast::<SocketRecord>()).removed == 0 }
}

fn oneshot(ops: &WaitOps, socket: *mut c_void, live: impl Fn() -> bool) {
    // SAFETY: `socket` stays live until its removal below.
    if live() && unsafe { (*socket.cast::<SocketRecord>()).events } & SW_EVENT_ONCE != 0 {
        // SAFETY: the native effect accepts `context` and the socket.
        unsafe { (ops.remove)(ops.context, socket) };
    }
}
