//! A coroutine's context: its stack and the switch onto it through boost.context's
//! fcontext (`fcontext.rs`). `AddressSanitizer` follows a stack switch only through its
//! fiber annotations, so they run whenever the process has its runtime, and
//! `ThreadSanitizer` through its own, in tools/tsan.py's build (`cfg(tsan)`). With
//! [`THREAD`] in its flags the context is a thread instead (`thread.rs`).

use crate::CCast;
use crate::coroutine::Coroutine;
use crate::coroutine::fcontext::{Transfer, jump_fcontext, make_fcontext};
use crate::coroutine::stack::Stack;
use crate::coroutine::state::Co;
use crate::coroutine::thread::{self, Entry, Started, THREAD};
use core::cell::Cell;
use core::ffi::{c_int, c_void};
use std::ptr;
use std::sync::OnceLock;

/// A coroutine's context record, which follows its state in its [`Native`]
/// record and never moves once switched in, so the switch hands its stack the
/// coroutine's `Co`. Both stacks reach it through that handle and only share it: what
/// a switch changes sits in cells. Dropping it frees its stack, or joins its thread,
/// so it is dropped only once its coroutine ended (or never ran).
pub struct Context {
    /// The suspended context.
    fctx: Cell<*mut c_void>,
    swap_fctx: Cell<*mut c_void>,
    /// The stack it runs on and its flags: none under [`THREAD`].
    stack: Stack,
    /// What the first switch in runs, in place: empty under [`THREAD`].
    entry: Entry,
    /// The stack that last switched in, which `swap_out` returns to (`AddressSanitizer` only).
    asan_bottom: Cell<*const c_void>,
    asan_size: Cell<usize>,
    /// The context's `ThreadSanitizer` fiber, and the one that last switched it in.
    #[cfg(tsan)]
    tsan_fiber: Option<tsan::Fiber>,
    #[cfg(tsan)]
    tsan_caller: Cell<Option<tsan::To<'static>>>,
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

    /// Whether its entry returned (upstream's `Coroutine::is_end`).
    #[must_use]
    pub fn ended(&self) -> bool {
        self.end.get()
    }

    /// A context on `stack` (switched into at `fctx`) or `thread`, with no entry yet.
    #[inline]
    fn made(stack: Stack, fctx: *mut c_void, thread: Option<Box<Started>>) -> Self {
        Self {
            fctx: Cell::new(fctx),
            swap_fctx: Cell::new(ptr::null_mut()),
            stack,
            entry: Entry::empty(),
            asan_bottom: Cell::new(ptr::null()),
            asan_size: Cell::new(0),
            #[cfg(tsan)]
            tsan_fiber: tsan::create(),
            #[cfg(tsan)]
            tsan_caller: Cell::new(None),
            end: Cell::new(false),
            thread,
        }
    }
}

impl Native {
    /// A record whose context runs `entry` on a stack it allocates: the entry is
    /// moved once, into the boxed record, where it runs. A [`THREAD`] context starts its
    /// thread with `entry` instead.
    ///
    /// # Errors
    /// No stack (out of memory), or the errno of a [`THREAD`] context whose
    /// thread failed to start.
    pub fn new(stack_size: usize, flags: u32, entry: Entry) -> Result<Box<Self>, Unmade> {
        if flags & THREAD != 0 {
            let thread = thread::start(entry).map_err(Unmade::Thread)?;
            return Ok(Box::new(Self {
                co: Coroutine::idle(),
                ctx: Context::made(Stack::none(flags), ptr::null_mut(), Some(thread)),
            }));
        }
        let stack_size = stack_size.c_cast::<u32>();
        let size = stack_size as usize;
        let stack = Stack::alloc(stack_size, flags).ok_or(Unmade::NoStack)?;
        let top = stack.base().wrapping_add(size);
        // SAFETY: a fresh `size`-byte stack, which boost builds the context down from.
        let fctx = unsafe { make_fcontext(top.cast(), size, first_frame) };
        let mut native = Box::new(Self {
            co: Coroutine::idle(),
            ctx: Context::made(stack, fctx, None),
        });
        native.ctx.entry = entry;
        Ok(native)
    }
}

/// Why [`Native::new`] made no record.
#[derive(Debug)]
pub enum Unmade {
    /// The stack's allocation failed.
    NoStack,
    /// The [`THREAD`] context's thread failed to start, with this errno.
    Thread(c_int),
}

/// A stack's fake stack as `AddressSanitizer` set it aside: none, or what a
/// [`StartSwitch`] saved into it, which only a [`FinishSwitch`] takes back. It is
/// neither `Copy` nor `Clone`, so each one saved is handed back once, after the switch
/// back that its start announced.
#[repr(transparent)]
struct FakeStack(*mut c_void);

impl FakeStack {
    const NONE: Self = Self(ptr::null_mut());
}

/// `__sanitizer_start_switch_fiber(fake_stack_save, bottom, size)`: records the stack
/// the next switch lands on and, given somewhere to save it, sets the current fake
/// stack aside there. It only writes through `fake_stack_save` and dereferences
/// neither the stack nor its bottom, so it is safe to call.
type StartSwitch = extern "C" fn(Option<&mut FakeStack>, *const c_void, usize);
/// `__sanitizer_finish_switch_fiber(fake_stack_save, bottom_old, size_old)`: restores
/// the fake stack a start saved (none: keeps the current one) and, given somewhere to
/// write them, reports the stack that switched in. A [`FakeStack`] is none or a start's
/// save, and the out-parameters are only written, so it is safe to call.
type FinishSwitch = extern "C" fn(FakeStack, Option<&mut *const c_void>, Option<&mut usize>);

// ThreadSanitizer's fiber calls. Each coroutine runs as its own fiber: TSan keeps a
// thread's setjmp buffers in one list and, on each setjmp, drops those taken below its
// stack pointer, so a setjmp on a coroutine stack mapped above the thread's stack would
// drop the thread's own buffers, and a later bailout to one aborts with "can't find
// longjmp buf" (#3400). A fiber keeps a list of its own.
#[cfg(tsan)]
mod tsan {
    use core::ffi::{c_uint, c_void};
    use core::marker::PhantomData;
    use core::ptr::NonNull;

    /// A context's own fiber, made with it and destroyed with it: once it ended or
    /// never ran, so while the fiber is not running.
    #[repr(transparent)]
    pub(super) struct Fiber(NonNull<c_void>);

    /// A live fiber to switch to: a [`Fiber`], borrowed, or the fiber that switched a
    /// context in, which waits in `swap_in` until that context switches out, the one
    /// switch to it made here. Only this module makes one.
    #[repr(transparent)]
    #[derive(Clone, Copy)]
    pub(super) struct To<'a>(NonNull<c_void>, PhantomData<&'a Fiber>);

    unsafe extern "C" {
        /// The running fiber, never null, so `None` never comes back.
        #[link_name = "__tsan_get_current_fiber"]
        pub(super) safe fn current() -> Option<To<'static>>;
        safe fn __tsan_create_fiber(flags: c_uint) -> Option<Fiber>;
        /// Switches to a live fiber.
        #[link_name = "__tsan_switch_to_fiber"]
        safe fn switch_to(fiber: To<'_>, flags: c_uint);
    }

    /// A new fiber, `None` if `ThreadSanitizer` made none.
    pub(super) fn create() -> Option<Fiber> {
        __tsan_create_fiber(0)
    }

    impl Fiber {
        #[inline(always)]
        pub(super) fn to(&self) -> To<'_> {
            To(self.0, PhantomData)
        }
    }

    impl To<'_> {
        /// Inlined: TSan counts each instrumented function's return on the running
        /// fiber's shadow stack, so no frame may return between the switch and the
        /// jump it announces. One that did would pop the new fiber's stack below its
        /// start, and its next call writes under it.
        #[inline(always)]
        pub(super) fn switch(self) {
            switch_to(self, 0);
        }
    }

    impl Drop for Fiber {
        fn drop(&mut self) {
            /// The fiber a dropped `Fiber` held: only this drop makes one, once per fiber.
            #[repr(transparent)]
            struct Released(NonNull<c_void>);
            unsafe extern "C" {
                safe fn __tsan_destroy_fiber(fiber: Released);
            }
            __tsan_destroy_fiber(Released(self.0));
        }
    }
}

/// `__sanitizer_{start,finish}_switch_fiber`, when `AddressSanitizer` is loaded.
fn asan() -> Option<(StartSwitch, FinishSwitch)> {
    /// `RTLD_DEFAULT`, the only handle this passes.
    #[repr(transparent)]
    struct Loaded(*mut c_void);
    /// `"__sanitizer_start_switch_fiber"`, the one name of its type.
    #[repr(transparent)]
    struct Start(*const core::ffi::c_char);
    /// `"__sanitizer_finish_switch_fiber"`, the one name of its type.
    #[repr(transparent)]
    struct Finish(*const core::ffi::c_char);
    // dlsym(3), once per sanitizer function: it reads the name up to its NUL and
    // returns that function's address, which the sanitizer interface declares with
    // the signature returned (a nullable `T *` is an `Option<&mut T>`, and a
    // `FakeStack` a `void *`), or null.
    unsafe extern "C" {
        #[link_name = "dlsym"]
        safe fn start_switch(handle: Loaded, name: Start) -> Option<StartSwitch>;
        #[link_name = "dlsym"]
        #[expect(
            clashing_extern_declarations,
            reason = "dlsym, ABI-identical: each declaration takes the one name whose \
                      function has the signature it returns"
        )]
        safe fn finish_switch(handle: Loaded, name: Finish) -> Option<FinishSwitch>;
    }
    static FIBERS: OnceLock<Option<(StartSwitch, FinishSwitch)>> = OnceLock::new();
    *FIBERS.get_or_init(|| {
        let start = Start(c"__sanitizer_start_switch_fiber".as_ptr());
        let finish = Finish(c"__sanitizer_finish_switch_fiber".as_ptr());
        start_switch(Loaded(libc::RTLD_DEFAULT), start)
            .zip(finish_switch(Loaded(libc::RTLD_DEFAULT), finish))
    })
}

const _: () = assert!(
    core::mem::offset_of!(Native, ctx) == size_of::<Coroutine>(),
    "a record's context follows its coroutine"
);

impl Co {
    /// Its context, which follows it in the [`Native`] record facade.rs boxed for it.
    fn context(self) -> *const Context {
        self.ptr().wrapping_add(1).cast()
    }

    /// Its context, shared for `f`. Both stacks (or threads) take only shared borrows
    /// of it, so `f` may switch away.
    fn read_context<R>(self, f: impl FnOnce(&Context) -> R) -> R {
        // SAFETY: only the scheduler's hooks below and the first frame reach this, with
        // a live coroutine that facade.rs boxed in a `Native`; shared, until `f` returns.
        f(unsafe { &*self.context() })
    }
}

/// The scheduler's `swap_in` hook: switches from the caller's stack onto `co`'s
/// context, returning when it switches out: whether it ended. The scheduler passes
/// a live coroutine of its own (boxed in a [`Native`]) that has not ended and is not
/// running, whose context stays put from here on.
pub(crate) fn swap_in(co: Co) -> bool {
    let asan = asan();
    let mut fake_stack = FakeStack::NONE;
    // The jump sits in the frame that switches fibers, which returns on the fiber it
    // entered on (see `tsan::To::switch`).
    let transfer = co.read_context(|ctx| {
        if let Some(thread) = &ctx.thread {
            let ended = thread.swap_in();
            ctx.end.set(ended);
            return Err(ended);
        }
        if let Some((start, _)) = asan {
            // Announces the switch onto the context's stack.
            start(
                Some(&mut fake_stack),
                ctx.stack.base().cast(),
                ctx.stack.size(),
            );
        }
        #[cfg(tsan)]
        {
            ctx.tsan_caller.set(tsan::current());
            // The context's own fiber, which runs from the jump on.
            if let Some(fiber) = &ctx.tsan_fiber {
                fiber.to().switch();
            }
        }
        // SAFETY: the context's suspended state; it resumes with its coroutine.
        Ok(unsafe { jump_fcontext(ctx.fctx.get(), co) })
    });
    let transfer = match transfer {
        Ok(transfer) => transfer,
        Err(ended) => return ended,
    };
    // The context switched back out, so this stack owns the record again.
    co.read_context(|ctx| {
        ctx.fctx.set(transfer.fctx);
        if let Some((_, finish)) = asan {
            // Completes the switch back that `swap_out` started.
            finish(fake_stack, None, None);
        }
        ctx.end.get()
    })
}

/// The scheduler's `swap_out` hook: switches from `co`'s stack back to the one that
/// last switched it in. The scheduler passes the running coroutine.
pub(crate) fn swap_out(co: Co) {
    let asan = asan();
    let mut fake_stack = FakeStack::NONE;
    // As in `swap_in`, the jump sits in the frame that switches fibers.
    let transfer = co.read_context(|ctx| {
        if let Some(thread) = &ctx.thread {
            thread.swap_out();
            return None;
        }
        if let Some((start, _)) = asan {
            // A finished context never resumes, so ASan may free its fake stack now.
            let save = (!ctx.end.get()).then_some(&mut fake_stack);
            // Announces the switch back to the stack that switched in.
            start(save, ctx.asan_bottom.get(), ctx.asan_size.get());
        }
        // The fiber that switched this context in, which runs from the jump on.
        #[cfg(tsan)]
        if let Some(caller) = ctx.tsan_caller.get() {
            caller.switch();
        }
        // SAFETY: the suspended caller of `swap_in`.
        Some(unsafe { jump_fcontext(ctx.swap_fctx.get(), co) })
    });
    let Some(transfer) = transfer else {
        return;
    };
    // Switched back in, so this stack owns the record again.
    co.read_context(|ctx| switched_in(ctx, transfer.fctx, asan, fake_stack));
}

/// Records the stack that switched in and completes the sanitizer's switch.
fn switched_in(
    ctx: &Context,
    swap_fctx: *mut c_void,
    asan: Option<(StartSwitch, FinishSwitch)>,
    fake_stack: FakeStack,
) {
    ctx.swap_fctx.set(swap_fctx);
    if let Some((_, finish)) = asan {
        let (mut bottom, mut size) = (ctx.asan_bottom.get(), ctx.asan_size.get());
        // `fake_stack` is what this stack's last start saved (none on a first switch).
        finish(fake_stack, Some(&mut bottom), Some(&mut size));
        ctx.asan_bottom.set(bottom);
        ctx.asan_size.set(size);
    }
}

/// The context's first frame: runs the entry, marks the end and switches out for good.
/// `swap_in` jumped here with its coroutine, which now runs.
extern "C" fn first_frame(transfer: Transfer) {
    let co = transfer.data;
    co.read_context(|record| {
        switched_in(record, transfer.fctx, asan(), FakeStack::NONE);
        record.entry.run_held();
    });
    co.read_context(|ctx| ctx.end.set(true));
    // The running coroutine; an ended one is never switched back in.
    swap_out(co);
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Log {
        co: Option<Co>,
        steps: Vec<&'static str>,
    }

    /// The coroutine of the log's record.
    unsafe fn co(log: *mut Log) -> Co {
        unsafe { (*log).co }.unwrap()
    }

    fn entry(log: *mut Log) {
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
            let native = Native::new(64 * 1024, 0, Entry::of(move || entry(log))).unwrap();
            (*log).co = Some(Co::leak(native));
            assert!(!swap_in(co(log)));
            (*log).steps.push("main");
            assert!(swap_in(co(log)));
            drop(Box::from_raw(co(log).ptr().cast::<Native>()));
            assert_eq!((*log).steps, ["entry", "main", "resumed"]);
        }
    }
}
