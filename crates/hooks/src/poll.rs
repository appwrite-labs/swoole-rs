//! `System::socket_poll` and the hooked `poll(2)` (`swoole_coroutine_poll`): wait
//! until any of several fds is ready. A zero timeout is one `poll(2)` here; otherwise
//! each fd is registered with the reactor, the coroutine parks, and the first
//! readiness or the timer wakes it once. [`NATIVE`] registers with the reactor, arms
//! the timer and parks; tests pass their own [`PollOps`].
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

/// The reactor, timer and coroutine operations a coroutine poll needs. The poll calls
/// each only with what it made or was given for this wait: its own parked task, the
/// sockets and timer `add` and `timer_add` returned, and the current coroutine.
pub struct PollOps {
    /// Registers `fd` for `events` with `task` as its object; the socket, or null
    /// (released without closing the fd) if the reactor refused it.
    pub add: fn(i32, i32, *mut c_void) -> *mut c_void,
    /// Unregisters a socket from `add` and releases it without closing the fd.
    pub remove: fn(*mut c_void),
    /// A one-shot timer of `timeout` seconds calling `swr_v1_hook_poll_timeout(task)`.
    pub timer_add: fn(f64, *mut c_void) -> *mut c_void,
    pub timer_del: fn(*mut c_void),
    /// Defers `swr_v1_hook_poll_completed(task)` to the reactor.
    pub defer: fn(*mut c_void),
    /// `Coroutine::get_current_safe()`.
    pub current: extern "C" fn() -> *mut c_void,
    /// `co->yield()`.
    pub park: fn(),
    /// `co->resume()`.
    pub resume: fn(*mut c_void),
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
                (self.ops.remove)(socket);
            }
        }
    }

    fn finish(&self) {
        self.clean();
        (self.ops.resume)(self.co);
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
            (task.ops.timer_del)(timer);
        }
        (task.ops.defer)(ptr);
    }
}

/// The deferred wake after the first readiness.
///
/// # Safety
/// `task` is the poll `PollOps::defer` was given.
pub unsafe extern "C-unwind" fn swr_v1_hook_poll_completed(task: *mut c_void) {
    // SAFETY: the caller's precondition.
    unsafe { self::task(task) }.finish();
}

/// The poll's timer fired.
///
/// # Safety
/// `task` is the poll `PollOps::timer_add` was given.
pub unsafe extern "C-unwind" fn swr_v1_hook_poll_timeout(task: *mut c_void) {
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
    let ready = swoole_rs_corebase::sys::poll(&mut list, 0);
    if ready > 0 {
        for (entry, polled) in fds.iter().zip(&list) {
            let mut row = entry.get();
            row.revents = from_poll(polled.revents);
            entry.set(row);
        }
    }
    ready > 0
}

/// With a nonzero timeout, outside a coroutine `ops.current` fails fatally.
fn poll_sockets(fds: &[Cell<PollFd>], timeout: f64, ops: &PollOps) -> bool {
    if timeout == 0.0 {
        return poll_now(fds);
    }
    let task = Task {
        fds,
        sockets: fds
            .iter()
            .map(|_| Cell::new(std::ptr::null_mut()))
            .collect(),
        co: (ops.current)(),
        timer: Cell::new(std::ptr::null_mut()),
        success: Cell::new(false),
        wait: Cell::new(true),
        ops,
    };
    let ptr = (&raw const task).cast_mut().cast::<c_void>();
    let mut tasked = false;
    for (entry, socket) in fds.iter().zip(&task.sockets) {
        let PollFd { fd, events, .. } = entry.get();
        // `ptr` outlives the registration: `clean` removes it before resuming.
        socket.set((ops.add)(fd, events, ptr));
        tasked |= !socket.get().is_null();
    }
    if !tasked {
        return false;
    }
    if timeout > 0.0 {
        // The timer is deleted or fires before resuming.
        task.timer.set((ops.timer_add)(timeout, ptr));
    }
    // Callbacks reach `task` while `co` is parked.
    (ops.park)();
    task.success.get()
}

/// `System::socket_poll` over `fds` (distinct fds), storing each one's `revents`: in a
/// coroutine unless `timeout` is 0.
pub fn socket_poll(fds: &mut [PollFd], timeout: f64, ops: &PollOps) -> bool {
    poll_sockets(Cell::from_mut(fds).as_slice_of_cells(), timeout, ops)
}

/// The bucket counts a libstdc++ `std::unordered_map<int, _>` grows through: the
/// first insert takes the first, and each later one past the current count the next.
const UNORDERED_BUCKETS: [u64; 18] = [
    13, 29, 59, 127, 257, 541, 1_109, 2_357, 5_087, 10_273, 20_753, 42_043, 85_229, 172_933,
    351_061, 712_697, 1_447_153, 2_938_679,
];

/// The order libstdc++'s `std::unordered_map<int, _>` iterates `keys` inserted in
/// turn (upstream's `socket_poll` map): a new key goes first in its bucket's run, or
/// at the front when its bucket is empty, and growing re-inserts each key in order.
fn unordered_map_order(keys: impl IntoIterator<Item = i32>) -> Vec<i32> {
    fn insert(order: &mut Vec<i32>, key: i32, buckets: u64) {
        let bucket = |key: i32| i64::from(key).cast_unsigned() % buckets;
        let at = order.iter().position(|&other| bucket(other) == bucket(key));
        order.insert(at.unwrap_or(0), key);
    }
    let mut order: Vec<i32> = Vec::new();
    let mut sizes = UNORDERED_BUCKETS.iter().copied().peekable();
    let mut buckets = 0;
    for key in keys {
        if order.contains(&key) {
            continue;
        }
        if order.len() as u64 >= buckets
            && let Some(grown) = sizes.next()
        {
            buckets = grown;
            for key in std::mem::take(&mut order) {
                insert(&mut order, key, buckets);
            }
        }
        insert(&mut order, key, buckets);
    }
    order
}

/// `System::socket_poll` of `(fd, events)` rows merged by fd, in upstream's map
/// order: each polled fd and its revents, or none on a timeout or when no fd could
/// be added. Outside a coroutine a nonzero `timeout` is upstream's fatal error.
#[must_use]
pub fn socket_poll_rows(rows: &[(i32, i32)], timeout: f64) -> Option<Vec<(i32, i32)>> {
    let mut fds: Vec<PollFd> = unordered_map_order(rows.iter().map(|row| row.0))
        .into_iter()
        .map(|fd| PollFd {
            fd,
            events: rows
                .iter()
                .filter(|row| row.0 == fd)
                .fold(0, |all, row| all | row.1),
            revents: 0,
        })
        .collect();
    let polled = socket_poll(&mut fds, timeout, &NATIVE);
    polled.then(|| fds.iter().map(|fd| (fd.fd, fd.revents)).collect())
}

/// The hooked `poll(2)` in a coroutine with a nonzero timeout: a repeated fd counts
/// once, through its first entry; the others' `revents` stay as they were.
///
/// # Safety
/// `fds` holds `nfds` pollfds.
pub unsafe fn swr_v1_hook_poll(
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
    // `rows` has distinct fds.
    if !poll_sockets(rows, f64::from(timeout) / 1000.0, ops) {
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

/// System's poll operations: the reactor, the timer and the current coroutine.
#[cfg(not(test))]
pub static NATIVE: PollOps = PollOps {
    add: native::add,
    remove: native::remove,
    timer_add: native::timer_add,
    timer_del: native::timer_del,
    defer: native::defer,
    current: crate::wait_event::native::current,
    park: crate::resolve::park,
    resume: native::resume,
};

/// The unit tests link no reactor: theirs is the fake.
#[cfg(test)]
pub static NATIVE: PollOps = tests::OPS;

#[cfg(not(test))]
pub(crate) mod native {
    use crate::wait_event::native as event;
    use core::ffi::c_void;
    use swoole_rs_corebase::constants::FdType;

    use swoole_rs_timer::{swoole_timer_del, swr_v1_timer_add_plain_sec};

    pub(super) fn add(fd: i32, events: i32, task: *mut c_void) -> *mut c_void {
        // SAFETY: a poll registers in a coroutine, on its thread's running reactor.
        unsafe { event::add::<{ FdType::SW_FD_CO_POLL as i32 }>(fd, events, task, &mut 0) }
    }

    pub(super) fn remove(socket: *mut c_void) {
        // SAFETY: a socket `add` registered, removed once.
        unsafe { event::remove(socket) };
    }

    pub(super) fn timer_add(timeout: f64, task: *mut c_void) -> *mut c_void {
        // SAFETY: the task outlives the timer, which fires or is deleted first.
        unsafe { swr_v1_timer_add_plain_sec(timeout, false, super::swr_v1_hook_poll_timeout, task) }
    }

    pub(crate) fn timer_del(timer: *mut c_void) {
        // SAFETY: the task's pending timer.
        unsafe { swoole_timer_del(timer.cast()) };
    }

    pub(super) fn defer(task: *mut c_void) {
        // SAFETY: the task stays parked until the deferred completion runs.
        unsafe { crate::reactor_handlers::defer(super::swr_v1_hook_poll_completed, task, None) };
    }

    pub(crate) fn resume(co: *mut c_void) {
        // SAFETY: the coroutine parked in the poll.
        unsafe { crate::resolve::resume(co) };
    }
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
    fn add(fd: i32, events: i32, task: *mut c_void) -> *mut c_void {
        log(format!("add {fd} {events}"));
        if fd == REFUSE.get() {
            return std::ptr::null_mut();
        }
        TASK.set(task as usize);
        (fd.unsigned_abs() as usize + 1000) as *mut c_void
    }
    fn remove(socket: *mut c_void) {
        log(format!("remove {}", socket as usize - 1000));
    }
    fn timer_add(timeout: f64, _: *mut c_void) -> *mut c_void {
        log(format!("timer {timeout}"));
        7 as *mut c_void
    }
    fn timer_del(_: *mut c_void) {
        log("timer_del".into());
    }
    fn defer(task: *mut c_void) {
        log("defer".into());
        DEFERRED.set(task as usize);
    }
    extern "C" fn current() -> *mut c_void {
        std::ptr::dangling_mut()
    }
    /// The reactor runs while parked: fire an event (or the timer), then the defer.
    fn park() {
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
    fn resume(_: *mut c_void) {
        log("resume".into());
    }
    pub(super) const OPS: PollOps = PollOps {
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
        assert!(socket_poll(&mut fds, 1.5, &OPS));
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
        assert!(!socket_poll(&mut fds, -1.0, &OPS));
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
        assert!(!socket_poll(&mut fds, 1.0, &OPS));
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
        assert!(socket_poll(&mut fds, 0.0, &OPS));
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
    /// libstdc++'s iteration orders, printed by a `std::unordered_map<int, int>` filled
    /// with `m[key] |= 1` in turn (GCC, the Docker image's toolchain).
    #[test]
    fn unordered_map_order_is_libstdcxx() {
        assert_eq!(unordered_map_order([5, 6]), [6, 5]);
        assert_eq!(
            unordered_map_order([7, 3, 9, 3, 12, 20, 1]),
            [1, 12, 9, 3, 20, 7]
        );
        let keys = (0..40).map(|n| (n * 37 + 11) % 101);
        assert_eq!(
            unordered_map_order(keys),
            [
                40, 3, 67, 30, 94, 57, 20, 84, 47, 10, 74, 51, 14, 78, 41, 68, 31, 4, 91, 95, 37,
                58, 0, 21, 85, 27, 48, 11, 98, 88, 24, 61, 34, 71, 7, 44, 81, 17, 54, 64
            ]
        );
        let keys = (0..300).map(|n| (n * 7919 + 13) % 1009);
        assert_eq!(
            unordered_map_order(keys),
            [
                680, 833, 986, 589, 742, 895, 498, 651, 804, 957, 560, 713, 866, 469, 622, 775,
                928, 531, 684, 837, 990, 593, 746, 899, 915, 212, 365, 518, 130, 671, 283, 824,
                436, 977, 121, 274, 427, 39, 580, 192, 733, 345, 886, 183, 336, 489, 101, 642, 254,
                795, 59, 407, 948, 245, 502, 398, 655, 10, 551, 808, 163, 704, 961, 316, 857, 154,
                307, 564, 460, 717, 72, 613, 870, 225, 766, 30, 378, 919, 216, 473, 369, 626, 522,
                779, 134, 675, 932, 287, 828, 92, 440, 981, 125, 278, 535, 431, 688, 43, 584, 841,
                994, 1, 349, 890, 187, 340, 597, 493, 750, 903, 258, 799, 63, 411, 952, 249, 506,
                402, 659, 812, 965, 320, 861, 158, 311, 568, 464, 721, 874, 34, 382, 923, 67, 415,
                956, 42, 299, 13, 270, 133, 390, 104, 361, 224, 481, 195, 452, 220, 477, 71, 869,
                328, 1002, 973, 849, 820, 940, 911, 439, 696, 410, 667, 530, 787, 501, 758, 286,
                543, 257, 514, 377, 634, 348, 605, 878, 468, 725, 315, 572, 162, 960, 419, 9, 807,
                266, 5, 324, 865, 969, 816, 406, 663, 253, 510, 100, 898, 357, 96, 894, 353, 444,
                985, 262, 803, 907, 497, 754, 344, 601, 191, 989, 448, 38, 836, 295, 998, 845, 435,
                692, 282, 539, 129, 927, 386, 291, 832, 936, 526, 783, 373, 630, 762, 609, 456,
                303, 150, 1006, 853, 700, 547, 394, 241, 88, 944, 791, 638, 485, 332, 179, 26, 882,
                729, 576, 423, 117, 208, 55, 146, 237, 84, 716, 175, 563, 22, 654, 113, 745, 204,
                592, 51, 683, 142, 774, 233, 621, 80, 712, 171, 559, 18, 650, 109, 741, 200, 588,
                47, 679, 138, 770, 229, 617, 76, 708, 167, 555, 14, 646, 105, 737, 196
            ]
        );
    }

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
