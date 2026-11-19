// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/coroutine/thread_context.cc (Swoole contributors).
//! `--enable-thread-context`: each coroutine runs on its own thread, and a switch
//! hands a baton from one thread to the next, so exactly one runs at a time.
//! Upstream hands off with `std::mutex`es unlocked by threads that didn't lock
//! them; a baton is a binary semaphore, which any thread may post.
use crate::small_fn::CopyFn;
use crate::{CellExt, SharedThread, adopt_shared_thread, shared_thread};
use core::cell::Cell;
use core::ffi::c_int;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;

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
    /// A coroutine thread's own record; none on the activating thread. Read
    /// by cloning it out ([`me`]), never borrowed.
    static ME: Cell<Option<Arc<Thread>>> = const { Cell::new(None) };
}

/// An owned reference to the calling coroutine thread, if any.
fn me() -> Option<Arc<Thread>> {
    ME.with(|me| me.with_taken(|record| record.clone()))
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
    me().as_deref().map_or(&MAIN, |me| &me.baton).wait();
}

/// A [`THREAD`] context's thread, which `Context` owns; dropping it joins the
/// thread, which must have ended or be about to.
pub(crate) struct Started {
    thread: Arc<Thread>,
    handle: Option<JoinHandle<()>>,
}

/// `SWR_CTX_THREAD` (`SW_USE_THREAD_CONTEXT`): the context is a thread, not a stack.
pub const THREAD: u32 = 4;

/// A coroutine's entry, which only [`Entry::of`] makes (in this crate): a `Copy`
/// closure the context it is handed to runs once, on its own stack or thread. It is
/// held inline: the largest closure a coroutine starts with, `phpcoroutines`'
/// (a call's callable, arguments and their count), fits.
pub struct Entry(CopyFn<(), ENTRY_WORDS>);

/// The words an [`Entry`] holds.
const ENTRY_WORDS: usize = 10;

#[expect(
    clippy::non_send_fields_in_send_ty,
    reason = "the slot holds a closure that may not be `Send`; the baton, not the type, \
              keeps its two threads apart, as the SAFETY comment says"
)]
// SAFETY: only this crate makes an entry, and only to hand it to the context of a new
// coroutine, which runs it once, on the thread that runs the coroutine, while that
// thread holds the baton and every other one waits for its own: its closure's
// captures are never reached from two threads at once, and each handoff orders the
// accesses on either side of it. They are `Copy`, so none has a destructor to run on
// the other thread.
unsafe impl Send for Entry {}

impl Entry {
    /// No entry: a [`THREAD`] context's, whose thread holds it.
    pub(crate) const fn empty() -> Self {
        Self(CopyFn::empty())
    }

    /// The entry that runs `entry`.
    pub(crate) fn of<F: Fn() + Copy + 'static>(entry: F) -> Self {
        let entry_slot = Self::empty();
        entry_slot.0.put(entry);
        entry_slot
    }

    /// Runs the held entry in place, where it stays: a stack context's, whose
    /// record never moves and whose first frame alone runs it.
    pub(crate) fn run_held(&self) {
        _ = self.0.run();
    }

    /// Runs the entry; this takes it, so it runs once.
    pub(crate) fn run(self) {
        self.run_held();
    }
}

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

/// Starts a thread that waits for the baton, then runs `entry`; the spawn's
/// errno when it fails.
pub(crate) fn start(entry: Entry) -> Result<Box<Started>, c_int> {
    let thread = Arc::new(Thread::default());
    let own = Arc::clone(&thread);
    let handle = std::thread::Builder::new()
        .stack_size(THREAD_STACK)
        .spawn(move || {
            let entry = entry;
            crate::signal_block_all();
            run(&own, entry);
        })
        .map_err(|error| error.raw_os_error().unwrap_or(libc::EAGAIN))?;
    Ok(Box::new(Started {
        thread,
        handle: Some(handle),
    }))
}

// Only the running thread runs a switch, as only it runs any coroutine code (the
// invariant every scheduler access rests on). A switch posts one baton, then waits
// on one, so whatever thread it is called on, at most one thread runs after it: a
// wrong one can leave threads waiting for good, never run two at once. Neither
// asks anything more of its caller.
impl Started {
    /// Switches onto the thread, which has not ended and is not running, returning
    /// when it switches back: whether it ended.
    pub(crate) fn swap_in(&self) -> bool {
        *self
            .thread
            .swap
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = me();
        self.thread.baton.post();
        wait_mine();
        self.thread.end.load(Ordering::Acquire)
    }

    /// Switches back to the thread that last switched this one in, called on the
    /// running thread.
    pub(crate) fn swap_out(&self) {
        self.thread.hand_back();
        self.thread.baton.wait();
    }
}

impl Drop for Started {
    fn drop(&mut self) {
        // A panic can't cross the entry's extern "C" frame, so join never errs.
        if let Some(Err(_)) = self.handle.take().map(JoinHandle::join) {
            crate::fatal("a coroutine thread's join failed");
        }
    }
}

/// The thread's body: wait for the baton, run the entry, mark the end and hand
/// the baton back for good.
fn run(thread: &Arc<Thread>, entry: Entry) {
    thread.baton.wait();
    // The first swap_in follows activation, which records SHARED.
    if let Some(shared) = SHARED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
    {
        adopt_shared_thread(shared);
    }
    ME.set(Some(Arc::clone(thread)));
    entry.run();
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

    fn entry(log: *mut Log) {
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
            (*log).thread = Some(start(Entry::of(move || entry(log))).unwrap());
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
