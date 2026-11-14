//! A coroutine's context: its stack and the switch onto it through boost.context's
//! fcontext (`fcontext.rs`). `AddressSanitizer` follows a stack switch only through its
//! fiber annotations, so they run whenever the process has its runtime, and
//! `ThreadSanitizer` through its own, in tools/tsan.py's build (`cfg(tsan)`). With
//! [`THREAD`] in its flags the context is a thread instead (`thread.rs`).

use crate::Coroutine;
use crate::coroutine::Co;
use crate::fcontext::{Transfer, jump_fcontext, make_fcontext};
use crate::stack::Stack;
use crate::thread::{self, Entry, Started, THREAD};
use core::cell::Cell;
use core::ffi::{c_int, c_void};
use std::ptr;
use std::sync::OnceLock;
use swoole_rs_corebase::CCast;

/// A coroutine's context record, which follows its state in its [`Native`]
/// record and never moves once switched in, so the switch hands its stack the
/// coroutine's `Co`. Both stacks reach it through that handle, and neither holds a
/// mutable borrow across a switch. Dropping it frees its stack, or joins its
/// thread, so it is dropped only once its coroutine ended (or never ran).
#[repr(C)]
pub struct Context {
    /// The suspended context.
    fctx: *mut c_void,
    swap_fctx: *mut c_void,
    /// The stack it runs on and its flags: none under [`THREAD`].
    stack: Stack,
    /// What the first switch in runs: none under [`THREAD`], or once it started.
    entry: Option<Entry>,
    /// The stack that last switched in, which `swap_out` returns to (`AddressSanitizer` only).
    asan_bottom: *const c_void,
    asan_size: usize,
    /// The context's `ThreadSanitizer` fiber, and the one that last switched it in.
    #[cfg(tsan)]
    tsan_fiber: *mut c_void,
    #[cfg(tsan)]
    tsan_caller: Cell<*mut c_void>,
    /// Set once the entry returned.
    end: Cell<bool>,
    /// The [`THREAD`] context's thread.
    thread: Option<Box<Started>>,
}

/// A `swoole::Coroutine`: its scheduler state, then its context. The
/// extension allocates (facade.rs) and frees (scheduler.rs) it;
/// `Coroutine::native` points back at it.
#[repr(C)]
pub struct Native {
    pub co: Coroutine,
    pub ctx: Context,
}

impl Context {
    /// The C stack and its size; null under [`THREAD`].
    #[must_use]
    pub fn stack(&self) -> (*mut u8, usize) {
        (self.stack.base(), self.stack.size())
    }

    /// Allocates the stack and makes the context that runs `entry` on it. A
    /// [`THREAD`] context starts its thread instead.
    ///
    /// # Errors
    /// No stack (out of memory), or the errno of a [`THREAD`] context whose
    /// thread failed to start.
    pub fn new(stack_size: usize, flags: u32, entry: Entry) -> Result<Self, Unmade> {
        let mut context = Self {
            fctx: ptr::null_mut(),
            swap_fctx: ptr::null_mut(),
            stack: Stack::none(flags),
            entry: None,
            asan_bottom: ptr::null(),
            asan_size: 0,
            #[cfg(tsan)]
            tsan_fiber: __tsan_create_fiber(0),
            #[cfg(tsan)]
            tsan_caller: Cell::new(ptr::null_mut()),
            end: Cell::new(false),
            thread: None,
        };
        if flags & THREAD != 0 {
            context.thread = Some(thread::start(entry).map_err(Unmade::Thread)?);
            return Ok(context);
        }
        let stack_size = stack_size.c_cast::<u32>();
        let size = stack_size as usize;
        let stack = Stack::alloc(stack_size, flags).ok_or(Unmade::NoStack)?;
        let top = stack.base().wrapping_add(size);
        // SAFETY: a fresh `size`-byte stack, which boost builds the context down from.
        context.fctx = unsafe { make_fcontext(top.cast(), size, first_frame) };
        context.stack = stack;
        context.entry = Some(entry);
        Ok(context)
    }
}

/// Why [`Context::new`] made no context.
#[derive(Debug)]
pub enum Unmade {
    /// The stack's allocation failed.
    NoStack,
    /// The [`THREAD`] context's thread failed to start, with this errno.
    Thread(c_int),
}

#[cfg(tsan)]
impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: the context's own fiber, which is not running: the context ended or never ran.
        unsafe {
            __tsan_destroy_fiber(self.tsan_fiber);
        }
    }
}

type StartSwitch = unsafe extern "C" fn(*mut *mut c_void, *const c_void, usize);
type FinishSwitch = unsafe extern "C" fn(*mut c_void, *mut *const c_void, *mut usize);

// ThreadSanitizer's fiber calls. Each coroutine runs as its own fiber: TSan keeps a
// thread's setjmp buffers in one list and, on each setjmp, drops those taken below its
// stack pointer, so a setjmp on a coroutine stack mapped above the thread's stack would
// drop the thread's own buffers, and a later bailout to one aborts with "can't find
// longjmp buf" (#3400). A fiber keeps a list of its own.
#[cfg(tsan)]
unsafe extern "C" {
    safe fn __tsan_get_current_fiber() -> *mut c_void;
    safe fn __tsan_create_fiber(flags: core::ffi::c_uint) -> *mut c_void;
    fn __tsan_destroy_fiber(fiber: *mut c_void);
    fn __tsan_switch_to_fiber(fiber: *mut c_void, flags: core::ffi::c_uint);
}

/// `__sanitizer_{start,finish}_switch_fiber`, when `AddressSanitizer` is loaded.
fn asan() -> Option<(StartSwitch, FinishSwitch)> {
    #[repr(C)]
    struct Fibers(StartSwitch, FinishSwitch);
    fn symbol(name: &std::ffi::CStr) -> *mut c_void {
        // SAFETY: dlsym with a NUL-terminated name has no other preconditions.
        unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) }
    }
    static FIBERS: OnceLock<Option<(StartSwitch, FinishSwitch)>> = OnceLock::new();
    *FIBERS.get_or_init(|| {
        let start = symbol(c"__sanitizer_start_switch_fiber");
        let finish = symbol(c"__sanitizer_finish_switch_fiber");
        if start.is_null() || finish.is_null() {
            return None;
        }
        // SAFETY: two non-null addresses of functions the sanitizer interface declares
        // with these signatures, in the order of `Fibers`' fields.
        let Fibers(start, finish) =
            unsafe { std::mem::transmute::<[*mut c_void; 2], Fibers>([start, finish]) };
        Some((start, finish))
    })
}

const _: () = assert!(
    core::mem::offset_of!(Native, ctx) == size_of::<Coroutine>(),
    "a record's context follows its coroutine"
);

impl Co {
    /// Its context, which follows it in the [`Native`] record facade.rs boxed for it.
    fn context(self) -> *mut Context {
        self.ptr().wrapping_add(1).cast()
    }

    /// Its context, shared for `f`. Both stacks (or threads) take only shared borrows
    /// of it while `f` switches away, so `f` may.
    fn read_context<R>(self, f: impl FnOnce(&Context) -> R) -> R {
        // SAFETY: only the scheduler's hooks below and the first frame reach this, with
        // a live coroutine that facade.rs boxed in a `Native`; shared, until `f` returns.
        f(unsafe { &*self.context() })
    }

    /// Its context, borrowed for `f`, which does not switch.
    fn with_context<R>(self, f: impl FnOnce(&mut Context) -> R) -> R {
        // SAFETY: as `read_context`, on the stack that owns the record between switches;
        // the borrow ends with `f`.
        f(unsafe { &mut *self.context() })
    }
}

/// The scheduler's `swap_in` hook: switches from the caller's stack onto `co`'s
/// context, returning when it switches out: whether it ended. The scheduler passes
/// a live coroutine of its own (boxed in a [`Native`]) that has not ended and is not
/// running, whose context stays put from here on.
pub(crate) fn swap_in(co: Co) -> bool {
    let asan = asan();
    let mut fake_stack = ptr::null_mut();
    let fctx = co.read_context(|ctx| {
        if let Some(thread) = &ctx.thread {
            // SAFETY: as the scheduler passes it; this thread holds the baton.
            let ended = unsafe { thread.swap_in() };
            ctx.end.set(ended);
            return Err(ended);
        }
        if let Some((start, _)) = asan {
            // SAFETY: announces the switch onto the context's stack.
            unsafe {
                start(
                    &raw mut fake_stack,
                    ctx.stack.base().cast(),
                    ctx.stack.size(),
                );
            };
        }
        #[cfg(tsan)]
        {
            ctx.tsan_caller.set(__tsan_get_current_fiber());
            // SAFETY: the context's live fiber, which runs from the jump on.
            unsafe { __tsan_switch_to_fiber(ctx.tsan_fiber, 0) };
        }
        Ok(ctx.fctx)
    });
    let fctx = match fctx {
        Ok(fctx) => fctx,
        Err(ended) => return ended,
    };
    // SAFETY: `fctx` is the context's suspended state; it resumes with its coroutine.
    let transfer = unsafe { jump_fcontext(fctx, co) };
    // The context switched back out, so this stack owns the record again.
    co.with_context(|ctx| {
        ctx.fctx = transfer.fctx;
        if let Some((_, finish)) = asan {
            // SAFETY: completes the switch back that `swap_out` started.
            unsafe { finish(fake_stack, ptr::null_mut(), ptr::null_mut()) };
        }
        ctx.end.get()
    })
}

/// The scheduler's `swap_out` hook: switches from `co`'s stack back to the one that
/// last switched it in. The scheduler passes the running coroutine.
pub(crate) fn swap_out(co: Co) {
    let asan = asan();
    let mut fake_stack = ptr::null_mut();
    let swap_fctx = co.read_context(|ctx| {
        if let Some(thread) = &ctx.thread {
            // SAFETY: the running thread, as the scheduler passes it.
            unsafe { thread.swap_out() };
            return None;
        }
        if let Some((start, _)) = asan {
            // A finished context never resumes, so ASan may free its fake stack now.
            let save = if ctx.end.get() {
                ptr::null_mut()
            } else {
                &raw mut fake_stack
            };
            // SAFETY: announces the switch back to the stack that switched in.
            unsafe { start(save, ctx.asan_bottom, ctx.asan_size) };
        }
        #[cfg(tsan)]
        // SAFETY: the fiber that switched this context in, which runs from the jump on.
        unsafe {
            __tsan_switch_to_fiber(ctx.tsan_caller.get(), 0);
        }
        Some(ctx.swap_fctx)
    });
    let Some(swap_fctx) = swap_fctx else {
        return;
    };
    // SAFETY: `swap_fctx` is the suspended caller of `swap_in`.
    let transfer = unsafe { jump_fcontext(swap_fctx, co) };
    // Switched back in, so this stack owns the record again.
    co.with_context(|ctx| switched_in(ctx, transfer.fctx, asan, fake_stack));
}

/// Records the stack that switched in and completes the sanitizer's switch.
fn switched_in(
    ctx: &mut Context,
    swap_fctx: *mut c_void,
    asan: Option<(StartSwitch, FinishSwitch)>,
    fake_stack: *mut c_void,
) {
    ctx.swap_fctx = swap_fctx;
    if let Some((_, finish)) = asan {
        // SAFETY: `fake_stack` is what this stack's last start saved (null on a
        // first switch), and the record's fields are live out-pointers.
        unsafe { finish(fake_stack, &raw mut ctx.asan_bottom, &raw mut ctx.asan_size) };
    }
}

/// The context's first frame: runs the entry, marks the end and switches out for good.
/// `swap_in` jumped here with its coroutine, which now runs.
extern "C" fn first_frame(transfer: Transfer) {
    let co = transfer.data;
    let entry = co.with_context(|record| {
        switched_in(record, transfer.fctx, asan(), ptr::null_mut());
        record.entry.take()
    });
    if let Some(entry) = entry {
        entry.run();
    }
    co.read_context(|ctx| ctx.end.set(true));
    // The running coroutine; an ended one is never switched back in.
    swap_out(co);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn record_layout_keeps_the_offsets_c_reads() {
        assert_eq!(std::mem::offset_of!(Context, stack), 16);
        assert_eq!(
            std::mem::offset_of!(Context, stack) + Stack::SIZE_OFFSET,
            24
        );
        assert_eq!(std::mem::offset_of!(Context, end), 64);
    }

    struct Log {
        co: Option<Co>,
        steps: Vec<&'static str>,
    }

    /// The coroutine of the log's record.
    unsafe fn co(log: *mut Log) -> Co {
        unsafe { (*log).co }.unwrap()
    }

    unsafe extern "C" fn entry(data: *mut c_void) {
        let log = data.cast::<Log>();
        // SAFETY: the test's log, which only the running stack touches.
        unsafe {
            (*log).steps.push("entry");
            swap_out(co(log));
            (*log).steps.push("resumed");
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri runs no assembly")]
    fn a_context_switches_onto_its_stack_and_back() {
        let mut log = Log {
            co: None,
            steps: Vec::new(),
        };
        let log = &raw mut log;
        // SAFETY: the record lives until freed below, and each switch is the stack's turn.
        unsafe {
            let ctx = Context::new(64 * 1024, 0, Entry::new(entry, log.cast())).unwrap();
            (*log).co = Some(Co::leak(Box::new(Native {
                co: Coroutine::IDLE,
                ctx,
            })));
            assert!(!swap_in(co(log)));
            (*log).steps.push("main");
            assert!(swap_in(co(log)));
            drop(Box::from_raw(co(log).ptr().cast::<Native>()));
            assert_eq!((*log).steps, ["entry", "main", "resumed"]);
        }
    }
}
