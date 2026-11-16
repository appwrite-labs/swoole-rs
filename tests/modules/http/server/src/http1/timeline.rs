//! A cross-process event log for the worker-replacement cases (#4046). It records every
//! fork (in the forking process just before `fork`, and in the child as it returns),
//! every process exit (`atexit`) and the case's own marks. Each event has the pid, the
//! time since [`arm`] and the thread's scheduler totals from Linux's
//! /proc/thread-self/schedstat: time on a CPU, and time runnable but waiting for one.
//! The log lives in shared memory the forked processes inherit. Nothing prints until a
//! count assertion fails, so a passing run pays only a clock read, a small procfs read
//! and a few atomic stores per event.

use std::ffi::c_int;
use std::fmt::Write as _;
use std::ptr;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

const CAPACITY: usize = 64;

unsafe extern "C" {
    fn mmap(
        addr: *mut std::ffi::c_void,
        length: usize,
        prot: c_int,
        flags: c_int,
        fd: c_int,
        offset: i64,
    ) -> *mut std::ffi::c_void;
    fn pthread_atfork(
        prepare: Option<extern "C" fn()>,
        parent: Option<extern "C" fn()>,
        child: Option<extern "C" fn()>,
    ) -> c_int;
    fn atexit(callback: extern "C" fn()) -> c_int;
}

#[repr(C)]
struct Event {
    micros: AtomicU64,
    pid: AtomicU32,
    /// schedstat's first two fields, in microseconds.
    on_cpu: AtomicU64,
    waiting: AtomicU64,
    /// A `&'static str`'s address and length: the forks share the binary's image.
    what: AtomicUsize,
    len: AtomicUsize,
}

#[repr(C)]
struct Log {
    next: AtomicUsize,
    events: [Event; CAPACITY],
}

static START: OnceLock<Instant> = OnceLock::new();
static LOG: OnceLock<usize> = OnceLock::new();
/// Set by [`arm`]: the hooks record nothing in a case that never armed the log.
static ARMED: AtomicBool = AtomicBool::new(false);

fn log() -> Option<&'static Log> {
    // SAFETY: the zero-filled shared mapping [`arm`] made, never unmapped.
    LOG.get().map(|&at| unsafe { &*(at as *const Log) })
}

/// Start the clock and the log, and hook fork and exit; call before the server forks.
pub(super) fn arm() {
    LOG.get_or_init(|| {
        // SAFETY: a fresh anonymous shared mapping (PROT_READ | PROT_WRITE,
        // MAP_SHARED | MAP_ANONYMOUS), zero-filled.
        let at = unsafe { mmap(ptr::null_mut(), size_of::<Log>(), 3, 0x21, -1, 0) };
        assert!(!at.is_null() && at as isize != -1, "mmap failed");
        // SAFETY: registers a plain function for the process's life.
        assert_eq!(
            unsafe { pthread_atfork(Some(on_fork), None, Some(on_forked)) },
            0,
            "pthread_atfork"
        );
        // SAFETY: as above.
        assert_eq!(unsafe { atexit(on_exit) }, 0, "atexit");
        at as usize
    });
    START.get_or_init(Instant::now);
    ARMED.store(true, Ordering::SeqCst);
}

extern "C" fn on_fork() {
    mark("fork");
}

extern "C" fn on_forked() {
    mark("forked child");
}

extern "C" fn on_exit() {
    mark("exit");
}

/// Record `what` in this process now.
pub(super) fn mark(what: &'static str) {
    let (Some(log), Some(start)) = (log(), START.get()) else {
        return;
    };
    if !ARMED.load(Ordering::SeqCst) {
        return;
    }
    let micros = u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX);
    let Some(event) = log.events.get(log.next.fetch_add(1, Ordering::SeqCst)) else {
        return;
    };
    let sched = std::fs::read_to_string("/proc/thread-self/schedstat").unwrap_or_default();
    let mut sched = sched
        .split_whitespace()
        .map(|ns| ns.parse::<u64>().unwrap_or(0) / 1000);
    event
        .on_cpu
        .store(sched.next().unwrap_or(0), Ordering::SeqCst);
    event
        .waiting
        .store(sched.next().unwrap_or(0), Ordering::SeqCst);
    event.pid.store(std::process::id(), Ordering::SeqCst);
    event.what.store(what.as_ptr() as usize, Ordering::SeqCst);
    event.len.store(what.len(), Ordering::SeqCst);
    event.micros.store(micros, Ordering::SeqCst);
}

/// The events so far, in time order, one per line.
pub(super) fn render() -> String {
    let Some(log) = log() else {
        return String::from("(no timeline)");
    };
    let count = log.next.load(Ordering::SeqCst).min(CAPACITY);
    let mut events: Vec<_> = log.events[..count]
        .iter()
        .map(|event| {
            let what = event.what.load(Ordering::SeqCst) as *const u8;
            let len = event.len.load(Ordering::SeqCst);
            // SAFETY: a `&'static str` `mark` stored, or empty while its store is pending.
            let what = if what.is_null() {
                "?"
            } else {
                unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(what, len)) }
            };
            let load = |field: &AtomicU64| field.load(Ordering::SeqCst);
            let pid = event.pid.load(Ordering::SeqCst);
            (
                load(&event.micros),
                pid,
                what,
                load(&event.on_cpu),
                load(&event.waiting),
            )
        })
        .collect();
    events.sort_unstable();
    let mut out = String::from(
        "timeline (us since start, pid, event, the thread's us so far on a CPU / runnable waiting for one):",
    );
    for (micros, pid, what, on_cpu, waiting) in events {
        _ = write!(
            out,
            "\n  +{micros:>8} {pid:>7} {what:<22} {on_cpu:>7} / {waiting}"
        );
    }
    out
}
