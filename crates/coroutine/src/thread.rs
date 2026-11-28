// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/coroutine/thread_context.cc (Swoole contributors).
//! `--enable-thread-context`: each coroutine runs on its own thread, and a switch
//! hands a baton from one thread to the next, so exactly one runs at a time.
//! Upstream hands off with `std::mutex`es unlocked by threads that didn't lock
//! them; a baton is a binary semaphore, which any thread may post.
use crate::Routine;
use core::cell::RefCell;
use core::ffi::{c_int, c_void};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
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

/// Whether coroutines are active on threads of their own.
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// The activating thread's `SwooleTG` members each coroutine thread adopts.
static SHARED: Mutex<Option<SharedThread>> = Mutex::new(None);

thread_local! {
    /// A coroutine thread's own record; none on the activating thread.
    static ME: RefCell<Option<Arc<Thread>>> = const { RefCell::new(None) };
}

/// One coroutine's thread, shared by the thread and its context.
#[derive(Default)]
struct Thread {
    baton: Baton,
    /// The thread that last switched this one in, none for the activating
    /// thread; taken when this one switches back.
    swap: Mutex<Option<Arc<Thread>>>,
    end: AtomicBool,
}

impl Thread {
    /// Posts the baton of the thread that switched this one in.
    fn hand_back(&self) {
        let swap = self
            .swap
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        swap.as_deref().map_or(&MAIN, |swap| &swap.baton).post();
    }
}

/// Waits for the calling thread's baton.
fn wait_mine() {
    ME.with_borrow(|me| me.as_deref().map_or(&MAIN, |me| &me.baton).wait());
}

/// A [`THREAD`] context's thread, which `Context` owns; dropping it joins the
/// thread, which must have ended or be about to.
pub(crate) struct Started {
    thread: Arc<Thread>,
    handle: Option<JoinHandle<()>>,
}

/// `SWR_CTX_THREAD` (`SW_USE_THREAD_CONTEXT`): the context is a thread, not a stack.
pub const THREAD: u32 = 4;

/// The entry a coroutine thread runs.
struct Entry(Routine, *mut c_void);

// SAFETY: only the coroutine's thread uses the creator's entry and data, while
// it holds the baton, so no two threads touch the data at once.
unsafe impl Send for Entry {}

/// `thread_context_init`: the calling thread holds the baton, and coroutine
/// threads share its reactor, timer, buffer stack and async threads.
pub fn swr_v1_thread_context_init() {
    *SHARED.lock().unwrap_or_else(PoisonError::into_inner) = Some(shared_thread());
    ACTIVE.store(true, Ordering::Release);
}

/// Whether coroutines run on threads of their own (`SW_USE_THREAD_CONTEXT`, active).
#[must_use]
pub fn thread_context_active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

/// `thread_context_clean`.
pub fn swr_v1_thread_context_clean() {
    *SHARED.lock().unwrap_or_else(PoisonError::into_inner) = None;
    ACTIVE.store(false, Ordering::Release);
}

/// Starts a thread that waits for the baton, then runs `entry(data)`; the
/// spawn's errno when it fails.
///
/// # Safety
/// `entry` takes `data`.
pub(crate) unsafe fn start(entry: Routine, data: *mut c_void) -> Result<Box<Started>, c_int> {
    let thread = Arc::new(Thread::default());
    let own = Arc::clone(&thread);
    let entry = Entry(entry, data);
    let handle = std::thread::Builder::new()
        .stack_size(THREAD_STACK)
        .spawn(move || {
            let entry = entry;
            swoole_rs_corebase::signal_block_all();
            run(&own, entry);
        })
        .map_err(|error| error.raw_os_error().unwrap_or(libc::EAGAIN))?;
    Ok(Box::new(Started {
        thread,
        handle: Some(handle),
    }))
}

impl Started {
    /// Switches onto the thread, returning when it switches back: whether it ended.
    ///
    /// # Safety
    /// The thread has not ended and is not running; the caller holds the baton.
    pub(crate) unsafe fn swap_in(&self) -> bool {
        *self
            .thread
            .swap
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = ME.with_borrow(Clone::clone);
        self.thread.baton.post();
        wait_mine();
        self.thread.end.load(Ordering::Acquire)
    }

    /// Switches back to the thread that last switched this one in.
    ///
    /// # Safety
    /// This is the running thread, called on it.
    pub(crate) unsafe fn swap_out(&self) {
        self.thread.hand_back();
        self.thread.baton.wait();
    }
}

impl Drop for Started {
    fn drop(&mut self) {
        // A panic can't cross the entry's extern "C" frame, so join never errs.
        if let Some(Err(_)) = self.handle.take().map(JoinHandle::join) {
            std::process::abort();
        }
    }
}

/// The thread's body: wait for the baton, run the entry, mark the end and hand
/// the baton back for good.
fn run(thread: &Arc<Thread>, Entry(entry, data): Entry) {
    thread.baton.wait();
    // The first swap_in follows activation, which records SHARED.
    if let Some(shared) = *SHARED.lock().unwrap_or_else(PoisonError::into_inner) {
        adopt_shared_thread(shared);
    }
    ME.set(Some(Arc::clone(thread)));
    // SAFETY: the creator's entry over its own data, as `start`'s caller promised.
    unsafe { entry(data) };
    ME.take();
    thread.end.store(true, Ordering::Release);
    thread.hand_back();
}

#[cfg(test)]
mod tests {
    use super::*;

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
        thread: Option<Box<Started>>,
        steps: Vec<&'static str>,
    }

    unsafe extern "C" fn entry(data: *mut c_void) {
        let log = data.cast::<Log>();
        // SAFETY: the test's log; this thread holds the baton.
        unsafe {
            (*log).steps.push("entry");
            (*log).thread.as_deref().unwrap().swap_out();
            (*log).steps.push("resumed");
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no sigfillset")]
    fn a_thread_context_runs_one_side_at_a_time() {
        let mut log = Log {
            thread: None,
            steps: Vec::new(),
        };
        let log = &raw mut log;
        swr_v1_thread_context_init();
        assert!(thread_context_active());
        // The thread backend's own entry points, which context.rs's dispatch on THREAD reaches.
        unsafe {
            (*log).thread = Some(start(entry, log.cast()).unwrap());
            let thread: *const Started = (*log).thread.as_deref().unwrap();
            assert!(!(*thread).swap_in());
            (*log).steps.push("main");
            assert!((*thread).swap_in());
            (*log).thread = None;
            assert_eq!((*log).steps, ["entry", "main", "resumed"]);
        }
        swr_v1_thread_context_clean();
        assert!(!thread_context_active());
    }
}
