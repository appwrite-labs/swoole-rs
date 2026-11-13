// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/coroutine/iouring.cc (Swoole contributors).
//! Swoole's `io_uring` engine over the `io-uring` crate: the ring, its submission and
//! completion queues, the per-request events, the wait queue for a full ring, and
//! every operation `Iouring` offers. crates/hooks/src/iouring.rs keeps the reactor registration,
//! the coroutine switches and the log lines, reached through [`Host`].
//!
//! An operation runs on the calling coroutine's stack: its [`Event`] lives in the
//! operation's frame while the coroutine is suspended, and the completion handler
//! ([`wakeup`]) writes the result into it before resuming it.

use crate::vector::IoVector;
use io_uring::types::{Fd, FsyncFlags, Timespec};
use io_uring::{IoUring, Probe, opcode, squeue};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ffi::{CStr, c_char, c_int, c_uint, c_void};
use std::os::fd::AsRawFd;
use std::ptr;
use std::rc::Rc;
use std::sync::atomic::AtomicU32;
use swoole_rs_corebase::net_types::SocketAddress;
use swoole_rs_corebase::set_errno;
use swoole_rs_corebase::sys::{self as os, ReadSlot};

/// `SW_IOURING_QUEUE_SIZE`: default ring size and completion batch.
const QUEUE_SIZE: u32 = 8192;
/// `SW_IOURING_SQE_BATCH_SIZE`: submissions deferred to the end of the loop below it.
const SQE_BATCH_SIZE: usize = 16;
/// User data of a linked timeout's own completion, which is ignored.
const TIMEOUT_EVENT: u64 = u64::MAX;
/// `IORING_SETUP_SQPOLL`, the one `SwooleG.iouring_flag` the engine honours.
const SETUP_SQPOLL: u32 = 2;
/// Upstream's `#define FUTEX2_SIZE_U32` and `FUTEX_BITSET_MATCH_ANY`.
const FUTEX2_SIZE_U32: u32 = 0x02;
const FUTEX_BITSET_MATCH_ANY: u64 = 0xffff_ffff;

/// What the engine needs from the coroutine scheduler, the reactor and the log.
pub struct Host {
    /// `Coroutine::get_current_safe()`.
    pub current: extern "C" fn() -> *mut c_void,
    /// The thread's engine, created on first use (none if it failed).
    pub instance: fn() -> Option<Rc<Uring>>,
    /// Suspend `coroutine`, the running one, until [`wakeup`] resumes it;
    /// cancelling it calls [`cancel`] with `event`.
    pub suspend: fn(coroutine: *mut c_void, event: *mut c_void),
    pub resume: unsafe extern "C-unwind" fn(coroutine: *mut c_void),
    /// `swoole_event_is_running()`.
    pub event_running: fn() -> bool,
    /// Logs "`io_uring_submit()` failed" with errno.
    pub submit_failed: fn(),
    /// Logs that the ring with this fd had no room for a timeout.
    pub timeout_full: fn(ring_fd: c_int),
}

/// `SwooleG.iouring_*` and the opcode probes this build requires.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Config {
    pub entries: u32,
    pub flag: u32,
    pub workers: u32,
    pub probe_futex: bool,
    pub probe_ftruncate: bool,
}

/// [`open`]'s outcome. Status 0 ready; 2 the ring could not be created (errno
/// set, no engine); 3 the opcode probe failed; 4 no futex opcodes; 5 no ftruncate
/// opcode; 6 the async worker limit failed (errno set).
pub struct Opened {
    pub uring: Option<Rc<Uring>>,
    pub status: c_int,
}

struct Event {
    coroutine: *mut c_void,
    data: squeue::Entry,
    result: isize,
    /// The linked timeout, when the operation has one.
    timeout: Option<Timespec>,
}

/// The ring and its bookkeeping. Resuming a request's coroutine may start another
/// request, so the engine is shared and its state sits in cells.
pub struct Uring {
    /// Borrowed within one queue operation, never while a coroutine runs.
    ring: RefCell<IoUring>,
    host: &'static Host,
    task_num: Cell<u32>,
    /// Requests waiting for room in the ring.
    waiting: RefCell<VecDeque<*mut Event>>,
    /// Completed events and their results.
    ready: Cell<Vec<(*mut Event, isize)>>,
}

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// Upstream's `DOUBLE_TO_TIMESPEC`.
fn timespec(seconds: f64) -> libc::timespec {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream's (__kernel_time64_t) and (long long) casts"
    )]
    let (mut tv_sec, mut tv_nsec) = (
        seconds.trunc() as i64,
        (seconds.fract() * 1_000_000_000.0) as i64,
    );
    if tv_nsec >= 1_000_000_000 {
        tv_sec += 1;
        tv_nsec = 0;
    }
    libc::timespec { tv_sec, tv_nsec }
}

/// `ts` as the ring takes it. A negative field stays one the kernel refuses.
fn kernel_timespec(ts: libc::timespec) -> Timespec {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "nanoseconds are below a second's; a negative count stays out of range"
    )]
    let nsec = ts.tv_nsec as u32;
    Timespec::new().sec(ts.tv_sec.cast_unsigned()).nsec(nsec)
}

/// `SwooleG.iouring_entries` rounded up to a power of two, at least 64.
fn ring_entries(configured: u32) -> u32 {
    if configured == 0 {
        return QUEUE_SIZE;
    }
    let mut shift = 6;
    while (1_u64 << shift) < u64::from(configured) {
        shift += 1;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream's `1 << i` stored in a uint32_t"
    )]
    let entries = (1_u64 << shift) as u32;
    entries
}

/// `siginfo_t` of a waited child as `waitpid`'s status word.
fn wait_status(code: c_int, status: c_int) -> c_int {
    match code {
        libc::CLD_EXITED => (status & 0xFF) << 8,
        libc::CLD_KILLED => status & 0x7F,
        libc::CLD_DUMPED => (status & 0x7F) | 0x80,
        libc::CLD_STOPPED => ((status & 0xFF) << 8) | 0x7F,
        libc::CLD_CONTINUED => 0xFFFF,
        _ => 0,
    }
}

/// The completion's result: negative errno becomes -1 with errno set, and a
/// cancelled request (its linked timeout fired) reports `ETIMEDOUT`.
fn complete(result: isize) -> isize {
    if result < 0 {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "io_uring results are an i32 cqe.res"
        )]
        let error = result.wrapping_neg() as c_int;
        set_errno(if error == libc::ECANCELED {
            libc::ETIMEDOUT
        } else {
            error
        });
        -1
    } else {
        result
    }
}

impl Event {
    /// Upstream's `INIT_EVENT`: the current coroutine, or a fatal error outside one.
    fn new(host: &Host, data: squeue::Entry) -> Self {
        Self {
            coroutine: (host.current)(),
            data,
            result: 0,
            timeout: None,
        }
    }

    fn with_timeout(mut self, seconds: f64) -> Self {
        if seconds > 0.0 {
            self.timeout = Some(kernel_timespec(timespec(seconds)));
        }
        self
    }

    /// Submit and suspend until the completion; the result as upstream's `execute`.
    fn execute(&mut self, host: &Host) -> isize {
        let Some(engine) = (host.instance)() else {
            return -1;
        };
        engine.dispatch(self);
        engine.task_num.set(engine.task_num.get() + 1);
        // The thread keeps the engine; this frame holds nothing across the switch.
        drop(engine);
        // The wakeup handler resumes this coroutine; the event stays in this frame
        // until then.
        (host.suspend)(self.coroutine, ptr::from_mut(self).cast());
        self.result
    }
}

impl Uring {
    /// The submission queue's used slots and its size.
    fn sq(&self) -> (usize, usize) {
        let mut ring = self.ring.borrow_mut();
        let queue = ring.submission();
        (queue.len(), queue.capacity())
    }

    fn sq_space_left(&self) -> usize {
        let (used, capacity) = self.sq();
        capacity - used
    }

    /// Queue `entries` together, in the free slots the caller saw there are.
    fn push(&self, entries: &[squeue::Entry]) {
        let mut ring = self.ring.borrow_mut();
        // SAFETY: an event's entry points into its operation's arguments and frame
        // (`request`'s contract), and its timeout into the event itself: all live
        // until the completion, which the suspended operation waits for.
        let pushed = unsafe { ring.submission().push_multiple(entries) };
        debug_assert!(pushed.is_ok(), "the caller saw the free slots");
    }

    /// Queue `event` in the ring, or in the wait queue when the ring is full. Both
    /// keep its address: it stays suspended in its operation until its completion,
    /// so every request the ring holds is an event suspended in its operation.
    fn dispatch(&self, event: &mut Event) {
        let space = self.sq_space_left();
        if space == 0 {
            self.waiting.borrow_mut().push_back(event);
            return;
        }
        if event.timeout.is_some() && space < 2 {
            (self.host.timeout_full)(self.fd());
        }
        // User data identifies the event on completion.
        let entry = event.data.clone().user_data(ptr::from_mut(event) as u64);
        match &event.timeout {
            Some(timeout) if space >= 2 => self.push(&[
                entry.flags(squeue::Flags::IO_LINK),
                opcode::LinkTimeout::new(timeout)
                    .build()
                    .user_data(TIMEOUT_EVENT),
            ]),
            _ => self.push(&[entry]),
        }
        let mut ring = self;
        submit(&mut ring, false);
    }
}

/// The ring's side of a submit, so its order can be tested without one.
trait Submitter {
    fn event_running(&mut self) -> bool;
    fn sq_used(&mut self) -> usize;
    fn submit(&mut self) -> c_int;
    fn sleep(&mut self);
    fn failed(&mut self);
}

impl Submitter for &Uring {
    fn event_running(&mut self) -> bool {
        (self.host.event_running)()
    }

    fn sq_used(&mut self) -> usize {
        self.sq().0
    }

    fn submit(&mut self) -> c_int {
        let mut ring = self.ring.borrow_mut();
        let queue = ring.submission();
        // As liburing's io_uring_submit: nothing to submit or flush, no system call.
        let idle = queue.is_empty() && !queue.cq_overflow();
        drop(queue);
        if idle {
            return 0;
        }
        match ring.submit() {
            Ok(submitted) => c_int::try_from(submitted).unwrap_or(c_int::MAX),
            Err(error) => -error.raw_os_error().unwrap_or(0),
        }
    }

    fn sleep(&mut self) {
        os::usleep(10_000);
    }

    fn failed(&mut self) {
        (self.host.submit_failed)();
    }
}

/// Submit queued requests; below the batch size, only at the end of the loop.
fn submit(ring: &mut impl Submitter, immediately: bool) {
    if !immediately && ring.event_running() && ring.sq_used() < SQE_BATCH_SIZE {
        return;
    }
    loop {
        let ret = ring.submit();
        if ret >= 0 {
            return;
        }
        match -ret {
            // Retried on the next submit.
            libc::EAGAIN => return,
            libc::EBUSY => ring.sleep(),
            libc::EINTR => {}
            _ => {
                ring.failed();
                return;
            }
        }
    }
}

/// Create the thread's engine: size and create the ring, probe the opcodes this
/// build requires, and cap the async workers.
#[must_use]
pub fn open(config: Config, host: &'static Host) -> Opened {
    let mut builder: io_uring::Builder = IoUring::builder();
    if config.flag == SETUP_SQPOLL {
        // No idle time of its own: the kernel's default, as liburing asks for.
        builder.setup_sqpoll(0);
    }
    let ring = match builder.build(ring_entries(config.entries)) {
        Ok(ring) => ring,
        Err(error) => {
            set_errno(error.raw_os_error().unwrap_or(0));
            return Opened {
                uring: None,
                status: 2,
            };
        }
    };
    let mut status = probe(&ring, config);
    if status == 0
        && config.workers > 0
        && let Err(error) = ring
            .submitter()
            .register_iowq_max_workers(&mut [config.workers; 2])
    {
        set_errno(error.raw_os_error().unwrap_or(0));
        status = 6;
    }
    let engine = Uring {
        ring: RefCell::new(ring),
        host,
        task_num: Cell::new(0),
        waiting: RefCell::new(VecDeque::new()),
        ready: Cell::new(Vec::with_capacity(QUEUE_SIZE as usize)),
    };
    Opened {
        uring: Some(Rc::new(engine)),
        status,
    }
}

/// Whether the running kernel has the opcodes this build was configured with.
fn probe(ring: &IoUring, config: Config) -> c_int {
    if !config.probe_futex && !config.probe_ftruncate {
        return 0;
    }
    let mut probe = Probe::new();
    if ring.submitter().register_probe(&mut probe).is_err() {
        return 3;
    }
    if config.probe_futex
        && !(probe.is_supported(opcode::FutexWait::CODE)
            && probe.is_supported(opcode::FutexWake::CODE))
    {
        4
    } else if config.probe_ftruncate && !probe.is_supported(opcode::Ftruncate::CODE) {
        5
    } else {
        0
    }
}

impl Uring {
    #[must_use]
    pub fn fd(&self) -> c_int {
        self.ring.borrow().as_raw_fd()
    }

    /// Requests in flight.
    #[must_use]
    pub fn task_num(&self) -> u32 {
        self.task_num.get()
    }

    /// Requests waiting for room in the ring.
    #[must_use]
    pub fn waiting_num(&self) -> usize {
        self.waiting.borrow().len()
    }

    #[must_use]
    pub fn sq_usage_percent(&self) -> f32 {
        #[expect(
            clippy::cast_precision_loss,
            reason = "upstream's (float) used / capacity"
        )]
        let percent = {
            let (used, capacity) = self.sq();
            used as f32 / capacity as f32 * 100.0
        };
        percent
    }

    /// Submit everything queued: the reactor's end-of-loop callback.
    pub fn submit_queued(&self) {
        let mut ring = self;
        submit(&mut ring, true);
    }
}

/// The ring fd is readable: harvest completions, move waiting requests into the
/// freed slots, then resume each completed request's coroutine.
pub fn wakeup(engine: &Uring) -> bool {
    loop {
        // Resuming coroutines may re-enter the engine, so the ready list is taken
        // out of it, and no cell of it is borrowed while they run.
        let mut ready = engine.ready.take();
        let mut harvested = false;
        for cqe in engine
            .ring
            .borrow_mut()
            .completion()
            .take(QUEUE_SIZE as usize)
        {
            harvested = true;
            if cqe.user_data() != TIMEOUT_EVENT {
                ready.push((cqe.user_data() as *mut Event, cqe.result() as isize));
            }
        }
        if !harvested {
            engine.ready.set(ready);
            return true;
        }
        for _ in 0..ready.len() {
            if engine.sq_space_left() == 0 {
                break;
            }
            let Some(waiting) = engine.waiting.borrow_mut().pop_front() else {
                break;
            };
            // SAFETY: a queued event is suspended in its operation, and nothing
            // else borrows it.
            engine.dispatch(unsafe { &mut *waiting });
        }
        for &(event, result) in &ready {
            let coroutine = {
                // SAFETY: user data is an event suspended in its operation, and
                // nothing else borrows it until its coroutine resumes.
                let event = unsafe { &mut *event };
                event.result = complete(result);
                event.coroutine
            };
            engine.task_num.set(engine.task_num.get() - 1);
            // SAFETY: the coroutine suspended in that event's operation, which may
            // re-enter the engine.
            unsafe { (engine.host.resume)(coroutine) };
        }
        ready.clear();
        engine.ready.set(ready);
    }
}

/// Cancel the request whose event is at `event`, as a coroutine cancelling a
/// suspended one does. The kernel only compares the address with each request's
/// user data.
pub fn cancel(host: &Host, event: *mut c_void) -> bool {
    let cancel = opcode::AsyncCancel::new(event.addr() as u64).build();
    Event::new(host, cancel).execute(host) == 0
}

/// One `io_uring` request, end to end.
///
/// # Safety
/// The entry's pointers stay valid until the request completes, which is before
/// this returns.
unsafe fn request(host: &Host, timeout: f64, entry: squeue::Entry) -> isize {
    Event::new(host, entry).with_timeout(timeout).execute(host)
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "upstream's static_cast<int> of the request result"
)]
fn int(result: isize) -> c_int {
    result as c_int
}

macro_rules! operations {
    ($(
        $(#[$doc:meta])*
        fn $name:ident($($arg:ident: $ty:ty),*) -> $ret:ty, timeout $timeout:expr, $entry:expr, $convert:expr;
    )*) => {$(
        $(#[$doc])*
        ///
        /// # Safety
        /// Pointer arguments are valid as for the system call.
        #[must_use]
        pub unsafe fn $name(host: &Host, $($arg: $ty),*) -> $ret {
            let entry = $entry.build();
            // SAFETY: the caller's contract covers every pointer the entry carries.
            let result = unsafe { request(host, $timeout, entry) };
            $convert(result)
        }
    )*};
}

/// [`operations!`] whose entries carry no pointer but their borrowed arguments'.
macro_rules! safe_operations {
    ($(
        $(#[$doc:meta])*
        $vis:vis fn $name:ident($($arg:ident: $ty:ty),*) -> $ret:ty, timeout $timeout:expr, $entry:expr, $convert:expr;
    )*) => {$(
        $(#[$doc])*
        #[must_use]
        $vis fn $name(host: &Host, $($arg: $ty),*) -> $ret {
            let entry = $entry.build();
            // SAFETY: the entry's only pointers are the borrowed arguments', which
            // outlive the request: it completes before `request` returns.
            let result = unsafe { request(host, $timeout, entry) };
            $convert(result)
        }
    )*};
}

safe_operations! {
    #[expect(clippy::cast_possible_truncation, reason = "liburing stores the size_t length in an unsigned")]
    pub fn recv(fd: c_int, buf: &mut [impl ReadSlot], flags: c_int, timeout: f64) -> isize,
        timeout timeout, opcode::Recv::new(Fd(fd), buf.as_mut_ptr().cast(), buf.len() as u32).flags(flags), std::convert::identity;
    #[expect(clippy::cast_possible_truncation, reason = "liburing stores the size_t length in an unsigned")]
    pub fn send(fd: c_int, buf: &[u8], flags: c_int, timeout: f64) -> isize,
        timeout timeout, opcode::Send::new(Fd(fd), buf.as_ptr(), buf.len() as u32).flags(flags), std::convert::identity;
    pub fn close_fd(fd: c_int) -> c_int,
        timeout -1.0, opcode::Close::new(Fd(fd)), int;
    #[expect(clippy::cast_possible_truncation, reason = "upstream passes size_t to liburing's unsigned nbytes")]
    pub fn read(fd: c_int, buf: &mut [impl ReadSlot], timeout: f64) -> isize,
        timeout timeout, opcode::Read::new(Fd(fd), buf.as_mut_ptr().cast(), buf.len() as c_uint).offset(u64::MAX), std::convert::identity;
    #[expect(clippy::cast_possible_truncation, reason = "upstream passes size_t to liburing's unsigned nbytes")]
    pub fn write(fd: c_int, buf: &[u8], timeout: f64) -> isize,
        timeout timeout, opcode::Write::new(Fd(fd), buf.as_ptr(), buf.len() as c_uint).offset(u64::MAX), std::convert::identity;
    pub fn futex_wait(futex: &AtomicU32) -> c_int,
        timeout -1.0, opcode::FutexWait::new(futex.as_ptr(), 1, FUTEX_BITSET_MATCH_ANY, FUTEX2_SIZE_U32), int;
    pub fn futex_wakeup(futex: &AtomicU32) -> c_int,
        timeout -1.0, opcode::FutexWake::new(futex.as_ptr(), 1, FUTEX_BITSET_MATCH_ANY, FUTEX2_SIZE_U32), int;
    /// `IORING_OP_TIMEOUT` for `ts`.
    fn timer(ts: &Timespec) -> isize,
        timeout -1.0, opcode::Timeout::new(ts), std::convert::identity;
    /// `IORING_OP_STATX` of `path` under `dfd` into `buf`.
    fn statx_into(dfd: c_int, path: &CStr, flags: c_int, buf: &mut libc::statx) -> isize,
        timeout -1.0, opcode::Statx::new(Fd(dfd), path.as_ptr(), ptr::from_mut(buf).cast()).flags(flags).mask(libc::STATX_BASIC_STATS | libc::STATX_BTIME), std::convert::identity;
    /// `IORING_OP_WAITID` into `info`.
    fn waitid(idtype: libc::idtype_t, id: libc::id_t, options: c_int, info: &mut libc::siginfo_t, timeout: f64) -> c_int,
        timeout timeout, opcode::WaitId::new(idtype, id, options).infop(info), int;
    /// `IORING_OP_ACCEPT` on `fd`: the peer's address into `address`'s storage, at most
    /// `address.len` bytes of it (and no more than it holds), and its length into `address.len`.
    pub fn accept(fd: c_int, address: &mut SocketAddress, flags: c_int, timeout: f64) -> c_int,
        timeout timeout, {
            address.len = address.len.min(SocketAddress::CAPACITY);
            opcode::Accept::new(Fd(fd), (&raw mut address.addr).cast(), &raw mut address.len).flags(flags)
        }, int;
    /// `IORING_OP_SEND` of `buf` to `address`, of which the kernel reads at most its storage.
    #[expect(clippy::cast_possible_truncation, reason = "liburing stores the size_t length in an unsigned")]
    pub fn sendto(fd: c_int, buf: &[u8], address: &SocketAddress, timeout: f64) -> isize,
        timeout timeout, opcode::Send::new(Fd(fd), buf.as_ptr(), buf.len() as u32)
            .dest_addr((&raw const address.addr).cast())
            .dest_addr_len(address.len.min(SocketAddress::CAPACITY)), std::convert::identity;
    /// `IORING_OP_CONNECT` of `fd` to `peer`, whose address the kernel reads.
    pub fn connect(fd: c_int, peer: crate::lifecycle::Peer<'_>, timeout: f64) -> c_int,
        timeout timeout, opcode::Connect::new(Fd(fd), peer.as_ptr(), peer.len()), int;
    /// `IORING_OP_POLL_ADD` of `fd`, which carries no pointer.
    fn poll_add(fd: c_int, events: u32, timeout: f64) -> c_int,
        timeout timeout, opcode::PollAdd::new(Fd(fd), events), int;
}

operations! {
    fn open_file(pathname: *const c_char, flags: c_int, mode: libc::mode_t) -> c_int,
        timeout -1.0, opcode::OpenAt::new(Fd(libc::AT_FDCWD), pathname).flags(flags | libc::O_CLOEXEC).mode(mode), int;
    fn recvmsg(fd: c_int, message: *mut libc::msghdr, flags: c_int, timeout: f64) -> isize,
        timeout timeout, opcode::RecvMsg::new(Fd(fd), message).flags(flags.cast_unsigned()), std::convert::identity;
    fn sendmsg(fd: c_int, message: *const libc::msghdr, flags: c_int, timeout: f64) -> isize,
        timeout timeout, opcode::SendMsg::new(Fd(fd), message).flags(flags.cast_unsigned()), std::convert::identity;
    fn rename(oldpath: *const c_char, newpath: *const c_char) -> c_int,
        timeout -1.0, opcode::RenameAt::new(Fd(libc::AT_FDCWD), oldpath, Fd(libc::AT_FDCWD), newpath), int;
    fn mkdir(pathname: *const c_char, mode: libc::mode_t) -> c_int,
        timeout -1.0, opcode::MkDirAt::new(Fd(libc::AT_FDCWD), pathname).mode(mode), int;
    fn unlink(pathname: *const c_char) -> c_int,
        timeout -1.0, opcode::UnlinkAt::new(Fd(libc::AT_FDCWD), pathname), int;
    fn rmdir(pathname: *const c_char) -> c_int,
        timeout -1.0, opcode::UnlinkAt::new(Fd(libc::AT_FDCWD), pathname).flags(libc::AT_REMOVEDIR), int;
    fn fsync(fd: c_int) -> c_int,
        timeout -1.0, opcode::Fsync::new(Fd(fd)), int;
    fn fdatasync(fd: c_int) -> c_int,
        timeout -1.0, opcode::Fsync::new(Fd(fd)).flags(FsyncFlags::DATASYNC), int;
    fn ftruncate(fd: c_int, length: libc::off_t) -> c_int,
        timeout -1.0, opcode::Ftruncate::new(Fd(fd), length.cast_unsigned()), int;
}

/// `IORING_OP_READV` into `vector`'s remaining buffers, or `IORING_OP_WRITEV` from
/// them, as the vector was made for.
#[must_use]
pub fn vector(host: &Host, fd: c_int, vector: &IoVector, timeout: f64) -> isize {
    let iovecs = vector.iovecs();
    let count = c_int::try_from(iovecs.len()).unwrap_or(0).cast_unsigned();
    let iov = iovecs.as_ptr();
    let entry = if vector.write() {
        opcode::Writev::new(Fd(fd), iov, count)
            .offset(u64::MAX)
            .build()
    } else {
        opcode::Readv::new(Fd(fd), iov, count)
            .offset(u64::MAX)
            .build()
    };
    // SAFETY: the entry's pointers are the borrowed vector's iovecs, which only its
    // `&mut` methods move, and their buffers, valid in its direction while it lives
    // (`IoVector::new`'s contract); the request completes before `request` returns.
    unsafe { request(host, timeout, entry) }
}

/// Sleep `seconds` in the ring's timer: 0 when it expired, -1 if cancelled.
#[must_use]
pub fn sleep(host: &Host, seconds: f64) -> c_int {
    sleep_timespec(host, timespec(seconds))
}

/// Upstream's `sleep(tv_sec, tv_nsec)`, which `sleep(seconds)` converts to.
fn sleep_timespec(host: &Host, ts: libc::timespec) -> c_int {
    let _expired = timer(host, &kernel_timespec(ts));
    if errno() == libc::ETIME { 0 } else { -1 }
}

/// `size` bytes of `in_fd` from `offset` (the file position without one) to
/// `out_fd`, spliced through a pipe. Upstream's zero-copy sendmsg needs
/// `MSG_SPLICE_PAGES`, which is kernel-internal: no build's headers have it.
#[must_use]
pub fn sendfile(
    host: &Host,
    out_fd: c_int,
    in_fd: c_int,
    offset: Option<&mut libc::off_t>,
    size: usize,
    timeout: f64,
) -> isize {
    if size == 0 {
        return 0;
    }
    let Some([read_end, write_end]) = os::pipe() else {
        return -1;
    };
    os::fcntl(write_end, libc::F_SETPIPE_SZ, 1024 * 1024);
    let pipe_size = os::fcntl(read_end, libc::F_GETPIPE_SZ, 0);
    let pipe_size = usize::try_from(pipe_size).unwrap_or(65536);
    let close_pipe = || {
        os::close(read_end);
        os::close(write_end);
    };
    let start = offset.as_deref().copied();
    let mut total_sent = 0_usize;
    // Each splice completes before the next, and carries no pointers.
    let splice = |fd_in, off_in, fd_out, nbytes| {
        let splice = opcode::Splice::new(Fd(fd_in), off_in, Fd(fd_out), -1, nbytes).build();
        Event::new(host, splice).with_timeout(timeout).execute(host)
    };
    while total_sent < size {
        let to_send = (size - total_sent).min(pipe_size);
        let from = start.map_or(-1, |start| {
            start + i64::try_from(total_sent).unwrap_or(i64::MAX)
        });
        #[expect(clippy::cast_possible_truncation, reason = "at most the pipe size")]
        let nbytes = to_send as c_uint;
        let ret1 = splice(in_fd, from, write_end, nbytes);
        if ret1 <= 0 {
            if total_sent > 0 {
                break;
            }
            let saved = errno();
            close_pipe();
            set_errno(saved);
            return -1;
        }
        let spliced = c_uint::try_from(ret1).unwrap_or(c_uint::MAX);
        let ret2 = splice(read_end, -1, out_fd, spliced);
        if ret2 <= 0 {
            if total_sent > 0 {
                break;
            }
            let saved = errno();
            close_pipe();
            set_errno(saved);
            return -1;
        }
        total_sent += ret2.unsigned_abs();
        if ret2 < ret1 {
            break;
        }
    }
    close_pipe();
    if total_sent > 0
        && let Some(offset) = offset
    {
        *offset += libc::off_t::try_from(total_sent).unwrap_or(libc::off_t::MAX);
    }
    total_sent.cast_signed()
}

fn device(major: u32, minor: u32) -> libc::dev_t {
    // Upstream's ((unsigned int) major << 8) | minor, in unsigned int.
    libc::dev_t::from((major << 8) | minor)
}

/// Upstream's `swoole_statx_to_stat`: the fields it copies, the rest untouched.
fn statx_to_stat(statx: &libc::statx, stat: &mut libc::stat) {
    stat.st_dev = device(statx.stx_dev_major, statx.stx_dev_minor);
    stat.st_mode = statx.stx_mode.into();
    #[cfg_attr(
        target_arch = "aarch64",
        expect(clippy::useless_conversion, reason = "nlink_t is u32 on aarch64")
    )]
    let nlink = statx.stx_nlink.into();
    stat.st_nlink = nlink;
    stat.st_uid = statx.stx_uid;
    stat.st_gid = statx.stx_gid;
    stat.st_rdev = device(statx.stx_rdev_major, statx.stx_rdev_minor);
    stat.st_ino = statx.stx_ino;
    stat.st_size = statx.stx_size.cast_signed();
    // blksize_t is i64 on x86_64 and i32 on aarch64; the value is a block size.
    #[cfg(target_arch = "x86_64")]
    let blksize = libc::blksize_t::from(statx.stx_blksize);
    #[cfg(not(target_arch = "x86_64"))]
    let blksize = libc::blksize_t::try_from(statx.stx_blksize).unwrap_or(libc::blksize_t::MAX);
    stat.st_blksize = blksize;
    stat.st_blocks = statx.stx_blocks.cast_signed();
    stat.st_atime = statx.stx_atime.tv_sec;
    stat.st_atime_nsec = statx.stx_atime.tv_nsec.into();
    stat.st_mtime = statx.stx_mtime.tv_sec;
    stat.st_mtime_nsec = statx.stx_mtime.tv_nsec.into();
    stat.st_ctime = statx.stx_ctime.tv_sec;
    stat.st_ctime_nsec = statx.stx_ctime.tv_nsec.into();
}

fn statx(host: &Host, dfd: c_int, path: &CStr, flags: c_int, stat: &mut libc::stat) -> c_int {
    let mut buf: libc::statx = swoole_rs_corebase::sys::zeroed();
    let ret = statx_into(host, dfd, path, flags, &mut buf);
    if ret == 0 {
        statx_to_stat(&buf, stat);
    }
    int(ret)
}

/// `fstat(2)` of `fd` into `stat`.
pub fn fstat(host: &Host, fd: c_int, stat: &mut libc::stat) -> c_int {
    statx(host, fd, c"", libc::AT_EMPTY_PATH, stat)
}

/// `stat(2)` of `path` into `stat`; upstream's ring `lstat` is this too.
pub fn stat(host: &Host, path: &CStr, stat: &mut libc::stat) -> c_int {
    statx(host, libc::AT_FDCWD, path, libc::AT_SYMLINK_NOFOLLOW, stat)
}

/// `waitpid` through `IORING_OP_WAITID`; `WNOHANG` stays a plain waitpid(2).
#[must_use]
pub fn waitpid(
    host: &Host,
    pid: libc::pid_t,
    stat_loc: Option<&mut c_int>,
    options: c_int,
    timeout: f64,
) -> libc::pid_t {
    if options & libc::WNOHANG != 0 {
        return os::waitpid(pid, stat_loc, options);
    }
    let mut info: libc::siginfo_t = swoole_rs_corebase::sys::zeroed();
    let (idtype, id) = if pid > 0 {
        (libc::P_PID, pid.cast_unsigned())
    } else {
        (libc::P_ALL, 0)
    };
    let options = if options == 0 { libc::WEXITED } else { options };
    let rc = waitid(host, idtype, id, options, &mut info, timeout);
    if rc == -1 {
        return rc;
    }
    // SAFETY: the kernel filled `info` for a waited child.
    let status = wait_status(info.si_code, unsafe { info.si_status() });
    if let Some(stat_loc) = stat_loc {
        *stat_loc = status;
    }
    // SAFETY: as above.
    unsafe { info.si_pid() }
}

/// poll(2) of one descriptor; `timeout` 0 polls without the ring.
pub fn poll(host: &Host, pollfd: &mut libc::pollfd, timeout: c_int) -> c_int {
    if timeout == 0 {
        return os::poll(core::slice::from_mut(pollfd), timeout);
    }
    let (descriptor, events) = (pollfd.fd, pollfd.events);
    let rc = poll_add(
        host,
        descriptor,
        events.cast_unsigned().into(),
        f64::from(timeout) / 1000.0,
    );
    if rc > 0 {
        #[expect(clippy::cast_possible_truncation, reason = "upstream's revents = rc")]
        let revents = rc as libc::c_short;
        pollfd.revents = revents;
        return 1;
    }
    rc
}

#[cfg(test)]
mod ported;

/// A ring host for tests, shared with the TLS transport's.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    // A host without coroutines: "suspending" drives the ring until the event
    // completes, which is what the reactor and the scheduler do together.
    thread_local! {
        static ENGINE: RefCell<Option<Rc<Uring>>> = const { RefCell::new(None) };
    }
    extern "C" fn current() -> *mut c_void {
        ptr::dangling_mut()
    }
    fn instance() -> Option<Rc<Uring>> {
        ENGINE.with_borrow(Clone::clone)
    }
    fn suspend(_coroutine: *mut c_void, _event: *mut c_void) {
        let engine = instance().expect("a test engine");
        engine.submit_queued();
        while engine.task_num() > 0 {
            let _waited = engine.ring.borrow().submit_and_wait(1);
            wakeup(&engine);
        }
    }
    unsafe extern "C-unwind" fn resume(_coroutine: *mut c_void) {}
    fn event_running() -> bool {
        false
    }
    fn submit_failed() {}
    fn timeout_full(_fd: c_int) {}
    pub(crate) static HOST: Host = Host {
        current,
        instance,
        suspend,
        resume,
        event_running,
        submit_failed,
        timeout_full,
    };

    pub(crate) fn with_engine(test: impl FnOnce()) {
        with_config(Config::default(), test);
    }

    /// An engine as `SwooleG.iouring_*` configures it, for `test`.
    pub(crate) fn with_config(config: Config, test: impl FnOnce()) {
        let opened = open(config, &HOST);
        assert_eq!(opened.status, 0, "io_uring is available");
        ENGINE.set(opened.uring);
        test();
        ENGINE.set(None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Recorder {
        running: bool,
        used: usize,
        results: Vec<c_int>,
        trace: Vec<String>,
    }

    impl Submitter for Recorder {
        fn event_running(&mut self) -> bool {
            self.trace.push("running".into());
            self.running
        }
        fn sq_used(&mut self) -> usize {
            self.trace.push("sq:used".into());
            self.used
        }
        fn submit(&mut self) -> c_int {
            self.trace.push("submit".into());
            self.results.remove(0)
        }
        fn sleep(&mut self) {
            self.trace.push("usleep:10000".into());
        }
        fn failed(&mut self) {
            self.trace.push("submit:error".into());
        }
    }

    fn submitted(running: bool, used: usize, results: &[c_int], immediately: bool) -> Vec<String> {
        let mut ring = Recorder {
            running,
            used,
            results: results.to_vec(),
            ..Recorder::default()
        };
        submit(&mut ring, immediately);
        ring.trace
    }

    // The hooks ordered gate's io_uring submit scenarios, against upstream's order.
    #[test]
    fn submit_batches_and_retries_in_upstream_order() {
        let (busy, intr, again) = (-libc::EBUSY, -libc::EINTR, -libc::EAGAIN);
        assert_eq!(
            submitted(true, 16, &[0], false),
            ["running", "sq:used", "submit"],
            "a full batch"
        );
        assert_eq!(
            submitted(true, 15, &[0], false),
            ["running", "sq:used"],
            "deferred"
        );
        assert_eq!(
            submitted(false, 15, &[0], false),
            ["running", "submit"],
            "loop not running"
        );
        assert_eq!(
            submitted(true, 16, &[again], true),
            ["submit"],
            "EAGAIN waits"
        );
        assert_eq!(
            submitted(true, 16, &[busy, 1], true),
            ["submit", "usleep:10000", "submit"],
            "EBUSY sleeps"
        );
        assert_eq!(
            submitted(true, 16, &[intr, 1], true),
            ["submit", "submit"],
            "EINTR retries"
        );
        assert_eq!(
            submitted(true, 16, &[-libc::EINVAL], true),
            ["submit", "submit:error"],
            "other errors log"
        );
        assert_eq!(
            submitted(true, 16, &[busy, intr, again], true),
            ["submit", "usleep:10000", "submit", "submit"],
            "retries end at EAGAIN"
        );
        assert_eq!(
            submitted(true, 16, &[busy, intr, 2], true),
            ["submit", "usleep:10000", "submit", "submit"],
            "retries end at success"
        );
        assert_eq!(submitted(true, 16, &[0], true), ["submit"], "immediately");
    }

    #[test]
    fn timespec_matches_double_to_timespec() {
        let ts = timespec(1.5);
        assert_eq!((ts.tv_sec, ts.tv_nsec), (1, 500_000_000), "1.5s");
        let ts = timespec(0.001);
        assert_eq!(ts.tv_sec, 0, "whole seconds");
        assert!((999_999..=1_000_000).contains(&ts.tv_nsec), "1ms in ns");
    }

    #[test]
    fn entries_round_up_to_a_power_of_two() {
        assert_eq!(ring_entries(0), QUEUE_SIZE, "unset keeps the default");
        assert_eq!(ring_entries(1), 64, "at least 64");
        assert_eq!(ring_entries(65), 128, "rounded up");
        assert_eq!(ring_entries(4096), 4096, "a power of two stays");
    }

    #[test]
    fn wait_status_matches_the_wait_macros() {
        let exited = wait_status(libc::CLD_EXITED, 3);
        assert!(
            libc::WIFEXITED(exited) && libc::WEXITSTATUS(exited) == 3,
            "exited"
        );
        let killed = wait_status(libc::CLD_KILLED, libc::SIGKILL);
        assert!(
            libc::WIFSIGNALED(killed) && libc::WTERMSIG(killed) == libc::SIGKILL,
            "killed"
        );
        assert!(
            libc::WCOREDUMP(wait_status(libc::CLD_DUMPED, libc::SIGSEGV)),
            "dumped"
        );
        assert!(
            libc::WIFSTOPPED(wait_status(libc::CLD_STOPPED, libc::SIGSTOP)),
            "stopped"
        );
        assert!(
            libc::WIFCONTINUED(wait_status(libc::CLD_CONTINUED, 0)),
            "continued"
        );
    }

    #[test]
    fn completion_maps_cancel_to_timeout() {
        assert_eq!(complete(5), 5, "success passes through");
        assert_eq!(complete(-(libc::ECANCELED as isize)), -1, "failure is -1");
        assert_eq!(errno(), libc::ETIMEDOUT, "cancelled reads as a timeout");
        assert_eq!(complete(-(libc::EBADF as isize)), -1, "failure is -1");
        assert_eq!(errno(), libc::EBADF, "other errors pass through");
    }
}
