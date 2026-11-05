//! `System::socket_poll` and the hooked `poll(2)` (`swoole_coroutine_poll`): wait
//! until any of several fds is ready. A zero timeout is one `poll(2)` here; otherwise
//! each fd is registered with the reactor, the coroutine parks, and the first
//! readiness or the timer wakes it once. The task and its bookkeeping are here;
//! native code passes reactor registration, timers and coroutine operations in
//! `PollOps`, and calls back `swr_v1_hook_poll_ready`/`_timeout`/`_completed`.
use crate::wait_event::{ERROR, READ, WRITE, from_poll, to_poll};
use std::cell::Cell;
use std::ffi::c_void;

/// One polled fd in Swoole event bits (upstream's `PollSocket`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PollFd {
    pub fd: i32,
    pub events: i32,
    pub revents: i32,
}

/// The native operations a coroutine poll needs.
#[repr(C)]
pub struct PollOps {
    /// Registers `fd` for `events` with `task` as its object; the socket, or null
    /// (released without closing the fd) if the reactor refused it.
    pub add: unsafe extern "C" fn(i32, i32, *mut c_void) -> *mut c_void,
    /// Unregisters a socket from `add` and releases it without closing the fd.
    pub remove: unsafe extern "C" fn(*mut c_void),
    /// A one-shot timer of `timeout` seconds calling `swr_v1_hook_poll_timeout(task)`.
    pub timer_add: unsafe extern "C" fn(f64, *mut c_void) -> *mut c_void,
    pub timer_del: unsafe extern "C" fn(*mut c_void),
    /// Defers `swr_v1_hook_poll_completed(task)` to the reactor.
    pub defer: unsafe extern "C" fn(*mut c_void),
    /// `Coroutine::get_current_safe()`.
    pub current: unsafe extern "C" fn() -> *mut c_void,
    /// `co->yield()`.
    pub park: unsafe extern "C" fn(*mut c_void),
    /// `co->resume()`.
    pub resume: unsafe extern "C" fn(*mut c_void),
}

/// A parked poll (upstream's `CoroPollTask`), on its coroutine's stack.
struct Task<'a> {
    fds: &'a [Cell<PollFd>],
    sockets: Vec<Cell<*mut c_void>>,
    co: *mut c_void,
    timer: Cell<*mut c_void>,
    success: Cell<bool>,
    wait: Cell<bool>,
    ops: &'a PollOps,
}

impl Task<'_> {
    fn clean(&self) {
        for socket in &self.sockets {
            let socket = socket.replace(std::ptr::null_mut());
            if !socket.is_null() {
                // SAFETY: `socket` came from `add` and is removed once.
                unsafe { (self.ops.remove)(socket) };
            }
        }
    }

    fn finish(&self) {
        self.clean();
        // SAFETY: `co` is parked in `poll_sockets`.
        unsafe { (self.ops.resume)(self.co) };
    }
}

/// # Safety
/// `task` is the object native registered through `PollOps::add` or a timer/defer.
unsafe fn task<'a>(task: *mut c_void) -> &'a Task<'a> {
    // SAFETY: the caller passes a live `Task`; all its mutation goes through cells.
    unsafe { &*task.cast::<Task<'a>>() }
}

/// Coalesces `event` into `received`: error readiness becomes the requested
/// read/write bits unless the error bit itself was requested.
fn coalesce(requested: i32, received: i32, event: i32) -> i32 {
    received
        | if event == ERROR && requested & ERROR == 0 {
            requested & (READ | WRITE)
        } else {
            event
        }
}

/// The reactor saw `event` on `fd` of a registered poll.
///
/// # Safety
/// `task` is a registered socket's object.
pub unsafe extern "C" fn swr_v1_hook_poll_ready(task: *mut c_void, fd: i32, event: i32) {
    let ptr = task;
    // SAFETY: the caller's precondition.
    let task = unsafe { self::task(ptr) };
    if let Some(entry) = task.fds.iter().find(|entry| entry.get().fd == fd) {
        let mut row = entry.get();
        row.revents = coalesce(row.events, row.revents, event);
        entry.set(row);
    }
    if task.wait.replace(false) {
        task.success.set(true);
        let timer = task.timer.replace(std::ptr::null_mut());
        if !timer.is_null() {
            // SAFETY: the timer is pending; it was added by this task.
            unsafe { (task.ops.timer_del)(timer) };
        }
        // SAFETY: `ptr` stays parked until the deferred completion resumes it.
        unsafe { (task.ops.defer)(ptr) };
    }
}

/// The deferred wake after the first readiness.
///
/// # Safety
/// `task` is the poll `PollOps::defer` was given.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_poll_completed(task: *mut c_void) {
    // SAFETY: the caller's precondition.
    unsafe { self::task(task) }.finish();
}

/// The poll's timer fired.
///
/// # Safety
/// `task` is the poll `PollOps::timer_add` was given.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_poll_timeout(task: *mut c_void) {
    // SAFETY: the caller's precondition.
    let task = unsafe { self::task(task) };
    task.timer.set(std::ptr::null_mut());
    task.success.set(false);
    task.wait.set(false);
    task.finish();
}

fn poll_now(fds: &[Cell<PollFd>]) -> bool {
    let mut list: Vec<libc::pollfd> = fds
        .iter()
        .map(|entry| libc::pollfd {
            fd: entry.get().fd,
            events: to_poll(entry.get().events),
            revents: 0,
        })
        .collect();
    let count = libc::nfds_t::try_from(list.len()).unwrap_or(libc::nfds_t::MAX);
    // SAFETY: `list` holds `count` pollfds.
    let ready = unsafe { libc::poll(list.as_mut_ptr(), count, 0) };
    if ready > 0 {
        for (entry, polled) in fds.iter().zip(&list) {
            let mut row = entry.get();
            row.revents = from_poll(polled.revents);
            entry.set(row);
        }
    }
    ready > 0
}

/// # Safety
/// `ops` are native's; with a nonzero timeout this runs in a coroutine.
unsafe fn poll_sockets(fds: &[Cell<PollFd>], timeout: f64, ops: &PollOps) -> bool {
    if timeout == 0.0 {
        return poll_now(fds);
    }
    let task = Task {
        fds,
        sockets: fds
            .iter()
            .map(|_| Cell::new(std::ptr::null_mut()))
            .collect(),
        // SAFETY: native's accessor; it bails out outside a coroutine.
        co: unsafe { (ops.current)() },
        timer: Cell::new(std::ptr::null_mut()),
        success: Cell::new(false),
        wait: Cell::new(true),
        ops,
    };
    let ptr = (&raw const task).cast_mut().cast::<c_void>();
    let mut tasked = false;
    for (entry, socket) in fds.iter().zip(&task.sockets) {
        let PollFd { fd, events, .. } = entry.get();
        // SAFETY: `ptr` outlives the registration: `clean` removes it before resuming.
        socket.set(unsafe { (ops.add)(fd, events, ptr) });
        tasked |= !socket.get().is_null();
    }
    if !tasked {
        return false;
    }
    if timeout > 0.0 {
        // SAFETY: as above; the timer is deleted or fires before resuming.
        task.timer.set(unsafe { (ops.timer_add)(timeout, ptr) });
    }
    // SAFETY: `co` is the current coroutine; callbacks reach `task` while parked.
    unsafe { (ops.park)(task.co) };
    task.success.get()
}

/// `System::socket_poll`: polls `fds[..count]`, storing each one's `revents`.
///
/// # Safety
/// `fds` holds `count` entries with distinct fds; `ops` as in [`PollOps`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_socket_poll(
    fds: *mut PollFd,
    count: usize,
    timeout: f64,
    ops: &PollOps,
) -> bool {
    let fds: &mut [PollFd] = if count == 0 {
        &mut []
    } else {
        // SAFETY: the caller passes `count` entries.
        unsafe { std::slice::from_raw_parts_mut(fds, count) }
    };
    // SAFETY: the caller's precondition.
    unsafe { poll_sockets(Cell::from_mut(fds).as_slice_of_cells(), timeout, ops) }
}

/// The hooked `poll(2)` in a coroutine with a nonzero timeout: a repeated fd counts
/// once, through its first entry; the others' `revents` stay as they were.
///
/// # Safety
/// `fds` holds `nfds` pollfds; `ops` as in [`PollOps`]; runs in a coroutine.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_poll(
    fds: *mut libc::pollfd,
    nfds: libc::nfds_t,
    timeout: i32,
    ops: &PollOps,
) -> i32 {
    let fds: &mut [libc::pollfd] = if nfds == 0 {
        &mut []
    } else {
        // SAFETY: the caller passes `nfds` pollfds.
        unsafe { std::slice::from_raw_parts_mut(fds, usize::try_from(nfds).unwrap_or(0)) }
    };
    let mut first = Vec::new();
    let mut rows: Vec<PollFd> = Vec::new();
    for (i, polled) in fds.iter().enumerate() {
        if rows.iter().all(|row| row.fd != polled.fd) {
            first.push(i);
            rows.push(PollFd {
                fd: polled.fd,
                events: from_poll(polled.events),
                revents: 0,
            });
        }
    }
    let rows = Cell::from_mut(&mut rows[..]).as_slice_of_cells();
    // SAFETY: the caller's precondition; `rows` has distinct fds.
    if !unsafe { poll_sockets(rows, f64::from(timeout) / 1000.0, ops) } {
        return -1;
    }
    let mut ready = 0;
    for (row, i) in rows.iter().zip(first) {
        let revents = row.get().revents;
        if let Some(fd) = fds.get_mut(i) {
            fd.revents = to_poll(revents);
        }
        ready += i32::from(revents > 0);
    }
    ready
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    thread_local! {
        static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
        /// Fds the fake reactor refuses, and what fires while parked.
        static REFUSE: Cell<i32> = const { Cell::new(-1) };
        static FIRE: Cell<Option<(i32, i32)>> = const { Cell::new(None) };
        static DEFERRED: Cell<usize> = const { Cell::new(0) };
        static TASK: Cell<usize> = const { Cell::new(0) };
    }
    fn log(entry: String) {
        LOG.with_borrow_mut(|log| log.push(entry));
    }
    fn take() -> Vec<String> {
        LOG.with_borrow_mut(std::mem::take)
    }
    unsafe extern "C" fn add(fd: i32, events: i32, task: *mut c_void) -> *mut c_void {
        log(format!("add {fd} {events}"));
        if fd == REFUSE.get() {
            return std::ptr::null_mut();
        }
        TASK.set(task as usize);
        (fd.unsigned_abs() as usize + 1000) as *mut c_void
    }
    unsafe extern "C" fn remove(socket: *mut c_void) {
        log(format!("remove {}", socket as usize - 1000));
    }
    unsafe extern "C" fn timer_add(timeout: f64, _: *mut c_void) -> *mut c_void {
        log(format!("timer {timeout}"));
        7 as *mut c_void
    }
    unsafe extern "C" fn timer_del(_: *mut c_void) {
        log("timer_del".into());
    }
    unsafe extern "C" fn defer(task: *mut c_void) {
        log("defer".into());
        DEFERRED.set(task as usize);
    }
    unsafe extern "C" fn current() -> *mut c_void {
        std::ptr::dangling_mut()
    }
    /// The reactor runs while parked: fire an event (or the timer), then the defer.
    unsafe extern "C" fn park(_: *mut c_void) {
        log("park".into());
        let task = TASK.get() as *mut c_void;
        match FIRE.get() {
            Some((fd, event)) => unsafe {
                swr_v1_hook_poll_ready(task, fd, event);
                swr_v1_hook_poll_ready(task, fd, event);
                swr_v1_hook_poll_completed(DEFERRED.get() as *mut c_void);
            },
            None => unsafe { swr_v1_hook_poll_timeout(task) },
        }
    }
    unsafe extern "C" fn resume(_: *mut c_void) {
        log("resume".into());
    }
    const OPS: PollOps = PollOps {
        add,
        remove,
        timer_add,
        timer_del,
        defer,
        current,
        park,
        resume,
    };

    #[test]
    fn readiness_wakes_once_and_cleans_up() {
        FIRE.set(Some((5, ERROR)));
        REFUSE.set(6);
        let mut fds = [
            PollFd {
                fd: 5,
                events: READ,
                revents: 0,
            },
            PollFd {
                fd: 6,
                events: WRITE,
                revents: 0,
            },
        ];
        assert!(unsafe { swr_v1_hook_socket_poll(fds.as_mut_ptr(), 2, 1.5, &OPS) });
        assert_eq!(
            fds[0].revents, READ,
            "error readiness becomes the requested read"
        );
        assert_eq!(fds[1].revents, 0);
        assert_eq!(
            take(),
            [
                "add 5 512",
                "add 6 1024",
                "timer 1.5",
                "park",
                "timer_del",
                "defer",
                "remove 5",
                "resume"
            ]
        );
    }

    #[test]
    fn timeout_fails_and_negative_has_no_timer() {
        FIRE.set(None);
        REFUSE.set(-1);
        let mut fds = [PollFd {
            fd: 3,
            events: READ | ERROR,
            revents: 0,
        }];
        assert!(!unsafe { swr_v1_hook_socket_poll(fds.as_mut_ptr(), 1, -1.0, &OPS) });
        assert_eq!(take(), ["add 3 2560", "park", "remove 3", "resume"]);
    }

    #[test]
    fn nothing_registered_fails_without_parking() {
        REFUSE.set(4);
        let mut fds = [PollFd {
            fd: 4,
            events: READ,
            revents: 0,
        }];
        assert!(!unsafe { swr_v1_hook_socket_poll(fds.as_mut_ptr(), 1, 1.0, &OPS) });
        assert_eq!(take(), ["add 4 512"]);
    }

    #[test]
    fn zero_timeout_polls_in_place() {
        let mut pair = [0; 2];
        assert_eq!(unsafe { libc::pipe(pair.as_mut_ptr()) }, 0);
        let mut fds = [
            PollFd {
                fd: pair[0],
                events: READ,
                revents: 0,
            },
            PollFd {
                fd: pair[1],
                events: WRITE,
                revents: 0,
            },
        ];
        assert!(unsafe { swr_v1_hook_socket_poll(fds.as_mut_ptr(), 2, 0.0, &OPS) });
        assert_eq!((fds[0].revents, fds[1].revents), (0, WRITE));
        assert_eq!(take(), Vec::<String>::new());
        unsafe {
            libc::close(pair[0]);
            libc::close(pair[1]);
        }
    }

    #[test]
    fn hooked_poll_counts_a_repeated_fd_once() {
        FIRE.set(Some((9, WRITE)));
        REFUSE.set(-1);
        let entry = |fd, events| libc::pollfd {
            fd,
            events,
            revents: -1,
        };
        let mut fds = [
            entry(9, libc::POLLOUT),
            entry(9, libc::POLLIN),
            entry(8, libc::POLLIN),
        ];
        assert_eq!(
            unsafe { swr_v1_hook_poll(fds.as_mut_ptr(), 3, 2000, &OPS) },
            1
        );
        assert_eq!(fds.map(|polled| polled.revents), [libc::POLLOUT, -1, 0]);
        assert_eq!(
            take(),
            [
                "add 9 1024",
                "add 8 512",
                "timer 2",
                "park",
                "timer_del",
                "defer",
                "remove 9",
                "remove 8",
                "resume"
            ]
        );
    }

    /// The ordered gate's former scenarios 643..931: upstream's
    /// `socket_poll_trigger_event` transcribed, against `swr_v1_hook_poll_ready`.
    #[test]
    fn ported_trigger_matrix() {
        let requested = [0, READ, WRITE, READ | WRITE, ERROR, ERROR | READ];
        let received = [0, READ, WRITE, ERROR];
        let events = [READ, WRITE, ERROR];
        for k in 0..288 {
            let (wait, timer) = ((k / 72) % 2 == 1, (k / 144) % 2 == 1);
            // Upstream: revents |= error && !(requested & error) ? requested & rw : event;
            // then the first wait claims success, deletes the timer and defers.
            let mut expected = Vec::new();
            let (mut revents, mut waiting, mut success, mut pending) =
                (received[(k / 6) % 4], wait, false, timer);
            for event in [events[(k / 24) % 3], READ] {
                let row = requested[k % 6];
                revents |= if event == ERROR && row & ERROR == 0 {
                    row & (READ | WRITE)
                } else {
                    event
                };
                if waiting {
                    waiting = false;
                    success = true;
                    if pending {
                        pending = false;
                        expected.push("timer_del".to_string());
                    }
                    expected.push("defer".to_string());
                }
                expected.push(format!("{revents} {waiting} {success}"));
            }
            let row = Cell::new(PollFd {
                fd: 77,
                events: requested[k % 6],
                revents: received[(k / 6) % 4],
            });
            let task = Task {
                fds: std::slice::from_ref(&row),
                sockets: Vec::new(),
                co: std::ptr::null_mut(),
                timer: Cell::new(if timer {
                    std::ptr::dangling_mut()
                } else {
                    std::ptr::null_mut()
                }),
                success: Cell::new(false),
                wait: Cell::new(wait),
                ops: &OPS,
            };
            let ptr = (&raw const task).cast_mut().cast::<c_void>();
            for event in [events[(k / 24) % 3], READ] {
                unsafe { swr_v1_hook_poll_ready(ptr, 77, event) };
                log(format!(
                    "{} {} {}",
                    row.get().revents,
                    task.wait.get(),
                    task.success.get()
                ));
            }
            assert_eq!(take(), expected, "scenario {}", k + 643);
        }
    }
}
