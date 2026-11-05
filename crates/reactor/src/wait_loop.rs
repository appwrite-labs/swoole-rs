//! `Reactor::wait()` for every backend in one call: upstream's `ReactorEpoll`, `ReactorKqueue`
//! and `ReactorPoll::wait` over the Rust reactor state (#336, #391). Rust owns the loop, the
//! kernel wait and the readiness order; native code supplies the effects upstream reaches
//! through `Reactor`'s callbacks, and the warnings.
use crate::SocketRecord;
use crate::handlers::{self, Handlers};
use crate::reactor_state::{ReactorState, before_wait, kernel_wait, swr_v1_reactor_state_backend};
use std::ffi::{c_int, c_void};

pub const POLL: u32 = 0;
pub const EPOLL: u32 = 1;
pub const KQUEUE: u32 = 2;

/// Native effects for one `wait()`. Every function receives `reactor`.
#[repr(C)]
pub struct WaitOps {
    /// The native `Reactor`; each handler runs as `handler(reactor, event)`.
    pub reactor: *mut c_void,
    /// `Reactor::defer_tasks`, read fresh before each kernel wait: non-null waits without blocking.
    pub deferred: *const *const c_void,
    pub begin: unsafe extern "C" fn(*mut c_void),
    pub end: unsafe extern "C" fn(*mut c_void, c_int),
    /// epoll with `io_uring` only: `interrupt` runs the EINTR handler when `interrupted`.
    pub interrupted: Option<unsafe extern "C" fn(*mut c_void) -> c_int>,
    pub interrupt: Option<unsafe extern "C" fn(*mut c_void)>,
    /// Warns about an uncaught kernel wait failure; `entries` is poll's entry count.
    pub wait_failed: unsafe extern "C" fn(*mut c_void, c_int),
    /// Warns that the read (0), write (1) or error (2) handler failed for `event`.
    pub handler_failed: unsafe extern "C" fn(*mut c_void, u32, *const Event),
    /// The oneshot removal of a socket.
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

/// One `wait()`: its native effects and what it reads from the state.
struct Loop<'a> {
    ops: &'a WaitOps,
    backend: u32,
    /// The backend's result array: `pollfd`, `epoll_event` or `kevent` entries.
    events: *const c_void,
    /// poll only: the socket each `pollfd` entry was built from.
    sockets: *const *mut c_void,
    reactor_id: i16,
    handlers: *const Handlers,
    event: *mut Event,
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

/// Upstream's `wait()` for the state's backend: 0 when the loop stops (and when poll fails
/// uncaught), -1 when epoll or kqueue fail uncaught.
/// # Safety
/// `owner` is a live state with an open backend and `ops` is live for the call. No reference to
/// the state crosses a native effect, so effects may change it through its native views.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_wait(owner: *mut ReactorState, ops: *const WaitOps) -> i32 {
    // SAFETY: per the contract, `ops` is live for this call.
    let ops = unsafe { &*ops };
    let reactor = ops.reactor;
    // SAFETY: the state is live; the backend's arrays stay put until the state closes it.
    let view = unsafe { swr_v1_reactor_state_backend(owner) };
    // SAFETY: the state is live; its fields and handler tables live as long as it does.
    let fields = unsafe { &raw const (*owner).fields };
    // SAFETY: as above.
    let handlers = unsafe { &raw const (*owner).handlers };
    // SAFETY: as above.
    let running = unsafe { &raw const (*fields).running };
    // SAFETY: as above.
    let once = unsafe { &raw const (*fields).once };
    // SAFETY: a single field read of the live state.
    let id = unsafe { (*fields).id };
    let mut event = Event {
        fd: 0,
        reactor_id: 0,
        fd_type: 0,
        socket: std::ptr::null_mut(),
    };
    let run = Loop {
        ops,
        backend: view.backend,
        events: view.events,
        sockets: view.sockets,
        #[expect(
            clippy::cast_possible_wrap,
            reason = "upstream's static_cast<int16_t>(reactor_->id)"
        )]
        reactor_id: id as i16,
        handlers,
        event: &raw mut event,
    };
    // SAFETY: no native effect is active.
    unsafe { before_wait(owner) };
    // SAFETY: single field reads; effects may change the field between them.
    while unsafe { running.read() } {
        // SAFETY: the native effect accepts `reactor`.
        unsafe { (ops.begin)(reactor) };
        let mut entries: c_int = 0;
        // SAFETY: the native slot is live for the call.
        let deferred = !unsafe { ops.deferred.read() }.is_null();
        // SAFETY: the kernel wait runs no native effect.
        let ready = unsafe { kernel_wait(owner, deferred, &raw mut entries) };
        match ready.cmp(&0) {
            core::cmp::Ordering::Less => {
                // upstream's Reactor::catch_error()
                if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
                    // SAFETY: the native effect accepts `reactor` and reads this thread's errno.
                    unsafe { (ops.wait_failed)(reactor, entries) };
                    return if run.backend == POLL { 0 } else { -1 };
                }
                if let (EPOLL, Some(interrupted), Some(interrupt)) =
                    (run.backend, ops.interrupted, ops.interrupt)
                {
                    // SAFETY: both native effects accept `reactor`.
                    if unsafe { interrupted(reactor) } != 0 {
                        // SAFETY: the native effect accepts `reactor`.
                        unsafe { interrupt(reactor) };
                    }
                }
            }
            core::cmp::Ordering::Equal => {
                // SAFETY: the native effect accepts `reactor`.
                unsafe { (ops.end)(reactor, 1) };
                // SAFETY: a single field read.
                if unsafe { once.read() } {
                    break;
                }
                continue;
            }
            core::cmp::Ordering::Greater => {
                let count = if run.backend == POLL { entries } else { ready };
                for index in 0..count.unsigned_abs() as usize {
                    // SAFETY: the kernel filled the first `count` entries of `events`.
                    unsafe { entry(&run, index) };
                }
            }
        }
        // SAFETY: the native effect accepts `reactor`.
        unsafe { (ops.end)(reactor, 0) };
        // SAFETY: a single field read.
        if unsafe { once.read() } {
            break;
        }
    }
    0
}

/// Decode and dispatch entry `index`.
/// # Safety
/// `index` is below the count the kernel wait reported.
unsafe fn entry(ops: &Loop<'_>, index: usize) {
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
fn ready(ops: &Loop<'_>, socket: *mut c_void, fd: c_int, mask: u32) {
    let reactor = ops.ops.reactor;
    let fd_type = prepare(ops, socket, fd, mask & HANGUP != 0);
    let live = || registered(socket);
    for (kind, bit) in [(0, READ), (1, WRITE)] {
        if mask & bit != 0 && live() && handle(ops, kind, fd_type) < 0 {
            // SAFETY: the native effect accepts `reactor` and the prepared event.
            unsafe { (ops.ops.handler_failed)(reactor, kind, ops.event) };
        }
    }
    if mask & HANGUP != 0 && live() {
        // The read or write handler has already seen the error or hangup.
        if mask & (READ | WRITE) != 0 {
            return;
        }
        if handle(ops, 2, fd_type) < 0 {
            // SAFETY: the native effect accepts `reactor` and the prepared event.
            unsafe { (ops.ops.handler_failed)(reactor, 2, ops.event) };
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
fn kqueue(ops: &Loop<'_>, udata: *mut c_void, filter: c_int) {
    let reactor = ops.ops.reactor;
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
            // SAFETY: the native effect accepts `reactor` and the prepared event.
            unsafe { (ops.ops.handler_failed)(reactor, kind, ops.event) };
        }
        oneshot(ops, udata, live);
    } else if filter == c_int::from(libc::EVFILT_SIGNAL) {
        if let Some(signal) = ops.ops.signal {
            // SAFETY: `udata` is the registered native signal; the effect accepts it.
            unsafe { signal(udata) };
        }
    } else if let Some(unknown) = ops.ops.unknown {
        // SAFETY: the native effect accepts `reactor`.
        unsafe { unknown(reactor, filter) };
    }
}

/// Runs the prepared event's read (0), write (1) or error (2) handler, as upstream's
/// `get_handler`/`get_error_handler`; a missing handler fails like one returning
/// `SW_ERR` rather than calling null.
fn handle(ops: &Loop<'_>, kind: u32, fd_type: c_int) -> c_int {
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
    unsafe { handler(ops.ops.reactor, ops.event.cast()) }
}

/// Fills the event for a ready socket, as each upstream backend did: `fd` is poll's entry
/// fd (-1: the socket's own), and `hangup` marks `event_hup`. Returns the fd type.
fn prepare(ops: &Loop<'_>, socket: *mut c_void, fd: c_int, hangup: bool) -> c_int {
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

fn oneshot(ops: &Loop<'_>, socket: *mut c_void, live: impl Fn() -> bool) {
    // SAFETY: `socket` stays live until its removal below.
    if live() && unsafe { (*socket.cast::<SocketRecord>()).events } & SW_EVENT_ONCE != 0 {
        // SAFETY: the native effect accepts `reactor` and the socket.
        unsafe { (ops.ops.remove)(ops.ops.reactor, socket) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reactor_state::{
        swr_v1_reactor_state_descriptor, swr_v1_reactor_state_free, swr_v1_reactor_state_new,
        swr_v1_reactor_state_open, swr_v1_reactor_state_socket,
    };
    use std::cell::RefCell;

    thread_local! {
        static EFFECTS: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
    }

    struct Probe {
        owner: *mut ReactorState,
        running: *mut bool,
        begin_stops: bool,
        cycles: u32,
    }

    fn effect(name: &'static str) {
        EFFECTS.with(|effects| effects.borrow_mut().push(name));
    }

    unsafe extern "C" fn begin(reactor: *mut c_void) {
        effect("begin");
        let probe = unsafe { &mut *reactor.cast::<Probe>() };
        if probe.begin_stops {
            unsafe { probe.running.write(false) };
        }
    }

    unsafe extern "C" fn end(reactor: *mut c_void, timed_out: c_int) {
        effect(if timed_out != 0 { "end-timeout" } else { "end" });
        let probe = unsafe { &mut *reactor.cast::<Probe>() };
        probe.cycles += 1;
        if probe.cycles == 2 {
            unsafe { probe.running.write(false) };
        }
    }

    unsafe extern "C" fn read(_: *mut c_void, event: *mut c_void) -> c_int {
        effect("read");
        let mut byte = 0u8;
        let fd = unsafe { (*event.cast::<Event>()).fd };
        assert_eq!(unsafe { libc::read(fd, (&raw mut byte).cast(), 1) }, 1);
        0
    }

    unsafe extern "C" fn remove(reactor: *mut c_void, socket: *mut c_void) {
        effect("remove");
        let probe = unsafe { &*reactor.cast::<Probe>() };
        unsafe { swr_v1_reactor_state_socket(probe.owner, socket.cast(), 2, 0) };
    }

    unsafe extern "C" fn unexpected(_: *mut c_void, _: c_int) {
        panic!("the kernel wait failed or reported an unknown entry");
    }

    unsafe extern "C" fn handler_failed(_: *mut c_void, _: u32, _: *const Event) {
        panic!("a handler failed");
    }

    /// The pinned loops' before/begin/wait/dispatch/end frame, `once`, `running` and the
    /// deferred zero timeout, over the actual kernel wait of poll and the native backend.
    #[test]
    fn whole_wait_runs_upstreams_frame_on_each_backend() {
        #[cfg(target_os = "linux")]
        let native_kind = 1;
        #[cfg(not(target_os = "linux"))]
        let native_kind = 2;
        for kind in [3, native_kind] {
            for once in [false, true] {
                for begin_stops in [false, true] {
                    for socket_once in [false, true] {
                        for deferred in [false, true] {
                            // SAFETY: each run owns its state, pipe and socket.
                            let effects =
                                unsafe { run(kind, [once, begin_stops, socket_once, deferred]) };
                            let mut expected = vec!["begin", "read"];
                            if socket_once {
                                expected.push("remove");
                            }
                            expected.push("end");
                            if !once && !begin_stops {
                                expected.extend(["begin", "end-timeout"]);
                            }
                            assert_eq!(
                                effects, expected,
                                "kind {kind} once {once} begin_stops {begin_stops} socket_once {socket_once} deferred {deferred}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// `flags`: the reactor's `once`, a begin callback that stops it, a ONCE socket, deferred work.
    unsafe fn run(kind: c_int, flags: [bool; 4]) -> Vec<&'static str> {
        let [once, begin_stops, socket_once, deferred] = flags;
        EFFECTS.with(|effects| effects.borrow_mut().clear());
        let state = swr_v1_reactor_state_new();
        assert_eq!(
            unsafe { swr_v1_reactor_state_open(state.owner, kind, 4) },
            0
        );
        let mut fds = [0; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let events = 512 | if socket_once { SW_EVENT_ONCE } else { 0 };
        let mut socket = SocketRecord::with_fd(fds[0], 5);
        let token: *mut c_void = (&raw mut socket).cast();
        unsafe {
            assert_eq!(
                swr_v1_reactor_state_socket(state.owner, &raw mut socket, 0, events),
                0
            );
            assert!(crate::swr_v1_reactor_handlers_set(
                state.handlers,
                5,
                512,
                Some(read)
            ));
            let descriptor = swr_v1_reactor_state_descriptor(state.owner);
            #[cfg(target_os = "linux")]
            if kind == 1 {
                assert_eq!(
                    crate::swr_v1_reactor_epoll_ctl(descriptor, 0, fds[0], token, events),
                    0
                );
            }
            #[cfg(not(target_os = "linux"))]
            if kind == 2 {
                assert_eq!(
                    crate::swr_v1_reactor_kqueue_add(descriptor, fds[0], token, events),
                    0
                );
            }
            assert_eq!(libc::write(fds[1], b"a".as_ptr().cast(), 1), 1);
            // Without deferred work the second wait times out after 1ms; with it, at once.
            (*state.fields).timeout_msec = if deferred { -1 } else { 1 };
            (*state.fields).once = once;
        }
        let mut probe = Probe {
            owner: state.owner,
            // SAFETY: the state's own field slot.
            running: unsafe { &raw mut (*state.fields).running },
            begin_stops,
            cycles: 0,
        };
        let task = 1u8;
        let pending: *const c_void = if deferred {
            (&raw const task).cast()
        } else {
            std::ptr::null()
        };
        let ops = WaitOps {
            reactor: (&raw mut probe).cast(),
            deferred: &raw const pending,
            begin,
            end,
            interrupted: None,
            interrupt: None,
            wait_failed: unexpected,
            handler_failed,
            remove,
            signal: None,
            unknown: Some(unexpected),
        };
        assert_eq!(
            unsafe { swr_v1_reactor_wait(state.owner, &raw const ops) },
            0
        );
        unsafe {
            assert!((*state.fields).start);
            swr_v1_reactor_state_free(state.owner);
            libc::close(fds[0]);
            libc::close(fds[1]);
        }
        EFFECTS.with(|effects| effects.borrow().clone())
    }
}
