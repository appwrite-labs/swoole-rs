//! Upstream's `core-tests/src/os/signal.cpp`, one function per TEST with the same inputs
//! and expected values: `swoole_signal_set`, `_callback`, `_get_handler`,
//! `_get_listener_num` and `_clear` are crates/reactor's `signal_upstream` over crates/process's
//! signal loop, and the sigaction form of `swoole_signal_set`, `_dispatch`, `_isset`,
//! `_block_all`, `_unblock_all` and `_to_str` are crates/process's `signal`. Each case runs in
//! its own process, so the handlers one case installs never reach another.
//! `os_signal.restore_previous_mask` is crates/corebase's
//! `signal_mask::tests::restore_previous_mask`.

use std::ffi::{CStr, c_int};
use std::sync::atomic::{AtomicI32, Ordering};

use swoole_rs_process::signal::{
    swoole_signal_block_all, swoole_signal_dispatch, swoole_signal_isset, swoole_signal_set_action,
    swoole_signal_to_str, swoole_signal_unblock_all,
};
use swoole_rs_reactor::signal_upstream::{
    swoole_signal_callback, swoole_signal_clear, swoole_signal_get_handler,
    swoole_signal_get_listener_num, swoole_signal_set,
};

use super::{SIGUSR1, SIGUSR2, SW_SIGNO_MAX, getpid, kill, last_error, spawn_exec};

// glibc's values (the core-tests build is Linux).
const SIGTERM: c_int = 15;
const SIGIO: c_int = 29;
const SW_ERROR_UNREGISTERED_SIGNAL: c_int = 721;

/// glibc's `sigset_t`.
#[cfg(target_os = "linux")]
type SigSet = [u64; 16];

type SignalHandler = unsafe extern "C" fn(c_int);

unsafe extern "C" {
    fn sysv_signal(signo: c_int, handler: usize) -> usize;
    fn pthread_self() -> usize;
    fn pthread_kill(thread: usize, signo: c_int) -> c_int;
    fn raise(signo: c_int) -> c_int;
    fn exit(status: c_int) -> !;
    fn waitpid(pid: c_int, status: *mut c_int, options: c_int) -> c_int;
    fn __libc_current_sigrtmin() -> c_int;
    #[cfg(target_os = "linux")]
    fn sigemptyset(set: *mut SigSet) -> c_int;
    #[cfg(target_os = "linux")]
    fn sigprocmask(how: c_int, set: *const SigSet, old: *mut SigSet) -> c_int;
    #[cfg(target_os = "linux")]
    fn sigismember(set: *const SigSet, signo: c_int) -> c_int;
}

/// `swoole_signal_set(signo, func)`, whose `safety` defaults to false.
fn set(signo: c_int, handler: Option<SignalHandler>) -> usize {
    set_safe(signo, handler, false)
}

/// `swoole_signal_set(signo, func, safety)`.
fn set_safe(signo: c_int, handler: Option<SignalHandler>, safety: bool) -> usize {
    let handler = handler.map_or(0, |handler| handler as usize);
    // SAFETY: null or one of this binary's handlers.
    unsafe { swoole_signal_set(signo, handler, safety) }
}

fn clear() {
    // On the main thread, which owns the handlers.
    swoole_signal_clear();
}

fn callback(signo: c_int) {
    // SAFETY: on the main thread, where the handlers may run.
    unsafe { swoole_signal_callback(signo) };
}

/// `swoole::test::spawn_exec_and_wait`: the child's wait status, or -1.
fn spawn_exec_and_wait(body: impl FnOnce()) -> c_int {
    let pid = spawn_exec(body);
    let mut status = 0;
    // SAFETY: our child, a writable status.
    if unsafe { waitpid(pid, &raw mut status, 0) } == pid {
        status
    } else {
        -1
    }
}

/// `ExitStatus::get_code()`: `WEXITSTATUS`.
fn exit_code(status: c_int) -> c_int {
    (status >> 8) & 0xff
}

unsafe extern "C" fn exit_255(_signo: c_int) {
    // SAFETY: ends the process, as upstream's handlers do.
    unsafe { exit(255) };
}

#[cfg(target_os = "linux")]
unsafe extern "C" fn sig_usr1(_signo: c_int) {}

/// Whether `signo` is in this thread's blocked set.
#[cfg(target_os = "linux")]
fn blocked(signo: c_int) -> c_int {
    const SIG_BLOCK: c_int = 0;
    let mut curset: SigSet = [0; 16];
    // SAFETY: sigemptyset(3), sigprocmask(2) and sigismember(3) over a local set.
    unsafe {
        sigemptyset(&raw mut curset);
        sigprocmask(SIG_BLOCK, std::ptr::null(), &raw mut curset);
        sigismember(&raw const curset, signo)
    }
}

#[cfg(target_os = "linux")]
fn signalfd() {
    super::os::event_init();

    assert_eq!(blocked(SIGUSR1), 0);

    swoole_rs_process::signalfd::swoole_signalfd_init();
    set(SIGUSR1, Some(sig_usr1));

    assert_eq!(blocked(SIGUSR1), 1);

    set(SIGUSR1, None);
    clear();

    assert_eq!(blocked(SIGUSR1), 0);

    super::os::event_wait();
}

fn block() {
    assert_eq!(
        spawn_exec_and_wait(|| {
            // SAFETY: sysv_signal(3) with a handler of this binary.
            unsafe { sysv_signal(SIGIO, exit_255 as *const () as usize) };

            std::thread::spawn(|| {
                let _ = swoole_signal_block_all();
                // SAFETY: pthread_kill(3) to this live thread.
                unsafe { pthread_kill(pthread_self(), SIGIO) };
            })
            .join()
            .unwrap();
        }),
        0
    );
}

fn unblock() {
    let status = spawn_exec_and_wait(|| {
        // SAFETY: sysv_signal(3) with a handler of this binary.
        unsafe { sysv_signal(SIGIO, exit_255 as *const () as usize) };

        std::thread::spawn(|| {
            let _ = swoole_signal_block_all();
            // SAFETY: pthread_kill(3) to this live thread.
            unsafe { pthread_kill(pthread_self(), SIGIO) };
            swoole_signal_unblock_all();
        })
        .join()
        .unwrap();
    });

    assert_eq!(exit_code(status), 255);
}

fn signal_to_str() {
    let text = |signo| {
        // SAFETY: the function's static buffer, NUL-terminated.
        unsafe { CStr::from_ptr(swoole_signal_to_str(signo).cast_const()) }
            .to_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(text(SIGTERM), "Terminated: 15");
    assert_eq!(text(SIGIO), "I/O possible: 29");
    // SAFETY: glibc's SIGRTMIN.
    let sigrtmin = unsafe { __libc_current_sigrtmin() };
    assert_eq!(text(sigrtmin), "Real-time signal 0: 34");
    assert_eq!(text(99999), "Unknown signal 99999: 99999");
}

fn set_() {
    set(SIGIO, Some(exit_255));
    assert!(swoole_signal_isset(SIGIO));
    assert!(!swoole_signal_isset(SIGTERM));
    set(SIGIO, None);
    assert!(!swoole_signal_isset(SIGIO));
}

static TRIGGER_SIGNAL: AtomicI32 = AtomicI32::new(0);

unsafe extern "C" fn record(signo: c_int) {
    TRIGGER_SIGNAL.store(signo, Ordering::SeqCst);
}

fn dispatch() {
    TRIGGER_SIGNAL.store(0, Ordering::SeqCst);
    set_safe(SIGIO, Some(record), true);
    // SAFETY: kill(2) to this process, whose SIGIO handler only marks the signal.
    unsafe { kill(getpid(), SIGIO) };
    assert_eq!(TRIGGER_SIGNAL.load(Ordering::SeqCst), 0);

    assert_eq!(swoole_signal_get_handler(SIGTERM), 0);
    assert_ne!(swoole_signal_get_handler(SIGIO), 0);

    swoole_signal_dispatch();
    assert_eq!(TRIGGER_SIGNAL.load(Ordering::SeqCst), SIGIO);

    TRIGGER_SIGNAL.store(0, Ordering::SeqCst);

    swoole_signal_dispatch();
    assert_eq!(TRIGGER_SIGNAL.load(Ordering::SeqCst), 0);

    assert_eq!(swoole_signal_get_listener_num(), 0);

    clear();
}

unsafe extern "C" fn raise_usr2(_signo: c_int) {
    // SAFETY: raise(3); SIGUSR2's handler only records it.
    unsafe { raise(SIGUSR2) };
}

fn nested() {
    TRIGGER_SIGNAL.store(0, Ordering::SeqCst);
    set(SIGUSR1, Some(raise_usr2));
    set(SIGUSR2, Some(record));
    // SAFETY: raise(3); SIGUSR1's handler raises SIGUSR2, whose handler records it.
    unsafe { raise(SIGUSR1) };
    assert_eq!(TRIGGER_SIGNAL.load(Ordering::SeqCst), SIGUSR2);

    clear();
}

fn error() {
    let _ = swoole_signal_set_action(SIGIO, 0, 0, 0);
    let _ = swoole_signal_block_all();
    let _ = swoole_signal_block_all(); // no effect
    swoole_signal_unblock_all();
    swoole_signal_unblock_all(); // no effect

    callback(SW_SIGNO_MAX); // no effect

    swoole_rs_corebase::log::set_last_error(0);
    callback(SIGIO);
    assert_eq!(last_error(), SW_ERROR_UNREGISTERED_SIGNAL);

    assert_eq!(swoole_signal_get_handler(SW_SIGNO_MAX), 0);
}

/// `os_signal.<name>`, in upstream's order; `signalfd` is `#ifdef HAVE_SIGNALFD`.
pub(super) const CASES: super::Cases = &[
    #[cfg(target_os = "linux")]
    ("signalfd", signalfd),
    ("block", block),
    ("unblock", unblock),
    ("signal_to_str", signal_to_str),
    ("set", set_),
    ("dispatch", dispatch),
    ("nested", nested),
    ("error", error),
];
