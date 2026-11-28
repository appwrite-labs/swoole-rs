//! Upstream's `swoole::Thread`: a pthread started with every signal blocked, as
//! upstream's `Thread::start` does, and the exit status its body reports. Thread
//! mode keeps one per worker slot, shared with the PHP thread that runs on it.
use std::ffi::{c_int, c_void};
use std::ptr;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Mutex, PoisonError};

/// A thread's body, run once on it.
type Body = Box<dyn FnOnce() + Send>;

/// A started thread's handle, until it is joined.
struct Handle(libc::pthread_t);

// SAFETY: a pthread handle names a thread, not memory; any thread may join it,
// and the record's mutex lets only one do so.
unsafe impl Send for Handle {}

#[derive(Default)]
pub struct Thread {
    handle: Mutex<Option<Handle>>,
    exit_status: AtomicI32,
}

/// The thread's start routine: `start` hands it the boxed body, which it owns.
extern "C" fn entry(body: Box<Body>) -> *mut c_void {
    body();
    ptr::null_mut()
}

/// `pthread_create`'s start routine type.
type Routine = extern "C" fn(*mut c_void) -> *mut c_void;

impl Thread {
    fn handle(&self) -> std::sync::MutexGuard<'_, Option<Handle>> {
        self.handle.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `start(fn)`: runs `body` on a new pthread with default attributes, which
    /// inherits a fully blocked signal mask. A thread already started and not yet
    /// joined was `std::terminate()`; so it is here.
    ///
    /// # Errors
    /// `pthread_create`'s error; nothing started.
    pub fn start(&self, body: impl FnOnce() + Send + 'static) -> Result<(), c_int> {
        let mut handle = self.handle();
        if handle.is_some() {
            std::process::abort();
        }
        let body: *mut Body = Box::into_raw(Box::new(Box::new(body)));
        // SAFETY: a `Box<Body>` parameter is ABI-compatible with the `*mut c_void`
        // `pthread_create` passes, which is always `body`.
        let entry = unsafe {
            std::mem::transmute::<extern "C" fn(Box<Body>) -> *mut c_void, Routine>(entry)
        };
        let mut thread: libc::pthread_t = crate::sys::zeroed();
        let code = crate::with_signals_blocked(|| {
            // SAFETY: default attributes; `entry` owns `body` once the thread starts.
            unsafe { libc::pthread_create(&raw mut thread, ptr::null(), entry, body.cast()) }
        });
        if code != 0 {
            // SAFETY: no thread started, so the box is still ours.
            drop(unsafe { Box::from_raw(body) });
            return Err(code);
        }
        *handle = Some(Handle(thread));
        Ok(())
    }

    /// The started thread's `pthread_t`, until it is joined or detached.
    pub fn id(&self) -> Option<libc::pthread_t> {
        self.handle().as_ref().map(|handle| handle.0)
    }

    /// `detach()`: the thread runs on unjoined. False when there is none.
    pub fn detach(&self) -> bool {
        let Some(Handle(thread)) = self.handle().take() else {
            return false;
        };
        // SAFETY: the handle `start` stored, taken so no one else joins or detaches it.
        unsafe { libc::pthread_detach(thread) };
        true
    }

    /// `joinable()`: started and not yet joined.
    pub fn joinable(&self) -> bool {
        self.handle().is_some()
    }

    /// `join()`: waits for the thread to end.
    ///
    /// # Errors
    /// `EINVAL` when there is no thread to join, or `pthread_join`'s error.
    pub fn join(&self) -> Result<(), c_int> {
        let Some(Handle(thread)) = self.handle().take() else {
            return Err(libc::EINVAL);
        };
        // SAFETY: the handle `start` stored, taken so no one else joins it.
        match unsafe { libc::pthread_join(thread, ptr::null_mut()) } {
            0 => Ok(()),
            code => Err(code),
        }
    }

    /// `enter()`: the body began; its exit status is 0 until it reports one.
    pub fn enter(&self) {
        self.exit_status.store(0, Ordering::Release);
    }

    /// `exit(status)`: the body's exit status.
    pub fn exit(&self, status: c_int) {
        self.exit_status.store(status, Ordering::Release);
    }

    /// `get_exit_status()`.
    pub fn exit_status(&self) -> c_int {
        self.exit_status.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no sigfillset")]
    fn a_thread_is_joinable_until_joined_and_keeps_its_exit_status() {
        let thread = Arc::new(Thread::default());
        assert!(!thread.joinable());
        assert_eq!(thread.join(), Err(libc::EINVAL));
        let release = Arc::new(AtomicBool::new(false));
        let (body_thread, body_release) = (Arc::clone(&thread), Arc::clone(&release));
        thread
            .start(move || {
                body_thread.enter();
                while !body_release.load(Ordering::Acquire) {
                    std::thread::yield_now();
                }
                body_thread.exit(37);
            })
            .unwrap();
        assert!(thread.joinable());
        release.store(true, Ordering::Release);
        assert_eq!(thread.join(), Ok(()));
        assert!(!thread.joinable());
        assert_eq!(thread.exit_status(), 37);
        assert_eq!(thread.join(), Err(libc::EINVAL));
    }

    /// core-tests/src/os/thread.cpp `TEST(thread, start_with_signals_blocked)`, each
    /// case in a new thread rather than a forked child: the mask and the flag are per
    /// thread.
    #[test]
    #[cfg_attr(miri, ignore = "Miri has no sigaddset")]
    fn start_with_signals_blocked() {
        use crate::signal_mask::tests::{current, only, same};
        for already_blocked in [false, true] {
            std::thread::spawn(move || unsafe {
                only(libc::SIGUSR1);
                if already_blocked {
                    crate::signal_block_all();
                }
                let expected = current();
                let inherited = Arc::new(Mutex::new(None));
                let slot = Arc::clone(&inherited);
                let thread = Thread::default();
                thread
                    .start(move || *slot.lock().unwrap() = Some(current()))
                    .unwrap();
                assert_eq!(thread.join(), Ok(()));
                let inherited = inherited.lock().unwrap().unwrap();
                for signo in [libc::SIGCHLD, libc::SIGUSR1, libc::SIGTERM] {
                    assert_eq!(
                        libc::sigismember(&raw const inherited, signo),
                        1,
                        "already_blocked={already_blocked}"
                    );
                }
                assert_eq!(crate::signal_blocking_all(), already_blocked);
                assert!(
                    same(expected, current()),
                    "already_blocked={already_blocked}"
                );
            })
            .join()
            .unwrap();
        }
    }
}
