//! A coroutine's context: its stack and the switch onto it through boost.context's
//! fcontext (thirdparty/boost/asm). `AddressSanitizer` follows a stack switch only through its
//! fiber annotations, so they run whenever the process has its runtime. With
//! [`THREAD`] in its flags the context is a thread instead (`thread.rs`).

use crate::Coroutine;
use crate::stack::{swr_v1_context_stack_alloc, swr_v1_context_stack_free};
use crate::thread::{self, THREAD};
use core::ffi::{c_int, c_void};
use std::ptr;
use std::sync::OnceLock;

/// A coroutine's context record, which follows its state in its [`Native`]
/// record and never moves, so the switch may hand out its address. Both stacks
/// reach it through raw pointers, and neither holds a borrow across a switch.
#[repr(C)]
pub struct Context {
    /// The suspended context, or the [`THREAD`] context's thread.
    pub(crate) fctx: *mut c_void,
    swap_fctx: *mut c_void,
    stack: *mut u8,
    stack_size: u32,
    flags: u32,
    pub(crate) entry: unsafe extern "C" fn(*mut c_void),
    pub(crate) data: *mut c_void,
    /// The stack that last switched in, which `swap_out` returns to (`AddressSanitizer` only).
    asan_bottom: *const c_void,
    asan_size: usize,
    pub(crate) end: bool,
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

    /// A [`THREAD`] context's record before its thread starts: no stack.
    pub(crate) const fn threaded(
        flags: u32,
        entry: unsafe extern "C" fn(*mut c_void),
        data: *mut c_void,
    ) -> Self {
        Self {
            fctx: ptr::null_mut(),
            swap_fctx: ptr::null_mut(),
            stack: ptr::null_mut(),
            stack_size: 0,
            flags,
            entry,
            data,
            asan_bottom: ptr::null(),
            asan_size: 0,
            end: false,
        }
    }
}

#[repr(C)]
struct Transfer {
    fctx: *mut c_void,
    data: *mut c_void,
}

unsafe extern "C" {
    fn swoole_make_fcontext(
        stack: *mut c_void,
        size: usize,
        entry: extern "C" fn(Transfer),
    ) -> *mut c_void;
    fn swoole_jump_fcontext(to: *mut c_void, data: *mut c_void) -> Transfer;
}

/// `SW_ERROR_MALLOC_FAIL`.
const SW_ERROR_MALLOC_FAIL: c_int = 501;

type StartSwitch = unsafe extern "C" fn(*mut *mut c_void, *const c_void, usize);
type FinishSwitch = unsafe extern "C" fn(*mut c_void, *mut *const c_void, *mut usize);

/// `__sanitizer_{start,finish}_switch_fiber`, when `AddressSanitizer` is loaded.
fn asan() -> Option<(StartSwitch, FinishSwitch)> {
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
        // SAFETY: the sanitizer interface declares it with this signature.
        let start = unsafe { std::mem::transmute::<*mut c_void, StartSwitch>(start) };
        // SAFETY: as above.
        let finish = unsafe { std::mem::transmute::<*mut c_void, FinishSwitch>(finish) };
        Some((start, finish))
    })
}

/// Allocates the stack and makes the context that runs `entry(data)` on it. Out of
/// memory is upstream's fatal error, then exit 254. A [`THREAD`] context starts
/// its thread instead and returns the errno when that fails, else 0.
///
/// # Safety
/// `ctx` is writable and stays at this address until [`swr_v1_context_free`].
pub unsafe fn swr_v1_context_init(
    ctx: *mut Context,
    stack_size: usize,
    flags: u32,
    entry: unsafe extern "C" fn(*mut c_void),
    data: *mut c_void,
) -> c_int {
    if flags & THREAD != 0 {
        // SAFETY: `ctx` is writable, as the caller promises.
        unsafe { ctx.write(Context::threaded(flags, entry, data)) };
        // SAFETY: initialized above; it stays put until free.
        return unsafe { thread::start(ctx) };
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream keeps the size as uint32_t"
    )]
    let stack_size = stack_size as u32;
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
        // SAFETY: exit has no preconditions.
        unsafe { libc::exit(254) };
    }
    // SAFETY: a fresh `size`-byte stack, which boost builds the context down from.
    let fctx = unsafe { swoole_make_fcontext(stack.wrapping_add(size).cast(), size, first_frame) };
    let context = Context {
        fctx,
        swap_fctx: ptr::null_mut(),
        stack,
        stack_size,
        flags,
        entry,
        data,
        asan_bottom: ptr::null(),
        asan_size: 0,
        end: false,
    };
    // SAFETY: `ctx` is writable, as the caller promises.
    unsafe { ctx.write(context) };
    0
}

/// # Safety
/// `ctx` came from [`swr_v1_context_init`] and is not running.
pub unsafe fn swr_v1_context_free(ctx: *mut Context) {
    // SAFETY: a live context, as the caller promises.
    if unsafe { (*ctx).flags } & THREAD != 0 {
        // SAFETY: a started thread context.
        return unsafe { thread::free(ctx) };
    }
    // SAFETY: a live context, as the caller promises.
    let ctx = unsafe { &mut *ctx };
    if !ctx.stack.is_null() {
        // SAFETY: allocated by init with this size and these flags, freed once.
        unsafe { swr_v1_context_stack_free(ctx.stack.cast(), ctx.stack_size as usize, ctx.flags) };
        ctx.stack = ptr::null_mut();
    }
}

/// Switches from the caller's stack onto `ctx`'s, returning when it switches out.
///
/// # Safety
/// `ctx` came from [`swr_v1_context_init`], has not ended, and is not running.
pub unsafe fn swr_v1_context_swap_in(ctx: *mut Context) {
    // SAFETY: a live context, as the caller promises.
    if unsafe { (*ctx).flags } & THREAD != 0 {
        // SAFETY: as above.
        return unsafe { thread::swap_in(ctx) };
    }
    let asan = asan();
    let mut fake_stack = ptr::null_mut();
    let fctx = {
        // SAFETY: a live context, as the caller promises; the borrow ends here.
        let ctx = unsafe { &*ctx };
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
        ctx.fctx
    };
    // SAFETY: `fctx` is the context's suspended state; it resumes with its record.
    let transfer = unsafe { swoole_jump_fcontext(fctx, ctx.cast()) };
    // SAFETY: the context switched back out, so this stack owns the record again.
    unsafe { (*ctx).fctx = transfer.fctx };
    if let Some((_, finish)) = asan {
        // SAFETY: completes the switch back that `swap_out` started.
        unsafe { finish(fake_stack, ptr::null_mut(), ptr::null_mut()) };
    }
}

/// Switches from `ctx`'s stack back to the one that last switched it in.
///
/// # Safety
/// `ctx` is the running context.
pub unsafe fn swr_v1_context_swap_out(ctx: *mut Context) {
    // SAFETY: the running context, as the caller promises.
    if unsafe { (*ctx).flags } & THREAD != 0 {
        // SAFETY: as above.
        return unsafe { thread::swap_out(ctx) };
    }
    let asan = asan();
    let mut fake_stack = ptr::null_mut();
    let swap_fctx = {
        // SAFETY: the running context's record; the borrow ends here.
        let ctx = unsafe { &*ctx };
        if let Some((start, _)) = asan {
            // A finished context never resumes, so ASan may free its fake stack now.
            let save = if ctx.end {
                ptr::null_mut()
            } else {
                &raw mut fake_stack
            };
            // SAFETY: announces the switch back to the stack that switched in.
            unsafe { start(save, ctx.asan_bottom, ctx.asan_size) };
        }
        ctx.swap_fctx
    };
    // SAFETY: `swap_fctx` is the suspended caller of `swap_in`.
    let transfer = unsafe { swoole_jump_fcontext(swap_fctx, ctx.cast()) };
    // SAFETY: switched back in, so this stack owns the record again.
    unsafe { switched_in(ctx, transfer.fctx, asan, fake_stack) };
}

/// Records the stack that switched in and completes the sanitizer's switch.
///
/// # Safety
/// `ctx` is the running context's record, which `swap_fctx` just switched from.
unsafe fn switched_in(
    ctx: *mut Context,
    swap_fctx: *mut c_void,
    asan: Option<(StartSwitch, FinishSwitch)>,
    fake_stack: *mut c_void,
) {
    // SAFETY: as the caller promises.
    let ctx = unsafe { &mut *ctx };
    ctx.swap_fctx = swap_fctx;
    if let Some((_, finish)) = asan {
        // SAFETY: the record's fields are live out-pointers.
        unsafe { finish(fake_stack, &raw mut ctx.asan_bottom, &raw mut ctx.asan_size) };
    }
}

/// The context's first frame: runs the entry, marks the end and switches out for good.
extern "C" fn first_frame(transfer: Transfer) {
    let ctx = transfer.data.cast::<Context>();
    // SAFETY: `swap_in` passed the record, which the running context now owns.
    unsafe { switched_in(ctx, transfer.fctx, asan(), ptr::null_mut()) };
    let (entry, data) = {
        // SAFETY: the record is live while its context runs; the borrow ends here.
        let ctx = unsafe { &*ctx };
        (ctx.entry, ctx.data)
    };
    // SAFETY: the creator's entry over its own data.
    unsafe { entry(data) };
    // SAFETY: as above.
    unsafe { (*ctx).end = true };
    // SAFETY: the running context; an ended one is never switched back in.
    unsafe { swr_v1_context_swap_out(ctx) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn record_layout_matches_the_c_header() {
        assert_eq!(size_of::<Context>(), 72);
        assert_eq!(std::mem::offset_of!(Context, stack), 16);
        assert_eq!(std::mem::offset_of!(Context, stack_size), 24);
        assert_eq!(std::mem::offset_of!(Context, end), 64);
    }
}
