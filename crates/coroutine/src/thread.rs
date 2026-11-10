// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/coroutine/thread_context.cc (Swoole contributors).
//! `--enable-thread-context`: each coroutine runs on its own thread, and a switch
//! hands a baton from one thread to the next, so exactly one runs at a time.
//! Upstream hands off with `std::mutex`es unlocked by threads that didn't lock
//! them; a baton is a binary semaphore, which any thread may post.

use crate::context::Context;
use core::ffi::c_int;
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use swoole_rs_corebase::{SharedThread, adopt_shared_thread, shared_thread};

/// glibc's default thread stack under the usual 8 MiB `RLIMIT_STACK`, which
/// upstream's `std::thread` gets; Rust's default is 2 MiB.
const THREAD_STACK: usize = 8 * 1024 * 1024;

#[derive(Default)]
struct Baton {
    ready: Mutex<bool>,
    wake: Condvar,
}

impl Baton {
    fn post(&self) {
        *self.ready.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.wake.notify_one();
    }

    fn wait(&self) {
        let ready = self.ready.lock().unwrap_or_else(PoisonError::into_inner);
        let mut ready = self
            .wake
            .wait_while(ready, |ready| !*ready)
            .unwrap_or_else(PoisonError::into_inner);
        *ready = false;
    }
}

/// The activating thread's baton.
static MAIN: Baton = Baton {
    ready: Mutex::new(false),
    wake: Condvar::new(),
};
/// The running thread's baton, null while coroutines are inactive.
static CURRENT: AtomicPtr<Baton> = AtomicPtr::new(ptr::null_mut());
/// The activating thread's `SwooleTG` members each coroutine thread adopts.
static SHARED: Mutex<Option<SharedThread>> = Mutex::new(None);

/// One coroutine's thread; `Context::fctx` owns it while `Context::flags` has
/// [`THREAD`].
struct Thread {
    baton: Baton,
    /// The baton of the thread that last switched this one in.
    swap: *const Baton,
    handle: Option<JoinHandle<()>>,
}

/// `SWR_CTX_THREAD` (`SW_USE_THREAD_CONTEXT`): the context is a thread, not a stack.
pub const THREAD: u32 = 4;

struct Start(*mut Context, *mut Thread);
// SAFETY: the record outlives its thread (free joins it), and only the thread
// holding the baton touches it.
unsafe impl Send for Start {}

/// `thread_context_init`: the calling thread holds the baton, and coroutine
/// threads share its reactor, timer, buffer stack and async threads.
pub fn swr_v1_thread_context_init() {
    *SHARED.lock().unwrap_or_else(PoisonError::into_inner) = Some(shared_thread());
    CURRENT.store(ptr::from_ref(&MAIN).cast_mut(), Ordering::Release);
}

/// Whether coroutines run on threads of their own (`SW_USE_THREAD_CONTEXT`, active).
#[must_use]
pub fn thread_context_active() -> bool {
    !CURRENT.load(Ordering::Acquire).is_null()
}

/// `thread_context_clean`.
pub fn swr_v1_thread_context_clean() {
    *SHARED.lock().unwrap_or_else(PoisonError::into_inner) = None;
    CURRENT.store(ptr::null_mut(), Ordering::Release);
}

/// Starts `ctx`'s thread, which waits for the baton. Returns the spawn's errno.
///
/// # Safety
/// `ctx` is initialized apart from `fctx` and stays at this address until free.
pub(crate) unsafe fn start(ctx: *mut Context) -> c_int {
    let thread = Box::into_raw(Box::new(Thread {
        baton: Baton::default(),
        swap: ptr::null(),
        handle: None,
    }));
    let start = Start(ctx, thread);
    let spawned = std::thread::Builder::new()
        .stack_size(THREAD_STACK)
        .spawn(move || {
            let start = start;
            swoole_rs_corebase::signal_block_all();
            // SAFETY: the record and its thread live until free joins this one.
            unsafe { run(start.0, start.1) };
        });
    match spawned {
        Ok(handle) => {
            // SAFETY: fresh from Box::into_raw; the new thread waits for the baton.
            unsafe { (*thread).handle = Some(handle) };
            // SAFETY: `ctx` is writable, as the caller promises.
            unsafe { (*ctx).fctx = thread.cast() };
            0
        }
        Err(error) => {
            // SAFETY: fresh from Box::into_raw and never shared.
            drop(unsafe { Box::from_raw(thread) });
            error.raw_os_error().unwrap_or(libc::EAGAIN)
        }
    }
}

/// # Safety
/// `ctx` came from [`start`]; its thread has ended or will.
pub(crate) unsafe fn free(ctx: *mut Context) {
    // SAFETY: as the caller promises.
    let thread = unsafe { (*ctx).fctx.cast::<Thread>() };
    // SAFETY: from Box::into_raw in `start`, freed once.
    let thread = unsafe { Box::from_raw(thread) };
    // A panic can't cross the entry's extern "C" frame, so join never errs.
    if let Some(Err(_)) = thread.handle.map(JoinHandle::join) {
        std::process::abort();
    }
}

/// # Safety
/// `ctx` came from [`start`], has not ended, and is not running.
pub(crate) unsafe fn swap_in(ctx: *mut Context) {
    // SAFETY: as the caller promises.
    let thread = unsafe { (*ctx).fctx.cast::<Thread>() };
    let swap = CURRENT.load(Ordering::Acquire).cast_const();
    // SAFETY: this thread holds the baton, so the record is its alone.
    unsafe { (*thread).swap = swap };
    // SAFETY: a live thread record.
    let baton = unsafe { &raw const (*thread).baton };
    CURRENT.store(baton.cast_mut(), Ordering::Release);
    // SAFETY: the baton lives as long as its record; the activating thread's is static.
    unsafe { (*baton).post() };
    // SAFETY: as above.
    unsafe { (*swap).wait() };
}

/// # Safety
/// `ctx` is the running context.
pub(crate) unsafe fn swap_out(ctx: *mut Context) {
    // SAFETY: as the caller promises.
    let thread = unsafe { (*ctx).fctx.cast::<Thread>() };
    // SAFETY: this thread holds the baton.
    let swap = unsafe { (*thread).swap };
    CURRENT.store(swap.cast_mut(), Ordering::Release);
    // SAFETY: the switching-in thread's baton outlives this switch.
    unsafe { (*swap).post() };
    // SAFETY: a live thread record.
    unsafe { (*thread).baton.wait() };
}

/// The thread's body: wait for the baton, run the entry, mark the end and hand
/// the baton back for good.
///
/// # Safety
/// `ctx` and `thread` came from [`start`].
unsafe fn run(ctx: *mut Context, thread: *mut Thread) {
    // SAFETY: a live thread record.
    unsafe { (*thread).baton.wait() };
    // The first swap_in follows activation, which records SHARED.
    if let Some(shared) = *SHARED.lock().unwrap_or_else(PoisonError::into_inner) {
        adopt_shared_thread(shared);
    }
    let (entry, data) = {
        // SAFETY: this thread holds the baton; the borrow ends here.
        let ctx = unsafe { &*ctx };
        (ctx.entry, ctx.data)
    };
    // SAFETY: C++'s entry over its own data.
    unsafe { entry(data) };
    // SAFETY: this thread still holds the baton.
    unsafe { (*ctx).end = true };
    // SAFETY: as above.
    let swap = unsafe { (*thread).swap };
    CURRENT.store(swap.cast_mut(), Ordering::Release);
    // SAFETY: the switching-in thread's baton outlives this switch.
    unsafe { (*swap).post() };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn a_baton_hands_off_between_threads() {
        let baton = Arc::new(Baton::default());
        let back = Arc::new(Baton::default());
        let (b, k) = (Arc::clone(&baton), Arc::clone(&back));
        let child = std::thread::spawn(move || {
            b.wait();
            k.post();
        });
        baton.post();
        back.wait();
        child.join().unwrap();
    }

    struct Log {
        ctx: core::mem::MaybeUninit<Context>,
        steps: Vec<&'static str>,
    }

    unsafe extern "C" fn entry(data: *mut core::ffi::c_void) {
        let log = data.cast::<Log>();
        // SAFETY: the test's log; this thread holds the baton.
        unsafe { (*log).steps.push("entry") };
        // SAFETY: the running context.
        unsafe { swap_out((*log).ctx.as_mut_ptr()) };
        // SAFETY: as above.
        unsafe { (*log).steps.push("resumed") };
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no sigfillset")]
    fn a_thread_context_runs_one_side_at_a_time() {
        let mut log = Log {
            ctx: core::mem::MaybeUninit::uninit(),
            steps: Vec::new(),
        };
        let data = (&raw mut log).cast();
        let ctx = log.ctx.as_mut_ptr();
        swr_v1_thread_context_init();
        // The context's own entry points, without the fcontext ones this test can't link.
        unsafe {
            ctx.write(Context::threaded(THREAD, entry, data));
            assert_eq!(start(ctx), 0);
            swap_in(ctx);
            log.steps.push("main");
            assert!(!(*ctx).end);
            swap_in(ctx);
            assert!((*ctx).end);
            free(ctx);
        }
        swr_v1_thread_context_clean();
        assert_eq!(log.steps, ["entry", "main", "resumed"]);
    }
}
