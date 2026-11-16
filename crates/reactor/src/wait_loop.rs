//! `Reactor::wait()` for every backend in one call: upstream's `ReactorEpoll`, `ReactorKqueue`
//! and `ReactorPoll::wait` over the Rust reactor state (#336, #391). Rust owns the loop, the
//! kernel wait, the readiness order and the warnings; native code supplies the effects
//! upstream reaches through `Reactor`'s callbacks.
use crate::Socket;
use crate::handlers::{self, Table};
use crate::reactor_state::{Entry, before_wait, kernel_wait};
use std::ffi::c_int;
use std::sync::OnceLock;
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::log;

/// Which kernel wait a reactor's backend runs.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum BackendKind {
    Poll,
    Epoll,
    Kqueue,
}

/// The effects of one `wait()`. Every function receives `reactor`.
pub(crate) struct WaitOps<'a> {
    /// The `Reactor` that waits; each handler runs as `handler(reactor, event)`.
    pub(crate) reactor: handlers::Running<'a>,
    pub(crate) begin: fn(handlers::Running<'_>),
    pub(crate) end: fn(handlers::Running<'_>, bool),
}

/// The `io_uring` engine's operations this crate calls. crates/hooks, which sits
/// above it, registers them with [`register_uring`] when the build has the engine.
pub struct UringOps {
    /// The epoll wait failed with EINTR on a thread whose engine is running.
    pub interrupted: fn() -> bool,
    /// Harvest the engine's completions after that interruption.
    pub interrupt: fn(),
    /// `Iouring::waitpid`.
    pub waitpid: fn(libc::pid_t, Option<&mut c_int>, c_int, f64) -> libc::pid_t,
}

static URING: OnceLock<&'static UringOps> = OnceLock::new();

/// Make `ops` the engine this crate calls: `swoole_init` registers it once.
pub fn register_uring(ops: &'static UringOps) {
    URING.get_or_init(|| ops);
}

/// The registered `io_uring` engine, if the build has one.
pub(crate) fn uring() -> Option<&'static UringOps> {
    URING.get().copied()
}

/// Upstream's `swoole::Event`: what a handler is called with. Its socket is a record
/// live until an unsafe free whose caller answers for the events left, as any
/// `Socket<'static>` is: C++ that hands one over vouches for it by the field's type.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Event {
    fd: c_int,
    reactor_id: i16,
    fd_type: c_int,
    socket: Option<Socket<'static>>,
}

impl Event {
    /// An event over `socket`.
    #[must_use]
    pub const fn of(
        fd: c_int,
        reactor_id: i16,
        fd_type: c_int,
        socket: Option<Socket<'static>>,
    ) -> Self {
        Self {
            fd,
            reactor_id,
            fd_type,
            socket,
        }
    }

    /// An event with no socket, which the wait loop fills.
    pub(crate) const fn empty() -> Self {
        Self::of(0, 0, 0, None)
    }

    #[must_use]
    pub const fn fd(&self) -> c_int {
        self.fd
    }

    #[must_use]
    pub const fn reactor_id(&self) -> i16 {
        self.reactor_id
    }

    #[must_use]
    pub const fn fd_type(&self) -> c_int {
        self.fd_type
    }

    /// The event's socket, if it has one.
    #[must_use]
    pub const fn socket(&self) -> Option<Socket<'static>> {
        self.socket
    }
}

/// One `wait()`: its native effects and what it reads from the state.
struct Loop<'a> {
    backend: BackendKind,
    reactor_id: i16,
    /// The reactor, for its handler tables and the handlers' own argument.
    reactor: handlers::Running<'a>,
    /// The event the loop prepared for the ready socket's handlers.
    event: std::cell::Cell<Event>,
    /// How many of the backend's entries the last kernel wait filled.
    ready: std::cell::Cell<usize>,
}

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

/// Upstream's `wait()` for the state's backend: true when the loop stops (and when poll
/// fails uncaught), false when epoll or kqueue fail uncaught. No reference to the state
/// crosses an effect, so effects may change it through its native views.
#[must_use]
pub(crate) fn wait<'a>(ops: &'a WaitOps<'a>) -> bool {
    let reactor = ops.reactor;
    // No native effect is active, and this borrow ends before the first one.
    let (backend, id) = reactor.with(|state| {
        before_wait(&mut state.fields);
        (state.backend_kind(), state.fields.id)
    });
    // Single field reads: effects may change either field between them.
    let once = || reactor.with(|state| state.fields.once);
    let run = Loop {
        backend,
        reactor_id: id.c_cast::<i16>(),
        reactor,
        event: std::cell::Cell::new(Event::empty()),
        ready: std::cell::Cell::new(0),
    };
    while reactor.with(|state| state.fields.running) {
        (ops.begin)(reactor);
        let mut entries = 0;
        // Deferred work waits without blocking.
        match kernel_wait(reactor, reactor.has_deferred(), &mut entries) {
            Err(error) => {
                // upstream's Reactor::catch_error()
                if error.raw_os_error() != Some(libc::EINTR) {
                    wait_failed(&run, entries);
                    return run.backend == BackendKind::Poll;
                }
                if run.backend == BackendKind::Epoll
                    && let Some(uring) = uring()
                    && (uring.interrupted)()
                {
                    (uring.interrupt)();
                }
            }
            Ok(0) => {
                (ops.end)(reactor, true);
                if once() {
                    break;
                }
                continue;
            }
            Ok(ready) => {
                let count = if run.backend == BackendKind::Poll {
                    entries
                } else {
                    ready
                };
                // The kernel filled the first `count` entries of `events`.
                run.ready.set(count);
                for index in 0..run.ready.get() {
                    entry(&run, index);
                }
            }
        }
        (ops.end)(reactor, false);
        if once() {
            break;
        }
    }
    true
}

/// Decode and dispatch entry `index`, if the kernel wait filled it.
fn entry(ops: &Loop<'_>, index: usize) {
    if index >= ops.ready.get() {
        return;
    }
    // A copy of the entry, read in the backend's own array: the borrow ends before
    // any handler runs.
    let Some(entry) = ops.reactor.with(|state| state.entry(index)) else {
        return;
    };
    match entry {
        Entry::Poll(pollfd, socket) => {
            let revents = pollfd.revents;
            if revents == 0 {
                return;
            }
            let mask = mask(
                revents & libc::POLLIN != 0,
                revents & libc::POLLOUT != 0,
                revents & (libc::POLLHUP | libc::POLLERR) != 0,
            );
            ready(ops, socket, pollfd.fd, mask);
        }
        #[cfg(target_os = "linux")]
        Entry::Epoll(event) => {
            let events = event.events;
            let mask = mask(
                events & libc::EPOLLIN as u32 != 0,
                events & libc::EPOLLOUT as u32 != 0,
                events & (libc::EPOLLRDHUP | libc::EPOLLERR | libc::EPOLLHUP) as u32 != 0,
            );
            // The token is a pointer stored in a u64.
            let token = event.u64.c_cast::<usize>() as *mut crate::SocketRecord;
            // SAFETY: epoll's token is a registered socket: live until an unsafe free,
            // which answers for the registration and the events it leaves.
            if let Some(socket) = unsafe { Socket::from_ptr(token) } {
                ready(ops, socket, -1, mask);
            }
        }
        #[cfg(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        ))]
        Entry::Kqueue(event) => kqueue(ops, &event),
    }
}

/// poll and epoll: read, write, error (unless read or write already ran), then a oneshot removal.
fn ready(ops: &Loop<'_>, socket: Socket<'static>, fd: c_int, mask: u32) {
    let fd_type = prepare(ops, socket, fd, mask & HANGUP != 0);
    let live = || registered(socket);
    for (kind, bit) in [(Table::Read, READ), (Table::Write, WRITE)] {
        if mask & bit != 0 && live() && !handle(ops, kind, fd_type) {
            handler_failed(ops, kind);
        }
    }
    if mask & HANGUP != 0 && live() {
        // The read or write handler has already seen the error or hangup.
        if mask & (READ | WRITE) != 0 {
            return;
        }
        if !handle(ops, Table::Error, fd_type) {
            handler_failed(ops, Table::Error);
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
fn kqueue(ops: &Loop<'_>, event: &libc::kevent) {
    let (udata, filter) = (
        event.udata.cast::<crate::SocketRecord>(),
        c_int::from(event.filter),
    );
    if udata.is_null() {
        return;
    }
    if filter == c_int::from(libc::EVFILT_READ) || filter == c_int::from(libc::EVFILT_WRITE) {
        // SAFETY: a read or write entry's udata is its registered socket: live until an
        // unsafe free, which answers for the registration and the events it leaves.
        let Some(socket) = (unsafe { Socket::from_ptr(udata) }) else {
            return;
        };
        let fd_type = prepare(ops, socket, -1, false);
        let live = || registered(socket);
        if !live() {
            return;
        }
        let kind = if filter == c_int::from(libc::EVFILT_READ) {
            Table::Read
        } else {
            Table::Write
        };
        if !handle(ops, kind, fd_type) {
            handler_failed(ops, kind);
        }
        oneshot(ops, socket, live);
    } else if filter == c_int::from(libc::EVFILT_SIGNAL) {
        // A signal entry's ident is its signal number, whose table slot is its udata.
        dispatch_signal(c_int::try_from(event.ident).unwrap_or(0));
    } else {
        log::error_log(
            LOG_WARNING,
            EVENT_UNKNOWN_DATA,
            c"ReactorKqueue::wait",
            format_args!("unknown event filter[{filter}]"),
        );
    }
}

/// kqueue delivers signals and unknown filters through the reactor.
#[cfg(any(
    target_os = "macos",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
mod kqueue_signal {
    use super::{c_int, log};

    /// `SW_LOG_WARNING`, `SW_ERROR_UNREGISTERED_SIGNAL` and `SW_ERROR_EVENT_UNKNOWN_DATA`.
    pub(super) use swoole_rs_corebase::log::LogLevel::SW_LOG_WARNING as LOG_WARNING;
    const UNREGISTERED_SIGNAL: c_int =
        swoole_rs_error::ErrorCode::SW_ERROR_UNREGISTERED_SIGNAL as c_int;
    pub(super) const EVENT_UNKNOWN_DATA: c_int =
        swoole_rs_error::ErrorCode::SW_ERROR_EVENT_UNKNOWN_DATA as c_int;

    /// `ReactorKqueue::wait`'s signal entry for `signo`: its registration is read before
    /// its handler, which may register another, runs.
    pub(super) fn dispatch_signal(signo: c_int) {
        dispatch(signo, swoole_rs_process::signal::signal_activated(signo));
    }

    /// An activated signal (`Some` handler) runs its handler unless that is `SIG_IGN`,
    /// and warns when it has none.
    pub(super) fn dispatch(signo: c_int, activated: Option<swoole_rs_process::signal::Handler>) {
        let Some(handler) = activated else {
            return;
        };
        match handler {
            swoole_rs_process::signal::Handler::NONE => {
                let name = swoole_rs_process::signal::signal_name(signo);
                let name = name.as_cstr();
                log::error_log(
                    LOG_WARNING,
                    UNREGISTERED_SIGNAL,
                    c"ReactorKqueue::wait",
                    format_args!(
                        "Unable to find callback function for signal {}",
                        name.to_string_lossy()
                    ),
                );
            }
            handler => handler.call(signo),
        }
    }
}
#[cfg(any(
    target_os = "macos",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
use kqueue_signal::{EVENT_UNKNOWN_DATA, LOG_WARNING, dispatch_signal};

/// Warns about an uncaught kernel wait failure (errno is the kernel's); `entries` is poll's.
fn wait_failed(ops: &Loop<'_>, entries: usize) {
    let (id, epfd, max_event_num, timeout) = ops.reactor.with(|state| {
        let fields = &state.fields;
        // `Reactor::get_timeout_msec()`.
        let timeout = if fields.defer_tasks.is_none() {
            fields.timeout_msec
        } else {
            0
        };
        (fields.id, state.backend_fd, fields.max_event_num, timeout)
    });
    match ops.backend {
        BackendKind::Epoll => log::sys_warning(
            c"ReactorEpoll::wait",
            format_args!(
                "[Reactor#{id}] epoll_wait(epfd={epfd}, max_events={max_event_num}, timeout={timeout}) failed"
            ),
        ),
        BackendKind::Kqueue => log::sys_warning(
            c"ReactorKqueue::wait",
            format_args!("kevent(epfd={epfd}) failed "),
        ),
        BackendKind::Poll => log::sys_warning(
            c"ReactorPoll::wait",
            format_args!("[Reactor#{id}] poll(nfds={entries}, timeout={timeout}) failed"),
        ),
    }
}

/// Warns that the read, write or error handler failed for the prepared event.
fn handler_failed(ops: &Loop<'_>, kind: Table) {
    let event = ops.event.get();
    let fd = event.fd;
    let name = |names: [&'static str; 3]| names.get(kind as usize).copied().unwrap_or_default();
    match ops.backend {
        BackendKind::Epoll => {
            let name = name(["EPOLLIN", "EPOLLOUT", "EPOLLERR"]);
            log::sys_warning(
                c"ReactorEpoll::wait",
                format_args!("{name} handle failed [fd={fd}, type={}]", event.fd_type),
            );
            return;
        }
        BackendKind::Kqueue => {
            let name = if kind == Table::Read { "read" } else { "write" };
            log::sys_warning(
                c"ReactorKqueue::wait",
                format_args!("kqueue event {name} socket#{fd} handler failed"),
            );
        }
        BackendKind::Poll => {
            let name = name(["POLLIN", "POLLOUT", "POLLERR"]);
            log::sys_warning(
                c"ReactorPoll::wait",
                format_args!("{name} handle failed. fd={fd}"),
            );
        }
    }
    swoole_rs_corebase::swoole_print_backtrace_on_error();
}

/// Runs the prepared event's read, write or error handler, as upstream's
/// `get_handler`/`get_error_handler`: false when it failed. A missing handler fails
/// like one returning `SW_ERR` rather than calling null.
fn handle(ops: &Loop<'_>, kind: Table, fd_type: c_int) -> bool {
    // The lookup's borrow of the tables ends here, before the handler runs.
    let Some(handler) = ops
        .reactor
        .handlers(|handlers| handlers.dispatch(kind, fd_type))
    else {
        return false;
    };
    // The handler may change the event, which the next handler of this socket sees; a
    // nested wait prepares its own.
    let mut event = ops.event.get();
    let result = handler(ops.reactor, &mut event);
    ops.event.set(event);
    result >= 0
}

/// Fills the event for a ready socket, as each upstream backend did: `fd` is poll's entry
/// fd (-1: the socket's own), and `hangup` marks `event_hup`. Returns the fd type.
fn prepare(ops: &Loop<'_>, socket: Socket<'static>, fd: c_int, hangup: bool) -> c_int {
    let (fd_type, own) = socket.read(|record| (record.fd_type, record.fd));
    let event = Event::of(
        if fd < 0 { own } else { fd },
        ops.reactor_id,
        fd_type,
        Some(socket),
    );
    ops.event.set(event);
    if hangup {
        socket.edit(|record| record.event_hup = true);
    }
    fd_type
}

/// Whether `socket` is still registered (`!socket->removed`).
fn registered(socket: Socket<'_>) -> bool {
    socket.read(|record| !record.removed)
}

fn oneshot(ops: &Loop<'_>, socket: Socket<'_>, live: impl Fn() -> bool) {
    if live()
        && socket
            .read(|record| record.events)
            .intersects(crate::Events::ONCE)
    {
        if ops.backend == BackendKind::Epoll {
            // epoll's oneshot entry is already disarmed, so only the registry forgets it.
            ops.reactor.with(|state| {
                state.record(crate::change::Request::Del(socket), crate::Events::NONE)
            });
        } else {
            // poll and kqueue run `Reactor::del`, which logs a failure.
            _ = crate::reactor_upstream::reactor_del(ops.reactor, socket);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reactor_state::tests::running;
    use std::cell::RefCell;

    thread_local! {
        static EFFECTS: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
        /// Whether begin stops the loop, and the end callbacks run so far.
        static PROBE: std::cell::Cell<(bool, u32)> = const { std::cell::Cell::new((false, 0)) };
    }

    fn effect(name: &'static str) {
        EFFECTS.with(|effects| effects.borrow_mut().push(name));
    }

    fn stop(reactor: crate::Running<'_>) {
        unsafe { (*reactor.state()).fields.running = false };
    }

    fn begin(reactor: crate::Running<'_>) {
        effect("begin");
        if PROBE.get().0 {
            stop(reactor);
        }
    }

    fn end(reactor: crate::Running<'_>, timed_out: bool) {
        effect(if timed_out { "end-timeout" } else { "end" });
        let (begin_stops, cycles) = PROBE.get();
        PROBE.set((begin_stops, cycles + 1));
        if cycles + 1 == 2 {
            stop(reactor);
        }
    }

    extern "C" fn read(_: crate::Running<'_>, event: &mut Event) -> c_int {
        effect("read");
        let mut byte = 0u8;
        assert_eq!(
            unsafe { libc::read(event.fd, (&raw mut byte).cast(), 1) },
            1
        );
        0
    }

    /// The pinned loops' before/begin/wait/dispatch/end frame, `once`, `running` and the
    /// deferred zero timeout, over the actual kernel wait of poll and the native backend.
    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll_create")]
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
                            let (effects, removed) =
                                unsafe { run(kind, [once, begin_stops, socket_once, deferred]) };
                            assert_eq!(removed, socket_once, "the oneshot socket is removed");
                            let mut expected = vec!["begin", "read", "end"];
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
    unsafe fn run(kind: c_int, flags: [bool; 4]) -> (Vec<&'static str>, bool) {
        let [once, begin_stops, socket_once, deferred] = flags;
        EFFECTS.with(|effects| effects.borrow_mut().clear());
        let state = crate::reactor_state::tests::state(kind, 4);
        let mut fds = [0; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let events = crate::Events::from_bits(512)
            | if socket_once {
                crate::Events::ONCE
            } else {
                crate::Events::NONE
            };
        let mut socket = crate::SocketRecord::with_fd(fds[0], 5);
        let token: *mut std::ffi::c_void = (&raw mut socket).cast();
        unsafe {
            assert!(crate::reactor_state::tests::record(
                state.owner,
                &raw mut socket,
                crate::ChangeOp::Add,
                events
            ));
            assert!((*state.handlers).set(5, crate::Events::READ, Some(read)));
            let descriptor = (*state.owner).backend_fd;
            #[cfg(target_os = "linux")]
            if kind == 1 {
                assert_eq!(
                    crate::registration::epoll_ctl(
                        descriptor,
                        crate::ChangeOp::Add,
                        fds[0],
                        token,
                        events
                    )
                    .ok(),
                    Some(())
                );
            }
            #[cfg(not(target_os = "linux"))]
            if kind == 2 {
                assert_eq!(
                    crate::registration::kqueue_add(descriptor, fds[0], token, events),
                    Ok(())
                );
            }
            assert_eq!(libc::write(fds[1], b"a".as_ptr().cast(), 1), 1);
            // Without deferred work the second wait times out after 1ms; with it, at once.
            (*state.fields).timeout_msec = if deferred { -1 } else { 1 };
            (*state.fields).once = once;
        }
        PROBE.set((begin_stops, 0));
        if deferred {
            // SAFETY: the state's own field; the queue is detached after the wait.
            unsafe { (*state.fields).defer_tasks = Some(Box::default()) };
        }
        let ops = WaitOps {
            reactor: running(state.owner),
            begin,
            end,
        };
        assert!(wait(&ops));
        let removed = socket.removed;
        unsafe {
            assert!((*state.fields).start);
            (*state.fields).defer_tasks = None;
            crate::reactor_state::tests::free(state.owner);
            libc::close(fds[0]);
            libc::close(fds[1]);
        }
        (EFFECTS.with(|effects| effects.borrow().clone()), removed)
    }

    /// kqueue's signal entry: inactive does nothing, no handler warns, `SIG_IGN` is skipped,
    /// any other handler runs with the signal number.
    #[test]
    #[cfg(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    fn signal_entries_run_their_handler_as_upstreams_kqueue_wait() {
        use swoole_rs_process::signal::Handler;
        static SEEN: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
        extern "C" fn handler(signo: c_int) {
            SEEN.store(signo, std::sync::atomic::Ordering::SeqCst);
        }
        let handler = Handler::new(handler);
        for (activated, seen) in [
            (None, 0),
            (Some(Handler::NONE), 0),
            (Some(Handler::IGNORE), 0),
            (Some(handler), libc::SIGUSR1),
        ] {
            SEEN.store(0, std::sync::atomic::Ordering::SeqCst);
            kqueue_signal::dispatch(libc::SIGUSR1, activated);
            assert_eq!(SEEN.load(std::sync::atomic::Ordering::SeqCst), seen);
        }
    }
}
