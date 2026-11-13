//! A coroutine's context: its stack and the switch onto it through boost.context's
//! fcontext (`fcontext.rs`). `AddressSanitizer` follows a stack switch only through its
//! fiber annotations, so they run whenever the process has its runtime, and
//! `ThreadSanitizer` through its own, in tools/tsan.py's build (`cfg(tsan)`). With
//! [`THREAD`] in its flags the context is a thread instead (`thread.rs`).

use crate::fcontext::{Transfer, jump_fcontext, make_fcontext};
use crate::stack::{swr_v1_context_stack_alloc, swr_v1_context_stack_free};
use crate::thread::{self, Started, THREAD};
use crate::{Coroutine, Routine};
use core::cell::Cell;
use core::ffi::{c_int, c_void};
use std::ptr;
use std::sync::OnceLock;
use swoole_rs_corebase::CCast;

/// A coroutine's context record, which follows its state in its [`Native`]
/// record and never moves once switched in, so the switch may hand out its
/// address. Both stacks reach it through raw pointers, and neither holds a
/// mutable borrow across a switch. Dropping it frees its stack, or joins its
/// thread, so it is dropped only once its coroutine ended (or never ran).
#[repr(C)]
pub struct Context {
    /// The suspended context.
    fctx: *mut c_void,
    swap_fctx: *mut c_void,
    stack: *mut u8,
    stack_size: u32,
    flags: u32,
    entry: Routine,
    data: *mut c_void,
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
        (self.stack, self.stack_size as usize)
    }

    /// Allocates the stack and makes the context that runs `entry(data)` on it.
    /// Out of memory is upstream's fatal error, then exit 254. A [`THREAD`]
    /// context starts its thread instead.
    ///
    /// # Errors
    /// The errno of a [`THREAD`] context whose thread failed to start.
    ///
    /// # Safety
    /// `entry` takes `data`.
    pub unsafe fn new(
        stack_size: usize,
        flags: u32,
        entry: Routine,
        data: *mut c_void,
    ) -> Result<Self, c_int> {
        let mut context = Self {
            fctx: ptr::null_mut(),
            swap_fctx: ptr::null_mut(),
            stack: ptr::null_mut(),
            stack_size: 0,
            flags,
            entry,
            data,
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
            // SAFETY: as the caller promises.
            context.thread = Some(unsafe { thread::start(entry, data) }?);
            return Ok(context);
        }
        let stack_size = stack_size.c_cast::<u32>();
        let size = stack_size as usize;
        let stack = swr_v1_context_stack_alloc(size, flags).cast::<u8>();
        if stack.is_null() {
            // SAFETY: nothing on this frame owns a destructor.
            unsafe {
                swoole_rs_corebase::fatal_error(
                    SW_ERROR_MALLOC_FAIL,
                    c"failed to malloc stack memory.",
                );
            }
            std::process::exit(254);
        }
        // SAFETY: a fresh `size`-byte stack, which boost builds the context down from.
        context.fctx = unsafe { make_fcontext(stack.wrapping_add(size).cast(), size, first_frame) };
        context.stack = stack;
        context.stack_size = stack_size;
        Ok(context)
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        if !self.stack.is_null() {
            // SAFETY: allocated by `new` with this size and these flags, freed once;
            // the context is not running.
            unsafe {
                swr_v1_context_stack_free(self.stack.cast(), self.stack_size as usize, self.flags);
            }
        }
        #[cfg(tsan)]
        // SAFETY: the context's own fiber, which is not running: the context ended or never ran.
        unsafe {
            __tsan_destroy_fiber(self.tsan_fiber);
        }
    }
}

/// `SW_ERROR_MALLOC_FAIL`.
const SW_ERROR_MALLOC_FAIL: c_int = 501;

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

/// Switches from the caller's stack onto `ctx`'s, returning when it switches
/// out: whether it ended.
///
/// # Safety
/// `ctx` is a [`Context::new`] that stays put from here on, has not ended, and
/// is not running.
pub unsafe fn swap_in(ctx: *mut Context) -> bool {
    let asan = asan();
    let mut fake_stack = ptr::null_mut();
    let fctx = {
        // SAFETY: a live context, as the caller promises. Only shared borrows of
        // it are taken until the switch returns.
        let ctx = unsafe { &*ctx };
        if let Some(thread) = &ctx.thread {
            // SAFETY: as the caller promises; this thread holds the baton.
            let ended = unsafe { thread.swap_in() };
            ctx.end.set(ended);
            return ended;
        }
        if let Some((start, _)) = asan {
            // SAFETY: announces the switch onto the context's stack.
            unsafe {
                start(
                    &raw mut fake_stack,
                    ctx.stack.cast(),
                    ctx.stack_size as usize,
                );
            };
        }
        #[cfg(tsan)]
        {
            ctx.tsan_caller.set(__tsan_get_current_fiber());
            // SAFETY: the context's live fiber, which runs from the jump on.
            unsafe { __tsan_switch_to_fiber(ctx.tsan_fiber, 0) };
        }
        ctx.fctx
    };
    // SAFETY: `fctx` is the context's suspended state; it resumes with its record.
    let transfer = unsafe { jump_fcontext(fctx, ctx.cast()) };
    // SAFETY: the context switched back out, so this stack owns the record again.
    let ctx = unsafe { &mut *ctx };
    ctx.fctx = transfer.fctx;
    if let Some((_, finish)) = asan {
        // SAFETY: completes the switch back that `swap_out` started.
        unsafe { finish(fake_stack, ptr::null_mut(), ptr::null_mut()) };
    }
    ctx.end.get()
}

/// Switches from `ctx`'s stack back to the one that last switched it in.
///
/// # Safety
/// `ctx` is the running context.
pub unsafe fn swap_out(ctx: *mut Context) {
    let asan = asan();
    let mut fake_stack = ptr::null_mut();
    let swap_fctx = {
        // SAFETY: the running context's record; only shared borrows of it are
        // taken until the switch returns.
        let ctx = unsafe { &*ctx };
        if let Some(thread) = &ctx.thread {
            // SAFETY: the running thread, as the caller promises.
            return unsafe { thread.swap_out() };
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
        ctx.swap_fctx
    };
    // SAFETY: `swap_fctx` is the suspended caller of `swap_in`.
    let transfer = unsafe { jump_fcontext(swap_fctx, ctx.cast()) };
    // SAFETY: switched back in, so this stack owns the record again.
    switched_in(unsafe { &mut *ctx }, transfer.fctx, asan, fake_stack);
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
extern "C" fn first_frame(transfer: Transfer) {
    let ctx = transfer.data.cast::<Context>();
    let (entry, data) = {
        // SAFETY: `swap_in` passed the record, which the running context now
        // owns; the borrow ends here.
        let record = unsafe { &mut *ctx };
        switched_in(record, transfer.fctx, asan(), ptr::null_mut());
        (record.entry, record.data)
    };
    // SAFETY: the creator's entry over its own data, as `Context::new`'s caller promised.
    unsafe { entry(data) };
    // SAFETY: as above; a shared borrow, which ends before the switch.
    unsafe { &*ctx }.end.set(true);
    // SAFETY: the running context; an ended one is never switched back in.
    unsafe { swap_out(ctx) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn record_layout_keeps_the_offsets_c_reads() {
        assert_eq!(std::mem::offset_of!(Context, stack), 16);
        assert_eq!(std::mem::offset_of!(Context, stack_size), 24);
        assert_eq!(std::mem::offset_of!(Context, end), 64);
    }

    struct Log {
        ctx: core::mem::MaybeUninit<Context>,
        steps: Vec<&'static str>,
    }

    unsafe extern "C" fn entry(data: *mut c_void) {
        let log = data.cast::<Log>();
        // SAFETY: the test's log, which only the running stack touches.
        unsafe {
            (*log).steps.push("entry");
            swap_out((*log).ctx.as_mut_ptr());
            (*log).steps.push("resumed");
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri runs no assembly")]
    fn a_context_switches_onto_its_stack_and_back() {
        let mut log = Log {
            ctx: core::mem::MaybeUninit::uninit(),
            steps: Vec::new(),
        };
        let log = &raw mut log;
        // SAFETY: the record stays in `log` until dropped, and each switch is the stack's turn.
        unsafe {
            let ctx = (*log).ctx.as_mut_ptr();
            ctx.write(Context::new(64 * 1024, 0, entry, log.cast()).unwrap());
            assert!(!swap_in(ctx));
            (*log).steps.push("main");
            assert!(swap_in(ctx));
            ctx.drop_in_place();
            assert_eq!((*log).steps, ["entry", "main", "resumed"]);
        }
    }
}
