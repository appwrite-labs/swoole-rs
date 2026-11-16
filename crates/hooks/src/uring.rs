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

use crate::vector::{IoVector, RecvMessage, SendMessage};
use io_uring::types::{Fd, FsyncFlags, Timespec};
use io_uring::{IoUring, Probe, opcode, squeue};
use std::cell::Cell;
use std::collections::VecDeque;
use std::ffi::{CStr, c_int, c_uint, c_void};
use std::io;
use std::os::fd::AsRawFd;
use std::ptr;
use std::rc::Rc;
use std::sync::atomic::AtomicU32;
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::net_types::SocketAddress;
use swoole_rs_corebase::set_errno;
use swoole_rs_corebase::sys::{self as os, ReadSlot};
use swoole_rs_coroutine::Waker;

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
    /// Suspend `coroutine`, the running one, until [`wakeup`] wakes it, storing the
    /// park's waker in `waker` first; cancelling it calls [`cancel`] with `event`.
    pub suspend: fn(coroutine: *mut c_void, event: *mut c_void, waker: &Cell<Option<Waker>>),
    /// `co->resume()` of a parked request's coroutine.
    pub resume: fn(Waker),
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenError {
    Create,
    Probe,
    Futex,
    Ftruncate,
    Workers,
}

/// A created ring remains owned even when its probe or worker setup failed.
pub struct Opened {
    pub uring: Option<Rc<Uring>>,
    pub status: Result<(), OpenError>,
}

/// A request, in its operation's frame while that suspends: the ring and the wait
/// queue reach it by address, shared, so its effects sit in cells.
struct Event {
    coroutine: *mut c_void,
    data: squeue::Entry,
    /// The suspended operation's waker, which the completion wakes.
    waker: Cell<Option<Waker>>,
    result: Cell<io::Result<isize>>,
    /// The linked timeout, when the operation has one.
    timeout: Option<Timespec>,
}

/// The ring and its bookkeeping. Resuming a request's coroutine may start another
/// request, so the engine is shared and its state sits in cells, each taken out
/// and put back with nothing in between that runs a coroutine or calls back.
pub struct Uring {
    /// Taken out within one queue operation ([`Uring::with_ring`]). Boxed, so a
    /// take moves a pointer.
    ring: Cell<Option<Box<IoUring>>>,
    /// The ring's fd.
    fd: c_int,
    host: &'static Host,
    task_num: Cell<u32>,
    /// Requests waiting for room in the ring.
    waiting: Cell<VecDeque<*const Event>>,
    /// Completed events and their results.
    ready: Cell<Vec<(*const Event, isize)>>,
}

use swoole_rs_corebase::sys::errno;

/// Upstream's `DOUBLE_TO_TIMESPEC`.
fn timespec(seconds: f64) -> libc::timespec {
    let (mut tv_sec, mut tv_nsec) = (
        seconds.trunc().c_cast::<i64>(),
        (seconds.fract() * 1_000_000_000.0).c_cast::<i64>(),
    );
    if tv_nsec >= 1_000_000_000 {
        tv_sec += 1;
        tv_nsec = 0;
    }
    libc::timespec { tv_sec, tv_nsec }
}

/// `ts` as the ring takes it. A negative field stays one the kernel refuses.
fn kernel_timespec(ts: libc::timespec) -> Timespec {
    // Nanoseconds are below a second's; a negative count stays out of range.
    let nsec = ts.tv_nsec.c_cast::<u32>();
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
    (1_u64 << shift).c_cast::<u32>()
}

/// `siginfo_t`'s `_sigchld` member: the child, its user and its status (then its
/// times, which nothing reads). Its `long`s align it as the kernel's union is.
#[repr(C)]
#[derive(Default)]
struct Child {
    pid: libc::pid_t,
    uid: libc::uid_t,
    status: c_int,
    times: [std::ffi::c_long; 2],
}

/// The `siginfo_t` `IORING_OP_WAITID` fills, read as a waited child's: the code, then
/// the union as [`Child`], padded to the kernel's size, all plain integers.
#[repr(C)]
#[derive(Default)]
struct ChildInfo {
    head: [c_int; 3],
    child: Child,
    tail: [std::ffi::c_long; CHILD_INFO_TAIL],
}

/// Where the union starts: after the three `int`s, at the union's alignment.
const CHILD_AT: usize = (3 * size_of::<c_int>()).next_multiple_of(align_of::<Child>());
const CHILD_INFO_TAIL: usize =
    (size_of::<libc::siginfo_t>() - CHILD_AT - size_of::<Child>()) / size_of::<std::ffi::c_long>();
const _: () = assert!(
    size_of::<ChildInfo>() == size_of::<libc::siginfo_t>()
        && align_of::<ChildInfo>() >= align_of::<libc::siginfo_t>()
        && std::mem::offset_of!(ChildInfo, child) == CHILD_AT,
    "siginfo_t's layout moved"
);

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

/// The completion retains its error and publishes errno before resuming; a
/// cancelled request (its linked timeout fired) reports `ETIMEDOUT`.
fn complete(result: isize) -> io::Result<isize> {
    if result < 0 {
        // An io_uring result is an i32 `cqe.res`.
        let error = result.wrapping_neg().c_cast::<c_int>();
        let error = if error == libc::ECANCELED {
            libc::ETIMEDOUT
        } else {
            error
        };
        set_errno(error);
        Err(io::Error::from_raw_os_error(error))
    } else {
        Ok(result)
    }
}

impl Event {
    /// Upstream's `INIT_EVENT`: the current coroutine, or a fatal error outside one.
    fn new(host: &Host, data: squeue::Entry) -> Self {
        Self {
            coroutine: (host.current)(),
            data,
            waker: Cell::new(None),
            result: Cell::new(Ok(0)),
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
    fn execute(self, host: &Host) -> io::Result<isize> {
        let Some(engine) = (host.instance)() else {
            return Err(io::Error::last_os_error());
        };
        engine.dispatch(&self);
        engine.task_num.set(engine.task_num.get() + 1);
        // The thread keeps the engine; this frame holds nothing across the switch.
        drop(engine);
        // The wakeup handler resumes this coroutine; the event stays in this frame
        // until then.
        (host.suspend)(
            self.coroutine,
            ptr::from_ref(&self).cast_mut().cast(),
            &self.waker,
        );
        self.result.into_inner()
    }
}

impl Uring {
    /// `f` over the ring, taken out meanwhile; none while a caller up the stack
    /// has it, which no caller does, as `f` neither runs a coroutine nor calls back.
    fn with_ring<R>(&self, f: impl FnOnce(&mut IoUring) -> R) -> Option<R> {
        let mut ring = self.ring.take()?;
        let result = f(&mut ring);
        // `set` would call the drop glue for the `None` it replaces.
        if let Some(nested) = self.ring.replace(Some(ring)) {
            drop(nested);
        }
        Some(result)
    }

    /// The submission queue's used slots and its size; a ring in use has no room.
    fn sq(&self) -> (usize, usize) {
        self.with_ring(|ring| {
            let queue = ring.submission();
            (queue.len(), queue.capacity())
        })
        .unwrap_or_default()
    }

    fn sq_space_left(&self) -> usize {
        let (used, capacity) = self.sq();
        capacity - used
    }

    /// Queue `entries` together, in the free slots the caller saw there are.
    fn push(&self, entries: &[squeue::Entry]) {
        let pushed = self.with_ring(|ring| {
            // SAFETY: an event's entry points into its operation's arguments and frame
            // (`request`'s contract), and its timeout into the event itself: all live
            // until the completion, which the suspended operation waits for.
            unsafe { ring.submission().push_multiple(entries) }
        });
        debug_assert!(
            matches!(pushed, Some(Ok(()))),
            "the caller saw the free slots"
        );
    }

    /// Queue `event` in the ring, or in the wait queue when the ring is full. Both
    /// keep its address: it stays suspended in its operation until its completion,
    /// so every request the ring holds is an event suspended in its operation.
    fn dispatch(&self, event: &Event) {
        let space = self.sq_space_left();
        if space == 0 {
            let mut waiting = self.waiting.take();
            waiting.push_back(event);
            self.waiting.set(waiting);
            return;
        }
        if event.timeout.is_some() && space < 2 {
            (self.host.timeout_full)(self.fd());
        }
        // User data identifies the event on completion.
        let entry = event.data.clone().user_data(ptr::from_ref(event) as u64);
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
        self.with_ring(|ring| {
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
        })
        .unwrap_or(0)
    }

    fn sleep(&mut self) {
        _ = os::usleep(10_000);
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
                status: Err(OpenError::Create),
            };
        }
    };
    let status = probe(&ring, config).and_then(|()| {
        if config.workers == 0 {
            return Ok(());
        }
        ring.submitter()
            .register_iowq_max_workers(&mut [config.workers; 2])
            .map_err(|error| {
                set_errno(error.raw_os_error().unwrap_or(0));
                OpenError::Workers
            })
    });
    let engine = Uring {
        fd: ring.as_raw_fd(),
        ring: Cell::new(Some(Box::new(ring))),
        host,
        task_num: Cell::new(0),
        waiting: Cell::default(),
        ready: Cell::new(Vec::with_capacity(QUEUE_SIZE as usize)),
    };
    Opened {
        uring: Some(Rc::new(engine)),
        status,
    }
}

/// Whether the running kernel has the opcodes this build was configured with.
fn probe(ring: &IoUring, config: Config) -> Result<(), OpenError> {
    if !config.probe_futex && !config.probe_ftruncate {
        return Ok(());
    }
    let mut probe = Probe::new();
    if ring.submitter().register_probe(&mut probe).is_err() {
        return Err(OpenError::Probe);
    }
    if config.probe_futex
        && !(probe.is_supported(opcode::FutexWait::CODE)
            && probe.is_supported(opcode::FutexWake::CODE))
    {
        Err(OpenError::Futex)
    } else if config.probe_ftruncate && !probe.is_supported(opcode::Ftruncate::CODE) {
        Err(OpenError::Ftruncate)
    } else {
        Ok(())
    }
}

impl Uring {
    #[must_use]
    pub fn fd(&self) -> c_int {
        self.fd
    }

    /// Requests in flight.
    #[must_use]
    pub fn task_num(&self) -> u32 {
        self.task_num.get()
    }

    /// Requests waiting for room in the ring.
    #[must_use]
    pub fn waiting_num(&self) -> usize {
        let waiting = self.waiting.take();
        let len = waiting.len();
        self.waiting.set(waiting);
        len
    }

    #[must_use]
    pub fn sq_usage_percent(&self) -> f32 {
        let (used, capacity) = self.sq();
        used.c_cast::<f32>() / capacity.max(1).c_cast::<f32>() * 100.0
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
        let harvested = engine.with_ring(|ring| {
            let mut harvested = false;
            for cqe in ring.completion().take(QUEUE_SIZE as usize) {
                harvested = true;
                if cqe.user_data() != TIMEOUT_EVENT {
                    ready.push((cqe.user_data() as *const Event, cqe.result() as isize));
                }
            }
            harvested
        });
        if harvested != Some(true) {
            engine.ready.set(ready);
            return true;
        }
        for _ in 0..ready.len() {
            if engine.sq_space_left() == 0 {
                break;
            }
            let mut queue = engine.waiting.take();
            let waiting = queue.pop_front();
            engine.waiting.set(queue);
            let Some(waiting) = waiting else {
                break;
            };
            // SAFETY: a queued event is suspended in its operation, and nothing
            // else borrows it.
            engine.dispatch(unsafe { &*waiting });
        }
        for &(event, result) in &ready {
            let waker = {
                // SAFETY: user data is an event suspended in its operation, which
                // only shares it until its coroutine resumes.
                let event = unsafe { &*event };
                event.result.set(complete(result));
                event.waker.get()
            };
            engine.task_num.set(engine.task_num.get() - 1);
            // The coroutine suspended in that event's operation, which may re-enter
            // the engine.
            if let Some(waker) = waker {
                (engine.host.resume)(waker);
            }
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
    matches!(Event::new(host, cancel).execute(host), Ok(0))
}

/// One `io_uring` request, end to end.
///
/// # Safety
/// The entry's pointers stay valid until the request completes, which is before
/// this returns.
unsafe fn request(host: &Host, timeout: f64, entry: squeue::Entry) -> io::Result<isize> {
    Event::new(host, entry).with_timeout(timeout).execute(host)
}

fn byte_count(result: io::Result<isize>) -> io::Result<usize> {
    // Successful CQE values are nonnegative; complete rejects every negative one.
    result.map(isize::cast_unsigned)
}

fn file_status(result: io::Result<isize>) -> io::Result<()> {
    result.map(|_| ())
}

/// One request per operation, whose entries carry no pointer but their borrowed
/// arguments'.
macro_rules! safe_operations {
    ($(
        $(#[$doc:meta])*
        $vis:vis fn $name:ident($($arg:ident: $ty:ty),*) -> $ret:ty, timeout $timeout:expr, $entry:expr, $convert:expr;
    )*) => {$(
        $(#[$doc])*
        ///
        /// # Errors
        /// Failure sets errno; typed operations also retain it in their Result.
        #[must_use = "check the operation result"]
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
    pub fn recv(fd: c_int, buf: &mut [impl ReadSlot], flags: c_int, timeout: f64) -> io::Result<usize>,
        timeout timeout, opcode::Recv::new(Fd(fd), buf.as_mut_ptr().cast(), buf.len().c_cast::<u32>()).flags(flags), byte_count;
    pub fn send(fd: c_int, buf: &[u8], flags: c_int, timeout: f64) -> io::Result<usize>,
        timeout timeout, opcode::Send::new(Fd(fd), buf.as_ptr(), buf.len().c_cast::<u32>()).flags(flags), byte_count;
    pub fn close_fd(fd: c_int) -> io::Result<()>,
        timeout -1.0, opcode::Close::new(Fd(fd)), file_status;
    pub fn read(fd: c_int, buf: &mut [impl ReadSlot], timeout: f64) -> io::Result<usize>,
        timeout timeout, opcode::Read::new(Fd(fd), buf.as_mut_ptr().cast(), buf.len().c_cast::<c_uint>()).offset(u64::MAX), byte_count;
    pub fn write(fd: c_int, buf: &[u8], timeout: f64) -> io::Result<usize>,
        timeout timeout, opcode::Write::new(Fd(fd), buf.as_ptr(), buf.len().c_cast::<c_uint>()).offset(u64::MAX), byte_count;
    pub fn futex_wait(futex: &AtomicU32) -> io::Result<c_int>,
        timeout -1.0, opcode::FutexWait::new(futex.as_ptr(), 1, FUTEX_BITSET_MATCH_ANY, FUTEX2_SIZE_U32), |result: io::Result<isize>| result.map(CCast::c_cast::<c_int>);
    pub fn futex_wakeup(futex: &AtomicU32) -> io::Result<c_int>,
        timeout -1.0, opcode::FutexWake::new(futex.as_ptr(), 1, FUTEX_BITSET_MATCH_ANY, FUTEX2_SIZE_U32), |result: io::Result<isize>| result.map(CCast::c_cast::<c_int>);
    /// `IORING_OP_TIMEOUT` for `ts`.
    fn timer(ts: &Timespec) -> io::Result<()>,
        timeout -1.0, opcode::Timeout::new(ts), file_status;
    /// `IORING_OP_STATX` of `path` under `dfd` into `buf`.
    fn statx_into(dfd: c_int, path: &CStr, flags: c_int, buf: &mut libc::statx) -> io::Result<()>,
        timeout -1.0, opcode::Statx::new(Fd(dfd), path.as_ptr(), ptr::from_mut(buf).cast()).flags(flags).mask(libc::STATX_BASIC_STATS | libc::STATX_BTIME), file_status;
    /// `IORING_OP_WAITID` into `info`.
    fn waitid(idtype: libc::idtype_t, id: libc::id_t, options: c_int, info: &mut ChildInfo, timeout: f64) -> io::Result<()>,
        timeout timeout, opcode::WaitId::new(idtype, id, options).infop(ptr::from_mut(info).cast()), file_status;
    /// `IORING_OP_ACCEPT` on `fd`: the peer's address into `address`'s storage, at most
    /// `address.len` bytes of it (and no more than it holds), and its length into `address.len`.
    pub fn accept(fd: c_int, address: &mut SocketAddress, flags: c_int, timeout: f64) -> io::Result<c_int>,
        timeout timeout, {
            address.len = address.len.min(SocketAddress::CAPACITY);
            opcode::Accept::new(Fd(fd), (&raw mut address.addr).cast(), &raw mut address.len).flags(flags)
        }, |result: io::Result<isize>| result.map(CCast::c_cast::<c_int>);
    /// `IORING_OP_SEND` of `buf` to `address`, of which the kernel reads at most its storage.
    pub fn sendto(fd: c_int, buf: &[u8], address: &SocketAddress, timeout: f64) -> io::Result<usize>,
        timeout timeout, opcode::Send::new(Fd(fd), buf.as_ptr(), buf.len().c_cast::<u32>())
            .dest_addr((&raw const address.addr).cast())
            .dest_addr_len(address.len.min(SocketAddress::CAPACITY)), byte_count;
    /// `IORING_OP_CONNECT` of `fd` to `address`, of which the kernel reads at most its storage.
    pub fn connect(fd: c_int, address: &SocketAddress, timeout: f64) -> io::Result<()>,
        timeout timeout, opcode::Connect::new(
            Fd(fd),
            (&raw const address.addr).cast(),
            address.len.min(SocketAddress::CAPACITY),
        ), file_status;
    /// `IORING_OP_RECVMSG` into `message`'s buffers.
    pub fn recvmsg(fd: c_int, message: &mut RecvMessage<'_>, flags: c_int, timeout: f64) -> io::Result<usize>,
        timeout timeout, opcode::RecvMsg::new(Fd(fd), message.as_mut_ptr()).flags(flags.cast_unsigned()), byte_count;
    /// `IORING_OP_SENDMSG` of `message`'s buffers.
    pub fn sendmsg(fd: c_int, message: &SendMessage<'_>, flags: c_int, timeout: f64) -> io::Result<usize>,
        timeout timeout, opcode::SendMsg::new(Fd(fd), message.as_ptr()).flags(flags.cast_unsigned()), byte_count;
    /// `IORING_OP_POLL_ADD` of `fd`, which carries no pointer.
    fn poll_add(fd: c_int, events: u32, timeout: f64) -> io::Result<c_int>,
        timeout timeout, opcode::PollAdd::new(Fd(fd), events), |result: io::Result<isize>| result.map(CCast::c_cast::<c_int>);
    pub fn open_file(pathname: &CStr, flags: c_int, mode: libc::mode_t) -> io::Result<c_int>,
        timeout -1.0, opcode::OpenAt::new(Fd(libc::AT_FDCWD), pathname.as_ptr()).flags(flags | libc::O_CLOEXEC).mode(mode), |result: io::Result<isize>| result.map(CCast::c_cast::<c_int>);
    pub fn rename(oldpath: &CStr, newpath: &CStr) -> io::Result<()>,
        timeout -1.0, opcode::RenameAt::new(Fd(libc::AT_FDCWD), oldpath.as_ptr(), Fd(libc::AT_FDCWD), newpath.as_ptr()), file_status;
    pub fn mkdir(pathname: &CStr, mode: libc::mode_t) -> io::Result<()>,
        timeout -1.0, opcode::MkDirAt::new(Fd(libc::AT_FDCWD), pathname.as_ptr()).mode(mode), file_status;
    pub fn unlink(pathname: &CStr) -> io::Result<()>,
        timeout -1.0, opcode::UnlinkAt::new(Fd(libc::AT_FDCWD), pathname.as_ptr()), file_status;
    pub fn rmdir(pathname: &CStr) -> io::Result<()>,
        timeout -1.0, opcode::UnlinkAt::new(Fd(libc::AT_FDCWD), pathname.as_ptr()).flags(libc::AT_REMOVEDIR), file_status;
    pub fn fsync(fd: c_int) -> io::Result<()>,
        timeout -1.0, opcode::Fsync::new(Fd(fd)), file_status;
    pub fn fdatasync(fd: c_int) -> io::Result<()>,
        timeout -1.0, opcode::Fsync::new(Fd(fd)).flags(FsyncFlags::DATASYNC), file_status;
    pub fn ftruncate(fd: c_int, length: libc::off_t) -> io::Result<()>,
        timeout -1.0, opcode::Ftruncate::new(Fd(fd), length.cast_unsigned()), file_status;
}

/// `IORING_OP_READV` into `vector`'s remaining buffers, or `IORING_OP_WRITEV` from
/// them, as the vector was made for.
/// # Errors
/// The completion's captured errno, including linked timeouts.
#[must_use = "check the transfer result"]
pub fn vector(host: &Host, fd: c_int, vector: &IoVector<'_>, timeout: f64) -> io::Result<usize> {
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
    // (the type's invariant); the request completes before `request` returns.
    let result = unsafe { request(host, timeout, entry) };
    byte_count(result)
}

/// Sleep `seconds` in the ring's timer.
/// # Errors
/// The ambient errno after the timer call, unless it is `ETIME`. This preserves
/// upstream's sleep contract, which ignores the request's result.
pub fn sleep(host: &Host, seconds: f64) -> io::Result<()> {
    sleep_timespec(host, timespec(seconds))
}

/// Upstream's `sleep(tv_sec, tv_nsec)`, which `sleep(seconds)` converts to.
fn sleep_timespec(host: &Host, ts: libc::timespec) -> io::Result<()> {
    let _expired = timer(host, &kernel_timespec(ts));
    if errno() == libc::ETIME {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// `size` bytes of `in_fd` from `offset` (the file position without one) to
/// `out_fd`, spliced through a pipe. Upstream's zero-copy sendmsg needs
/// `MSG_SPLICE_PAGES`, which is kernel-internal: no build's headers have it.
/// # Errors
/// Pipe creation or a splice failed before any output was sent. A zero-byte
/// splice also fails with the ambient errno, matching upstream.
pub fn sendfile(
    host: &Host,
    out_fd: c_int,
    in_fd: c_int,
    offset: Option<&mut libc::off_t>,
    size: usize,
    timeout: f64,
) -> io::Result<usize> {
    if size == 0 {
        return Ok(0);
    }
    let [read_end, write_end] = os::pipe().ok_or_else(io::Error::last_os_error)?;
    let _resized = os::fcntl(write_end, libc::F_SETPIPE_SZ, 1024 * 1024);
    let pipe_size = os::fcntl(read_end, libc::F_GETPIPE_SZ, 0).unwrap_or(-1);
    let pipe_size = usize::try_from(pipe_size).unwrap_or(65536);
    // Each splice completes before the next, and carries no pointers.
    let splice = |fd_in, off_in, fd_out, nbytes| {
        let splice = opcode::Splice::new(Fd(fd_in), off_in, Fd(fd_out), -1, nbytes).build();
        byte_count(Event::new(host, splice).with_timeout(timeout).execute(host))
    };
    let result = splice_copy(
        size,
        pipe_size,
        offset.as_deref().copied(),
        |from, count| splice(in_fd, from, write_end, count),
        |count| splice(read_end, -1, out_fd, count),
    );
    let saved = errno();
    _ = os::close(read_end);
    _ = os::close(write_end);
    if result.is_err() {
        set_errno(saved);
    }
    if let Ok(total) = result
        && total > 0
        && let Some(offset) = offset
    {
        *offset += libc::off_t::try_from(total).unwrap_or(libc::off_t::MAX);
    }
    result
}

/// Splice through the pipe in pairs, retaining completed output on a later error.
fn splice_copy(
    size: usize,
    pipe_size: usize,
    start: Option<libc::off_t>,
    mut read: impl FnMut(i64, c_uint) -> io::Result<usize>,
    mut write: impl FnMut(c_uint) -> io::Result<usize>,
) -> io::Result<usize> {
    let mut total = 0_usize;
    while total < size {
        let from = start.map_or(-1, |start| start + i64::try_from(total).unwrap_or(i64::MAX));
        let count = c_uint::try_from((size - total).min(pipe_size)).unwrap_or(c_uint::MAX);
        let next = (|| {
            let input = read(from, count)?;
            if input == 0 {
                return Err(io::Error::last_os_error());
            }
            let output = write(c_uint::try_from(input).unwrap_or(c_uint::MAX))?;
            if output == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok((input, output))
        })();
        match next {
            Ok((input, output)) => {
                total += output;
                if output < input {
                    break;
                }
            }
            Err(error) if total == 0 => return Err(error),
            Err(_) => break,
        }
    }
    Ok(total)
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

fn statx(
    host: &Host,
    dfd: c_int,
    path: &CStr,
    flags: c_int,
    stat: &mut libc::stat,
) -> io::Result<()> {
    let mut buf: libc::statx = swoole_rs_corebase::sys::zeroed();
    statx_into(host, dfd, path, flags, &mut buf)?;
    statx_to_stat(&buf, stat);
    Ok(())
}

/// `fstat(2)` of `fd` into `stat`.
/// # Errors
/// The ring could not read the file metadata; `stat` is unchanged.
pub fn fstat(host: &Host, fd: c_int, stat: &mut libc::stat) -> io::Result<()> {
    statx(host, fd, c"", libc::AT_EMPTY_PATH, stat)
}

/// `stat(2)` of `path` into `stat`; upstream's ring `lstat` is this too.
/// # Errors
/// The ring could not read the file metadata; `stat` is unchanged.
pub fn stat(host: &Host, path: &CStr, stat: &mut libc::stat) -> io::Result<()> {
    statx(host, libc::AT_FDCWD, path, libc::AT_SYMLINK_NOFOLLOW, stat)
}

/// `waitpid` through `IORING_OP_WAITID`; `WNOHANG` stays a plain waitpid(2).
/// # Errors
/// The wait failed; the status word is unchanged.
pub fn waitpid(
    host: &Host,
    pid: libc::pid_t,
    stat_loc: Option<&mut c_int>,
    options: c_int,
    timeout: f64,
) -> io::Result<libc::pid_t> {
    if options & libc::WNOHANG != 0 {
        return os::waitpid(pid, stat_loc, options).map(|child| child.unwrap_or(0));
    }
    let mut info = ChildInfo::default();
    let (idtype, id) = if pid > 0 {
        (libc::P_PID, pid.cast_unsigned())
    } else {
        (libc::P_ALL, 0)
    };
    let options = if options == 0 { libc::WEXITED } else { options };
    waitid(host, idtype, id, options, &mut info, timeout)?;
    // The kernel filled `info` for a waited child.
    let [_, _, code] = info.head;
    if let Some(stat_loc) = stat_loc {
        *stat_loc = wait_status(code, info.child.status);
    }
    Ok(info.child.pid)
}

/// poll(2) of one descriptor; `timeout` 0 polls without the ring.
/// # Errors
/// The poll failed, including a linked timeout expiring.
pub fn poll(host: &Host, pollfd: &mut libc::pollfd, timeout: c_int) -> io::Result<c_int> {
    if timeout == 0 {
        return os::poll(core::slice::from_mut(pollfd), timeout);
    }
    let (descriptor, events) = (pollfd.fd, pollfd.events);
    let rc = poll_add(
        host,
        descriptor,
        events.cast_unsigned().into(),
        f64::from(timeout) / 1000.0,
    )?;
    if rc > 0 {
        let revents = rc.c_cast::<libc::c_short>();
        pollfd.revents = revents;
        return Ok(1);
    }
    Ok(rc)
}

#[cfg(test)]
mod ported;

/// A ring host for tests, shared with the TLS transport's.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use std::cell::RefCell;

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
    fn suspend(_coroutine: *mut c_void, _event: *mut c_void, _waker: &Cell<Option<Waker>>) {
        let engine = instance().expect("a test engine");
        engine.submit_queued();
        while engine.task_num() > 0 {
            let _waited = engine.with_ring(|ring| ring.submit_and_wait(1));
            wakeup(&engine);
        }
    }
    fn resume(_waker: Waker) {}
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
        assert_eq!(opened.status, Ok(()), "io_uring is available");
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
    fn wait_creation_failure_retains_error_and_outputs() {
        let host = Host {
            instance: || None,
            ..testing::HOST
        };
        let mut status = 123;
        let mut fd = libc::pollfd {
            fd: -1,
            events: libc::POLLIN,
            revents: libc::POLLOUT,
        };
        let word = AtomicU32::new(1);
        set_errno(libc::EPERM);
        let errors = [
            waitpid(&host, -1, Some(&mut status), 0, 1.0).unwrap_err(),
            poll(&host, &mut fd, 1000).unwrap_err(),
            futex_wait(&host, &word).unwrap_err(),
            futex_wakeup(&host, &word).unwrap_err(),
        ];
        set_errno(libc::EINTR);
        for error in errors {
            assert_eq!(error.raw_os_error(), Some(libc::EPERM));
        }
        assert_eq!(status, 123);
        assert_eq!(fd.revents, libc::POLLOUT);
        assert_eq!(word.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    #[test]
    fn immediate_waits_bypass_the_ring_and_retain_results() {
        let host = Host {
            instance: || panic!("immediate waits must not create a ring"),
            ..testing::HOST
        };
        let mut fd = libc::pollfd {
            fd: -1,
            events: libc::POLLIN,
            revents: libc::POLLOUT,
        };
        assert_eq!(poll(&host, &mut fd, 0).unwrap(), 0);
        assert_eq!(fd.revents, 0);
        let mut status = 123;
        let error = waitpid(
            &host,
            libc::pid_t::MAX,
            Some(&mut status),
            libc::WNOHANG,
            -1.0,
        )
        .unwrap_err();
        assert_eq!(status, 123);
        set_errno(libc::EINTR);
        assert_eq!(error.raw_os_error(), Some(libc::ECHILD));
    }

    #[test]
    fn transfer_creation_errors_retain_errno_and_buffers() {
        let host = Host {
            instance: || {
                set_errno(libc::EPERM);
                None
            },
            ..testing::HOST
        };
        let mut buffer = [7_u8, 8];
        let mut message_buffer = [9_u8, 10];
        let output = [std::io::IoSlice::new(b"xy")];
        let mut input = [std::io::IoSliceMut::new(&mut message_buffer)];
        let message = SendMessage::new(&output);
        let mut received = RecvMessage::new(&mut input);
        let vectors = IoVector::new(crate::vector::Vectors::Write(&output)).unwrap();
        let stopped = vectors.stopped();
        let mut address = SocketAddress {
            len: 16,
            ..SocketAddress::default()
        };
        let errors = [
            read(&host, -1, &mut buffer, -1.0).unwrap_err(),
            recv(&host, -1, &mut buffer, 0, -1.0).unwrap_err(),
            write(&host, -1, b"xy", -1.0).unwrap_err(),
            send(&host, -1, b"xy", 0, -1.0).unwrap_err(),
            sendto(&host, -1, b"xy", &address, -1.0).unwrap_err(),
            sendmsg(&host, -1, &message, 0, -1.0).unwrap_err(),
            recvmsg(&host, -1, &mut received, 0, -1.0).unwrap_err(),
            vector(&host, -1, &vectors, -1.0).unwrap_err(),
            accept(&host, -1, &mut address, 0, -1.0).unwrap_err(),
            connect(&host, -1, &address, -1.0).unwrap_err(),
        ];
        set_errno(libc::EINTR);
        for error in errors {
            assert_eq!(error.raw_os_error(), Some(libc::EPERM));
        }
        assert_eq!(buffer, [7, 8]);
        assert_eq!(message_buffer, [9, 10]);
        assert_eq!(vectors.stopped(), stopped);
        assert_eq!(address.len, 16);
    }

    #[test]
    fn byte_counts_keep_eof_partial_success_and_completion_errors() {
        set_errno(libc::EINTR);
        assert_eq!(byte_count(complete(0)).unwrap(), 0);
        assert_eq!(byte_count(complete(3)).unwrap(), 3);
        assert_eq!(errno(), libc::EINTR);
        let error = byte_count(complete(-(libc::ECANCELED as isize))).unwrap_err();
        set_errno(libc::EBADF);
        assert_eq!(error.raw_os_error(), Some(libc::ETIMEDOUT));
    }

    #[test]
    fn filesystem_creation_failure_retains_error() {
        let host = Host {
            instance: || None,
            ..testing::HOST
        };
        set_errno(libc::EPERM);
        let error = open_file(&host, c"unused", libc::O_RDONLY, 0).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::EPERM));
        let mut metadata: libc::stat = os::zeroed();
        metadata.st_size = 123;
        let error = fstat(&host, -1, &mut metadata).unwrap_err();
        assert_eq!(metadata.st_size, 123);
        set_errno(libc::EINTR);
        assert_eq!(error.raw_os_error(), Some(libc::EPERM));
    }

    #[test]
    fn filesystem_errors_retain_errno_and_leave_stat_untouched() {
        testing::with_engine(|| {
            set_errno(0);
            let error = fsync(&testing::HOST, -1).unwrap_err();
            assert_eq!(error.raw_os_error(), Some(libc::EBADF));
            assert_eq!(errno(), libc::EBADF);
            let mut metadata: libc::stat = os::zeroed();
            metadata.st_size = 123;
            metadata.st_mode = 0o640;
            let error = fstat(&testing::HOST, -1, &mut metadata).unwrap_err();
            assert_eq!(error.raw_os_error(), Some(libc::EBADF));
            assert_eq!(metadata.st_size, 123);
            assert_eq!(metadata.st_mode, 0o640);
            set_errno(libc::EINTR);
            assert_eq!(error.raw_os_error(), Some(libc::EBADF));
        });
    }

    #[test]
    fn completion_error_survives_errno_changes() {
        for (completion, expected) in [
            (-libc::EBADF, libc::EBADF),
            (-libc::ECANCELED, libc::ETIMEDOUT),
            (-libc::ETIME, libc::ETIME),
        ] {
            let result = complete(completion as isize);
            assert_eq!(errno(), expected);
            set_errno(libc::EINTR);
            assert_eq!(
                file_status(result).unwrap_err().raw_os_error(),
                Some(expected)
            );
        }
        set_errno(libc::EINTR);
        assert_eq!(complete(0).unwrap(), 0);
        assert_eq!(complete(123).unwrap(), 123);
        assert_eq!(errno(), libc::EINTR);
    }

    #[test]
    fn completion_maps_cancel_to_timeout() {
        assert_eq!(complete(5).unwrap_or(-1), 5, "success passes through");
        assert_eq!(
            complete(-(libc::ECANCELED as isize)).unwrap_or(-1),
            -1,
            "failure is -1"
        );
        assert_eq!(errno(), libc::ETIMEDOUT, "cancelled reads as a timeout");
        assert_eq!(
            complete(-(libc::EBADF as isize)).unwrap_or(-1),
            -1,
            "failure is -1"
        );
        assert_eq!(errno(), libc::EBADF, "other errors pass through");
    }

    #[test]
    fn sleep_retains_the_ambient_errno_contract() {
        let host = Host {
            instance: || None,
            ..testing::HOST
        };
        for code in [libc::ETIME, 0, libc::EPERM, libc::ETIMEDOUT] {
            set_errno(code);
            let result = sleep(&host, 0.1);
            assert_eq!(errno(), code);
            set_errno(libc::EINTR);
            if code == libc::ETIME {
                assert!(result.is_ok());
            } else {
                assert_eq!(result.unwrap_err().raw_os_error(), Some(code));
            }
        }
    }

    #[test]
    fn sendfile_empty_and_creation_failure_leave_offset_unchanged() {
        let host = Host {
            instance: || {
                set_errno(libc::EPERM);
                None
            },
            ..testing::HOST
        };
        let mut offset = 9;
        set_errno(libc::EINTR);
        assert_eq!(
            sendfile(&host, -1, -1, Some(&mut offset), 0, 1.0).unwrap(),
            0
        );
        assert_eq!(errno(), libc::EINTR);
        let error = sendfile(&host, -1, -1, Some(&mut offset), 1, 1.0).unwrap_err();
        assert_eq!(errno(), libc::EPERM);
        set_errno(libc::EBADF);
        assert_eq!(error.raw_os_error(), Some(libc::EPERM));
        assert_eq!(offset, 9);
    }

    #[test]
    fn splice_copy_retains_chunk_offsets_and_short_output() {
        let mut reads = Vec::new();
        let mut writes = Vec::new();
        let copied = splice_copy(
            9,
            4,
            Some(7),
            |from, count| {
                reads.push((from, count));
                Ok(count as usize)
            },
            |count| {
                writes.push(count);
                Ok(count as usize)
            },
        )
        .unwrap();
        assert_eq!(copied, 9);
        assert_eq!(reads, [(7, 4), (11, 4), (15, 1)]);
        assert_eq!(writes, [4, 4, 1]);
        let mut calls = 0;
        assert_eq!(
            splice_copy(
                9,
                4,
                None,
                |from, count| {
                    calls += 1;
                    assert_eq!((from, count), (-1, 4));
                    Ok(4)
                },
                |count| {
                    assert_eq!(count, 4);
                    Ok(3)
                }
            )
            .unwrap(),
            3
        );
        assert_eq!(calls, 1);
    }

    #[test]
    fn splice_copy_preserves_first_errors_and_later_progress() {
        for write_fails in [false, true] {
            for first in [false, true] {
                for eof in [false, true] {
                    let mut read_calls = 0;
                    let mut write_calls = 0;
                    let failure = || {
                        set_errno(libc::EIO);
                        if eof {
                            Ok(0)
                        } else {
                            Err(io::Error::last_os_error())
                        }
                    };
                    let result = splice_copy(
                        8,
                        4,
                        None,
                        |_, _| {
                            read_calls += 1;
                            if !write_fails && (first || read_calls > 1) {
                                failure()
                            } else {
                                Ok(4)
                            }
                        },
                        |_| {
                            write_calls += 1;
                            if write_fails && (first || write_calls > 1) {
                                failure()
                            } else {
                                Ok(4)
                            }
                        },
                    );
                    set_errno(libc::EINTR);
                    if first {
                        assert_eq!(result.unwrap_err().raw_os_error(), Some(libc::EIO));
                    } else {
                        assert_eq!(result.unwrap(), 4);
                    }
                    assert_eq!(read_calls, if first { 1 } else { 2 });
                    assert_eq!(
                        write_calls,
                        if write_fails {
                            read_calls
                        } else {
                            read_calls - 1
                        }
                    );
                }
            }
        }
    }
}
