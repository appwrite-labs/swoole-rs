// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/coroutine/iouring.cc (Swoole contributors).
//! Swoole's `io_uring` engine over liburing: the ring, its submission and
//! completion queues, the per-request events, the wait queue for a full ring, and
//! every operation `Iouring` offers. crates/hooks/src/iouring.rs keeps the reactor registration,
//! the coroutine switches and the log lines, reached through [`Host`].
//!
//! An operation runs on the calling coroutine's stack: its [`Event`] lives in the
//! operation's frame while the coroutine is suspended, and the completion handler
//! ([`wakeup`]) writes the result into it before resuming it.

use std::collections::VecDeque;
use std::ffi::{CStr, c_char, c_int, c_uint, c_void};
use std::{mem, ptr};
use swoole_rs_corebase::set_errno;
use swoole_rs_corebase::sys as os;

use swoole_rs_uring_sys as sys;

/// `SW_IOURING_QUEUE_SIZE`: default ring size and completion batch.
const QUEUE_SIZE: u32 = 8192;
/// `SW_IOURING_SQE_BATCH_SIZE`: submissions deferred to the end of the loop below it.
const SQE_BATCH_SIZE: u32 = 16;
/// User data of a linked timeout's own completion, which is ignored.
const TIMEOUT_EVENT: u64 = u64::MAX;
const EVENT_TIMEOUT: c_int = 1;
/// Upstream's `#define FUTEX2_SIZE_U32` and `FUTEX_BITSET_MATCH_ANY`.
const FUTEX2_SIZE_U32: u32 = 0x02;
const FUTEX_BITSET_MATCH_ANY: u64 = 0xffff_ffff;

/// What the engine needs from the coroutine scheduler, the reactor and the log.
pub struct Host {
    /// `Coroutine::get_current_safe()`.
    pub current: extern "C" fn() -> *mut c_void,
    /// The thread's engine, created on first use (null if it failed).
    pub instance: fn() -> *mut Uring,
    /// Suspend `coroutine` until [`wakeup`] resumes it; cancelling it calls
    /// [`cancel`] with `event`.
    pub suspend: unsafe fn(coroutine: *mut c_void, event: *mut c_void),
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

/// [`open`]'s outcome. Status 0 ready; 1 the loaded liburing is older
/// than the one built against (versions in the other fields); 2 the ring could not
/// be created (errno set, no engine); 3 the opcode probe failed; 4 no futex
/// opcodes; 5 no ftruncate opcode; 6 the async worker limit failed (errno set).
#[repr(C)]
#[derive(Debug)]
pub struct Opened {
    pub uring: *mut Uring,
    pub status: c_int,
    pub major: c_int,
    pub minor: c_int,
    pub runtime_major: c_int,
    pub runtime_minor: c_int,
}

struct Event {
    coroutine: *mut c_void,
    data: sys::io_uring_sqe,
    result: isize,
    timeout: sys::__kernel_timespec,
    flags: c_int,
}

pub struct Uring {
    ring: sys::io_uring,
    host: &'static Host,
    task_num: u32,
    waiting: VecDeque<*mut Event>,
    cqes: Vec<*mut sys::io_uring_cqe>,
    /// Completed events and their results.
    ready: Vec<(*mut Event, isize)>,
}

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// Upstream's `DOUBLE_TO_TIMESPEC`.
fn timespec(seconds: f64) -> sys::__kernel_timespec {
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
    sys::__kernel_timespec { tv_sec, tv_nsec }
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

/// An all-zero sqe: the state every `io_uring_prep_*` starts from.
fn zeroed_sqe() -> sys::io_uring_sqe {
    // SAFETY: the sqe is integers and unions of them, for which all-zero is valid.
    unsafe { mem::zeroed() }
}

impl Event {
    /// Upstream's `INIT_EVENT`: the current coroutine, or a fatal error outside one.
    fn new(host: &Host) -> Self {
        Self {
            coroutine: (host.current)(),
            data: zeroed_sqe(),
            result: 0,
            timeout: sys::__kernel_timespec {
                tv_sec: 0,
                tv_nsec: 0,
            },
            flags: 0,
        }
    }

    fn with_timeout(mut self, seconds: f64) -> Self {
        if seconds > 0.0 {
            self.flags |= EVENT_TIMEOUT;
            self.timeout = timespec(seconds);
        }
        self
    }

    fn sqe(&mut self) -> *mut sys::io_uring_sqe {
        &raw mut self.data
    }

    /// Submit and suspend until the completion; the result as upstream's `execute`.
    fn execute(&mut self, host: &Host) -> isize {
        let uring = (host.instance)();
        if uring.is_null() {
            return -1;
        }
        let (coroutine, event) = (self.coroutine, ptr::from_mut(self).cast());
        {
            // SAFETY: the thread's live engine; the borrow ends before the switch,
            // which may re-enter it.
            let engine = unsafe { &mut *uring };
            engine.dispatch(self);
            engine.task_num += 1;
        }
        // SAFETY: suspends this coroutine; the wakeup handler resumes it, and the
        // event stays in this frame until then.
        unsafe { (host.suspend)(coroutine, event) };
        self.result
    }
}

impl Uring {
    /// A free submission slot, or null when the ring is full.
    fn get_sqe(&mut self) -> *mut sys::io_uring_sqe {
        // SAFETY: the ring is initialized.
        unsafe { sys::swr_uring_get_sqe(&raw mut self.ring) }
    }

    /// Queue `event` in the ring, or in the wait queue when the ring is full. Both
    /// keep its address: it stays suspended in its operation until its completion.
    fn dispatch(&mut self, event: &mut Event) {
        let sqe = self.get_sqe();
        if sqe.is_null() {
            self.waiting.push_back(event);
            return;
        }
        // SAFETY: a free slot of this ring, which nothing else refers to.
        let sqe = unsafe { &mut *sqe };
        // SAFETY: the sqe is plain integers, so a bitwise copy is a valid one.
        *sqe = unsafe { ptr::read(&raw const event.data) };
        // User data identifies the event on completion.
        sqe.user_data = ptr::from_mut(event) as u64;
        if event.flags & EVENT_TIMEOUT != 0 {
            let timeout_sqe = self.get_sqe();
            if timeout_sqe.is_null() {
                (self.host.timeout_full)(self.ring.ring_fd);
            } else {
                // SAFETY: another free slot of this ring.
                let timeout_sqe = unsafe { &mut *timeout_sqe };
                *timeout_sqe = zeroed_sqe();
                // SAFETY: the timeout lives in the event, valid until its completion.
                unsafe {
                    sys::swr_uring_prep_link_timeout(timeout_sqe, &raw mut event.timeout, 0);
                }
                timeout_sqe.user_data = TIMEOUT_EVENT;
                sqe.flags |= 1 << sys::io_uring_sqe_flags_bit_IOSQE_IO_LINK_BIT;
            }
        }
        submit(self, false);
    }
}

fn sq_space_left(engine: &Uring) -> u32 {
    // SAFETY: the ring is initialized.
    unsafe { sys::swr_uring_sq_space_left(&raw const engine.ring) }
}

fn sq_used(engine: &Uring) -> u32 {
    engine.ring.sq.ring_entries - sq_space_left(engine)
}

/// The ring's side of a submit, so its order can be tested without one.
trait Submitter {
    fn event_running(&mut self) -> bool;
    fn sq_used(&mut self) -> u32;
    fn submit(&mut self) -> c_int;
    fn sleep(&mut self);
    fn failed(&mut self);
}

impl Submitter for Uring {
    fn event_running(&mut self) -> bool {
        (self.host.event_running)()
    }

    fn sq_used(&mut self) -> u32 {
        sq_used(self)
    }

    fn submit(&mut self) -> c_int {
        // SAFETY: the ring is initialized.
        unsafe { sys::io_uring_submit(&raw mut self.ring) }
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

/// Create the thread's engine: check the loaded liburing, size and create the
/// ring, probe the opcodes this build requires, and cap the async workers.
#[must_use]
pub fn open(config: Config, host: &'static Host) -> Opened {
    let mut opened = Opened {
        uring: ptr::null_mut(),
        status: 0,
        major: sys::IO_URING_VERSION_MAJOR.cast_signed(),
        minor: sys::IO_URING_VERSION_MINOR.cast_signed(),
        // SAFETY: liburing's version queries have no preconditions.
        runtime_major: unsafe { sys::io_uring_major_version() },
        // SAFETY: as above.
        runtime_minor: unsafe { sys::io_uring_minor_version() },
    };
    if opened.runtime_major != opened.major || opened.runtime_minor < opened.minor {
        opened.status = 1;
        return opened;
    }
    let mut engine = Box::new(Uring {
        // SAFETY: io_uring_queue_init initializes every field it uses.
        ring: unsafe { mem::zeroed() },
        host,
        task_num: 0,
        waiting: VecDeque::new(),
        cqes: vec![ptr::null_mut(); QUEUE_SIZE as usize],
        ready: Vec::with_capacity(QUEUE_SIZE as usize),
    });
    let flags = if config.flag == sys::IORING_SETUP_SQPOLL {
        sys::IORING_SETUP_SQPOLL
    } else {
        0
    };
    // SAFETY: `engine.ring` is a ring for liburing to initialize.
    let ret = unsafe {
        sys::io_uring_queue_init(ring_entries(config.entries), &raw mut engine.ring, flags)
    };
    if ret < 0 {
        set_errno(-ret);
        opened.status = 2;
        return opened;
    }
    opened.status = probe(&mut engine, config);
    if opened.status == 0 && config.workers > 0 {
        let mut workers = [config.workers, config.workers];
        // SAFETY: the ring is initialized; `workers` holds the two limits.
        let ret = unsafe {
            sys::io_uring_register_iowq_max_workers(&raw mut engine.ring, workers.as_mut_ptr())
        };
        if ret < 0 {
            set_errno(-ret);
            opened.status = 6;
        }
    }
    opened.uring = Box::into_raw(engine);
    opened
}

/// Whether the running kernel has the opcodes this build was configured with.
fn probe(engine: &mut Uring, config: Config) -> c_int {
    if !config.probe_futex && !config.probe_ftruncate {
        return 0;
    }
    // SAFETY: the ring is initialized.
    let probe = unsafe { sys::io_uring_get_probe_ring(&raw mut engine.ring) };
    if probe.is_null() {
        return 3;
    }
    #[expect(
        clippy::cast_possible_wrap,
        reason = "opcode numbers are small enum values"
    )]
    // SAFETY: `probe` is the live probe just returned.
    let supported = |op: u32| unsafe { sys::swr_uring_opcode_supported(probe, op as c_int) } != 0;
    let status = if config.probe_futex
        && !(supported(sys::io_uring_op_IORING_OP_FUTEX_WAIT)
            && supported(sys::io_uring_op_IORING_OP_FUTEX_WAKE))
    {
        4
    } else if config.probe_ftruncate && !supported(sys::io_uring_op_IORING_OP_FTRUNCATE) {
        5
    } else {
        0
    };
    // SAFETY: frees the probe returned above, once.
    unsafe { sys::io_uring_free_probe(probe) };
    status
}

/// Destroy an engine from [`open`].
///
/// # Safety
/// `uring` came from [`open`], has no request in flight, and is not used
/// again.
pub unsafe fn close(uring: *mut Uring) {
    // SAFETY: the caller's contract.
    let mut engine = unsafe { Box::from_raw(uring) };
    // SAFETY: the ring was initialized by `open`.
    unsafe { sys::io_uring_queue_exit(&raw mut engine.ring) };
}

impl Uring {
    #[must_use]
    pub fn fd(&self) -> c_int {
        self.ring.ring_fd
    }

    /// Requests in flight.
    #[must_use]
    pub fn task_num(&self) -> u32 {
        self.task_num
    }

    /// Requests waiting for room in the ring.
    #[must_use]
    pub fn waiting_num(&self) -> usize {
        self.waiting.len()
    }

    #[must_use]
    pub fn sq_usage_percent(&self) -> f32 {
        #[expect(
            clippy::cast_precision_loss,
            reason = "upstream's (float) used / capacity"
        )]
        let percent = sq_used(self) as f32 / self.ring.sq.ring_entries as f32 * 100.0;
        percent
    }

    /// Submit everything queued: the reactor's end-of-loop callback.
    pub fn submit_queued(&mut self) {
        submit(self, true);
    }
}

/// The ring fd is readable: harvest completions, move waiting requests into the
/// freed slots, then resume each completed request's coroutine.
///
/// # Safety
/// `uring` is a live engine, called from the event loop, not from a coroutine
/// suspended in one of its requests.
pub unsafe fn wakeup(uring: *mut Uring) -> bool {
    loop {
        // Resuming coroutines may re-enter the engine, so the ready list is taken
        // out of it, and no borrow of it is held while they run.
        let mut ready = {
            // SAFETY: the caller's contract.
            let engine = unsafe { &mut *uring };
            // SAFETY: the ring is initialized; `cqes` has room for QUEUE_SIZE.
            let count = unsafe {
                sys::io_uring_peek_batch_cqe(
                    &raw mut engine.ring,
                    engine.cqes.as_mut_ptr(),
                    QUEUE_SIZE,
                )
            };
            if count == 0 {
                return true;
            }
            let mut ready = mem::take(&mut engine.ready);
            for &cqe in engine.cqes.iter().take(count as usize) {
                // SAFETY: liburing filled the first `count` entries with live cqes.
                let cqe = unsafe { &*cqe };
                let (data, res) = (cqe.user_data, cqe.res);
                if data == TIMEOUT_EVENT {
                    continue;
                }
                ready.push((data as *mut Event, res as isize));
            }
            // SAFETY: consumes the `count` cqes read above.
            unsafe { sys::swr_uring_cq_advance(&raw mut engine.ring, count) };
            for _ in 0..ready.len() {
                if sq_space_left(engine) == 0 {
                    break;
                }
                let Some(waiting) = engine.waiting.pop_front() else {
                    break;
                };
                // SAFETY: a queued event is suspended in its operation, and nothing
                // else borrows it.
                engine.dispatch(unsafe { &mut *waiting });
            }
            ready
        };
        for &(event, result) in &ready {
            let coroutine = {
                // SAFETY: user data is an event suspended in its operation, and
                // nothing else borrows it until its coroutine resumes.
                let event = unsafe { &mut *event };
                event.result = complete(result);
                event.coroutine
            };
            let host = {
                // SAFETY: the caller's contract; the borrow ends before the resume.
                let engine = unsafe { &mut *uring };
                engine.task_num -= 1;
                engine.host
            };
            // SAFETY: resumes the suspended coroutine, which may re-enter the engine.
            unsafe { (host.resume)(coroutine) };
        }
        ready.clear();
        // SAFETY: the caller's contract.
        unsafe { (*uring).ready = ready };
    }
}

/// Cancel the request whose event is at `event`, as a coroutine cancelling a
/// suspended one does. The kernel only compares the address with each request's
/// user data.
#[expect(
    clippy::not_unsafe_ptr_arg_deref,
    reason = "`event` is only a key: the kernel compares it, nothing dereferences it"
)]
pub fn cancel(host: &Host, event: *mut c_void) -> bool {
    let mut cancel = Event::new(host);
    // SAFETY: a zeroed sqe; `event` is a key, never dereferenced.
    unsafe { sys::swr_uring_prep_cancel(cancel.sqe(), event, 0) };
    cancel.execute(host) == 0
}

/// One `io_uring` request, end to end: `prep` fills the sqe, with pointers that
/// stay valid until the request completes, which is before this returns.
fn request(host: &Host, timeout: f64, prep: impl FnOnce(&mut sys::io_uring_sqe)) -> isize {
    let mut event = Event::new(host).with_timeout(timeout);
    prep(&mut event.data);
    event.execute(host)
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
        fn $name:ident($($arg:ident: $ty:ty),*) -> $ret:ty, timeout $timeout:expr, |$sqe:ident| $prep:expr, $convert:expr;
    )*) => {$(
        $(#[$doc])*
        ///
        /// # Safety
        /// Pointer arguments are valid as for the system call.
        #[must_use]
        pub unsafe fn $name(host: &Host, $($arg: $ty),*) -> $ret {
            // SAFETY: the caller's contract covers every pointer the sqe carries.
            let result = request(host, $timeout, |$sqe| unsafe { $prep });
            $convert(result)
        }
    )*};
}

operations! {
    fn open_file(pathname: *const c_char, flags: c_int, mode: libc::mode_t) -> c_int,
        timeout -1.0, |sqe| sys::swr_uring_prep_open(sqe, pathname, flags | libc::O_CLOEXEC, mode), int;
    fn connect(fd: c_int, addr: *const libc::sockaddr, len: libc::socklen_t, timeout: f64) -> c_int,
        timeout timeout, |sqe| sys::swr_uring_prep_connect(sqe, fd, addr.cast(), len), int;
    fn accept(fd: c_int, addr: *mut libc::sockaddr, len: *mut libc::socklen_t, flags: c_int, timeout: f64) -> c_int,
        timeout timeout, |sqe| sys::swr_uring_prep_accept(sqe, fd, addr.cast(), len, flags), int;
    fn recv(fd: c_int, buf: *mut c_void, len: usize, flags: c_int, timeout: f64) -> isize,
        timeout timeout, |sqe| sys::swr_uring_prep_recv(sqe, fd, buf, len, flags), std::convert::identity;
    fn send(fd: c_int, buf: *const c_void, len: usize, flags: c_int, timeout: f64) -> isize,
        timeout timeout, |sqe| sys::swr_uring_prep_send(sqe, fd, buf, len, flags), std::convert::identity;
    fn recvmsg(fd: c_int, message: *mut libc::msghdr, flags: c_int, timeout: f64) -> isize,
        timeout timeout, |sqe| sys::swr_uring_prep_recvmsg(sqe, fd, message.cast(), flags.cast_unsigned()), std::convert::identity;
    fn sendmsg(fd: c_int, message: *const libc::msghdr, flags: c_int, timeout: f64) -> isize,
        timeout timeout, |sqe| sys::swr_uring_prep_sendmsg(sqe, fd, message.cast(), flags.cast_unsigned()), std::convert::identity;
    #[expect(clippy::too_many_arguments, reason = "sendto(2)'s six, the timeout and the ring's host")]
    fn sendto(fd: c_int, buf: *const c_void, n: usize, flags: c_int, addr: *const libc::sockaddr, len: libc::socklen_t, timeout: f64) -> isize,
        timeout timeout, |sqe| sys::swr_uring_prep_sendto(sqe, fd, buf, n, flags, addr.cast(), len), std::convert::identity;
    fn readv(fd: c_int, iov: *const libc::iovec, count: c_int, timeout: f64) -> isize,
        timeout timeout, |sqe| sys::swr_uring_prep_readv(sqe, fd, iov.cast(), count.cast_unsigned(), u64::MAX), std::convert::identity;
    fn writev(fd: c_int, iov: *const libc::iovec, count: c_int, timeout: f64) -> isize,
        timeout timeout, |sqe| sys::swr_uring_prep_writev(sqe, fd, iov.cast(), count.cast_unsigned(), u64::MAX), std::convert::identity;
    fn close_fd(fd: c_int) -> c_int,
        timeout -1.0, |sqe| sys::swr_uring_prep_close(sqe, fd), int;
    #[expect(clippy::cast_possible_truncation, reason = "upstream passes size_t to liburing's unsigned nbytes")]
    fn read(fd: c_int, buf: *mut c_void, size: usize, timeout: f64) -> isize,
        timeout timeout, |sqe| sys::swr_uring_prep_read(sqe, fd, buf, size as c_uint, u64::MAX), std::convert::identity;
    #[expect(clippy::cast_possible_truncation, reason = "upstream passes size_t to liburing's unsigned nbytes")]
    fn write(fd: c_int, buf: *const c_void, size: usize, timeout: f64) -> isize,
        timeout timeout, |sqe| sys::swr_uring_prep_write(sqe, fd, buf, size as c_uint, u64::MAX), std::convert::identity;
    fn rename(oldpath: *const c_char, newpath: *const c_char) -> c_int,
        timeout -1.0, |sqe| sys::swr_uring_prep_rename(sqe, oldpath, newpath), int;
    fn mkdir(pathname: *const c_char, mode: libc::mode_t) -> c_int,
        timeout -1.0, |sqe| sys::swr_uring_prep_mkdir(sqe, pathname, mode), int;
    fn unlink(pathname: *const c_char) -> c_int,
        timeout -1.0, |sqe| sys::swr_uring_prep_unlink(sqe, pathname, 0), int;
    fn rmdir(pathname: *const c_char) -> c_int,
        timeout -1.0, |sqe| sys::swr_uring_prep_unlink(sqe, pathname, libc::AT_REMOVEDIR), int;
    fn fsync(fd: c_int) -> c_int,
        timeout -1.0, |sqe| sys::swr_uring_prep_fsync(sqe, fd, 0), int;
    fn fdatasync(fd: c_int) -> c_int,
        timeout -1.0, |sqe| sys::swr_uring_prep_fsync(sqe, fd, sys::IORING_FSYNC_DATASYNC), int;
    fn ftruncate(fd: c_int, length: libc::off_t) -> c_int,
        timeout -1.0, |sqe| sys::swr_uring_prep_ftruncate(sqe, fd, length), int;
    fn futex_wait(futex: *mut u32) -> c_int,
        timeout -1.0, |sqe| sys::swr_uring_prep_futex_wait(sqe, futex, 1, FUTEX_BITSET_MATCH_ANY, FUTEX2_SIZE_U32, 0), int;
    fn futex_wakeup(futex: *mut u32) -> c_int,
        timeout -1.0, |sqe| sys::swr_uring_prep_futex_wake(sqe, futex, 1, FUTEX_BITSET_MATCH_ANY, FUTEX2_SIZE_U32, 0), int;
}

/// Sleep `seconds` in the ring's timer: 0 when it expired, -1 if cancelled.
#[must_use]
pub fn sleep(host: &Host, seconds: f64) -> c_int {
    sleep_timespec(host, timespec(seconds))
}

/// Upstream's `sleep(tv_sec, tv_nsec)`, which `sleep(seconds)` converts to.
fn sleep_timespec(host: &Host, mut ts: sys::__kernel_timespec) -> c_int {
    // SAFETY: `ts` outlives the request.
    request(host, -1.0, |sqe| unsafe {
        sys::swr_uring_prep_timeout(sqe, &raw mut ts, 0, 0);
    });
    if errno() == libc::ETIME { 0 } else { -1 }
}

/// A peek through the ring, then the datagram itself without blocking.
///
/// # Safety
/// Pointers are valid as for recvfrom(2).
pub unsafe fn recvfrom(
    host: &Host,
    fd: c_int,
    buf: *mut c_void,
    n: usize,
    addr: *mut libc::sockaddr,
    len: *mut libc::socklen_t,
    timeout: f64,
) -> isize {
    // SAFETY: the caller's contract.
    let peeked = unsafe { recv(host, fd, buf, n, libc::MSG_PEEK, timeout) };
    if peeked > 0 {
        // SAFETY: the caller's contract.
        unsafe { libc::recvfrom(fd, buf, n, libc::MSG_DONTWAIT, addr, len) }
    } else {
        peeked
    }
}

/// `size` bytes of `in_fd` from `offset` (the file position without one) to
/// `out_fd`. With `splice_pages` (`MSG_SPLICE_PAGES`, when the build headers have
/// it) one zero-copy sendmsg; otherwise splices through a pipe.
#[must_use]
pub fn sendfile(
    host: &Host,
    out_fd: c_int,
    in_fd: c_int,
    offset: Option<&mut libc::off_t>,
    size: usize,
    timeout: f64,
    splice_pages: c_int,
) -> isize {
    if size == 0 {
        return 0;
    }
    if splice_pages != 0 {
        send_spliced(host, out_fd, in_fd, offset, size, timeout, splice_pages)
    } else {
        send_piped(host, out_fd, in_fd, offset, size, timeout)
    }
}

fn send_spliced(
    host: &Host,
    out_fd: c_int,
    in_fd: c_int,
    offset: Option<&mut libc::off_t>,
    size: usize,
    timeout: f64,
    splice_pages: c_int,
) -> isize {
    let mut iov = libc::iovec {
        iov_base: ptr::null_mut(),
        iov_len: size,
    };
    // SAFETY: an all-zero msghdr is valid.
    let mut msg: libc::msghdr = unsafe { mem::zeroed() };
    msg.msg_iov = &raw mut iov;
    msg.msg_iovlen = 1;
    let from = offset.as_deref().map_or(u64::MAX, |&at| at.cast_unsigned());
    #[expect(clippy::cast_possible_truncation, reason = "upstream's sqe.len = size")]
    let len = size as u32;
    let ret = request(host, timeout, |sqe| {
        // SAFETY: `msg` outlives the request.
        unsafe {
            sys::swr_uring_prep_sendmsg(
                sqe,
                out_fd,
                (&raw const msg).cast(),
                splice_pages.cast_unsigned(),
            );
        }
        sqe.len = len;
        sqe.__bindgen_anon_5.splice_fd_in = in_fd;
        sqe.__bindgen_anon_1.addr2 = from;
    });
    if ret > 0
        && let Some(offset) = offset
    {
        *offset += ret as libc::off_t;
    }
    ret
}

fn send_piped(
    host: &Host,
    out_fd: c_int,
    in_fd: c_int,
    offset: Option<&mut libc::off_t>,
    size: usize,
    timeout: f64,
) -> isize {
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
    let mut event = Event::new(host).with_timeout(timeout);
    while total_sent < size {
        let to_send = (size - total_sent).min(pipe_size);
        let from = start.map_or(-1, |start| {
            start + i64::try_from(total_sent).unwrap_or(i64::MAX)
        });
        #[expect(clippy::cast_possible_truncation, reason = "at most the pipe size")]
        let nbytes = to_send as c_uint;
        // The request completes before the next.
        event.data = zeroed_sqe();
        // SAFETY: the event's own sqe; the splice carries no pointers.
        unsafe { sys::swr_uring_prep_splice(event.sqe(), in_fd, from, write_end, -1, nbytes, 0) };
        let ret1 = event.execute(host);
        if ret1 <= 0 {
            if total_sent > 0 {
                break;
            }
            let saved = errno();
            close_pipe();
            set_errno(saved);
            return -1;
        }
        event.data = zeroed_sqe();
        let spliced = c_uint::try_from(ret1).unwrap_or(c_uint::MAX);
        // SAFETY: as above.
        unsafe { sys::swr_uring_prep_splice(event.sqe(), read_end, -1, out_fd, -1, spliced, 0) };
        let ret2 = event.execute(host);
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
    // SAFETY: an all-zero statx is valid.
    let mut buf: libc::statx = unsafe { mem::zeroed() };
    let mask = libc::STATX_BASIC_STATS | libc::STATX_BTIME;
    // SAFETY: `buf` and `path` outlive the request.
    let ret = request(host, -1.0, |sqe| unsafe {
        sys::swr_uring_prep_statx(sqe, dfd, path.as_ptr(), flags, mask, (&raw mut buf).cast());
    });
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
pub fn waitpid(
    host: &Host,
    pid: libc::pid_t,
    stat_loc: Option<&mut c_int>,
    options: c_int,
    timeout: f64,
) -> libc::pid_t {
    if options & libc::WNOHANG != 0 {
        let stat_loc = stat_loc.map_or(ptr::null_mut(), ptr::from_mut);
        // SAFETY: null or the caller's status word.
        return unsafe { libc::waitpid(pid, stat_loc, options) };
    }
    // SAFETY: an all-zero siginfo_t is valid.
    let mut info: libc::siginfo_t = unsafe { mem::zeroed() };
    let (idtype, id) = if pid > 0 {
        (libc::P_PID, pid.cast_unsigned())
    } else {
        (libc::P_ALL, 0)
    };
    let options = if options == 0 { libc::WEXITED } else { options };
    // SAFETY: `info` outlives the request.
    let rc = int(request(host, timeout, |sqe| unsafe {
        sys::swr_uring_prep_waitid(sqe, idtype, id, (&raw mut info).cast(), options, 0);
    }));
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
    // SAFETY: the request carries no pointers.
    let rc = int(request(host, f64::from(timeout) / 1000.0, |sqe| unsafe {
        sys::swr_uring_prep_poll_add(sqe, descriptor, events.cast_unsigned().into());
    }));
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
        static ENGINE: std::cell::Cell<*mut Uring> = const { std::cell::Cell::new(ptr::null_mut()) };
    }
    extern "C" fn current() -> *mut c_void {
        ptr::dangling_mut()
    }
    fn instance() -> *mut Uring {
        ENGINE.with(std::cell::Cell::get)
    }
    unsafe fn suspend(_coroutine: *mut c_void, event: *mut c_void) {
        let uring = ENGINE.with(std::cell::Cell::get);
        let event = event.cast::<Event>();
        // SAFETY: the test engine is live; the event is in the caller's frame.
        unsafe {
            (*uring).submit_queued();
            while (*uring).task_num > 0 && !(*event).coroutine.is_null() {
                let mut cqe = ptr::null_mut();
                sys::io_uring_wait_cqes(
                    &raw mut (*uring).ring,
                    &raw mut cqe,
                    1,
                    ptr::null_mut(),
                    ptr::null_mut(),
                );
                wakeup(uring);
            }
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
        ENGINE.with(|engine| engine.set(opened.uring));
        test();
        // SAFETY: nothing is in flight.
        unsafe { close(opened.uring) };
        ENGINE.with(|engine| engine.set(ptr::null_mut()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Recorder {
        running: bool,
        used: u32,
        results: Vec<c_int>,
        trace: Vec<String>,
    }

    impl Submitter for Recorder {
        fn event_running(&mut self) -> bool {
            self.trace.push("running".into());
            self.running
        }
        fn sq_used(&mut self) -> u32 {
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

    fn submitted(running: bool, used: u32, results: &[c_int], immediately: bool) -> Vec<String> {
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
