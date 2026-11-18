//! Zend bailouts (`zend_bailout`, a `longjmp` to the nearest `zend_try`) around
//! code Rust calls. A bailout must never jump over a Rust frame, so Rust traps
//! it in C, returns normally, and hands a [`Bailout`] back to the C or C++ that
//! called it, which re-raises it with `zend_bailout()`. See docs/zend.md.
use crate::{
    Value, Zvals,
    host::{Callee, CalleePtr},
};
use std::ffi::c_int;

unsafe extern "C" {
    /// The shim's whole-thread exit mark: a C thread-local of its own, not
    /// executor state, read on any thread; no arguments, and nothing runs.
    safe fn swr_zend_guard_pending() -> c_int;
    /// `zend_call_function` inside `zend_try` (nonzero for a bailout) over a
    /// callable's cache (an empty one resolves `name`, when given), its lent
    /// arguments and a live return slot: `result` says whether it succeeded.
    safe fn swr_zend_try_call(
        cache: CalleePtr<'_>,
        args: Zvals<'_>,
        name: Option<&Value>,
        retval: &Value,
        result: &mut c_int,
    ) -> c_int;
}

/// A trapped bailout. Its owner finishes its Rust work, then reports it to
/// the C/C++ caller (for example as a flag), which calls `zend_bailout()`.
#[must_use = "a trapped bailout must be re-raised by the C/C++ caller"]
#[derive(Debug)]
pub struct Bailout;

// Per thread on a thread-safe PHP, as the shim's `ZEND_TLS` guard state; one
// process flag otherwise, with no TLS lookup on each check. Tests (which
// unwind) keep it per thread, as they run in parallel.
#[cfg(any(zts, panic = "unwind"))]
thread_local! {
    static PENDING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(not(any(zts, panic = "unwind")))]
static PENDING: Flag = Flag(std::sync::atomic::AtomicBool::new(false));

/// The process's pending flag, with `Cell`'s interface.
#[cfg(not(any(zts, panic = "unwind")))]
struct Flag(std::sync::atomic::AtomicBool);

#[cfg(not(any(zts, panic = "unwind")))]
impl Flag {
    fn get(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }
    fn set(&self, value: bool) {
        self.0.store(value, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The native destination of a contained escape. Whole-thread provenance remains
/// pending until the request boundary acknowledges it, including across PHP catches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BailoutKind {
    Zend,
    ThreadExit,
}

impl Bailout {
    /// Native provenance of this contained escape; no PHP operation is run.
    #[must_use]
    pub fn kind(&self) -> BailoutKind {
        if thread_exit_pending() {
            BailoutKind::ThreadExit
        } else {
            BailoutKind::Zend
        }
    }

    /// Leave the bailout for the generated native boundary ([`crate::exports`]),
    /// after the handler or export has completed its Rust cleanup.
    pub fn defer(self) {
        PENDING.set(true);
    }
}

/// [`Bailout::defer`] of a failed result.
pub fn defer(result: Result<(), Bailout>) {
    if let Err(bailout) = result {
        bailout.defer();
    }
}

/// Whole-thread provenance remains native and survives ordinary PHP catches.
/// Only `Swoole\Thread` marks one, and it needs ZTS: never on NTS.
#[inline]
pub(crate) fn thread_exit_pending() -> bool {
    cfg!(zts) && swr_zend_guard_pending() != 0
}

/// Whether a bailout waits for its native boundary. No error is raised meanwhile:
/// upstream never ran past the first bailout.
#[must_use]
pub fn pending() -> bool {
    PENDING.get() || thread_exit_pending()
}

/// `value`, or a [`Bailout`] while one is pending, which stays pending.
/// # Errors
/// A bailout is pending.
pub fn outcome<T>(value: T) -> Result<T, Bailout> {
    if pending() { Err(Bailout) } else { Ok(value) }
}

/// Hand a contained bailout to an outer C trampoline after Rust cleanup.
/// The caller must re-raise a true result in C; consuming the pending bit here
/// prevents a caught shutdown bailout from poisoning a later request operation.
#[must_use = "the C caller must re-raise a returned bailout"]
#[inline]
pub fn relay_to_c(result: Result<(), Bailout>) -> bool {
    defer(result);
    if PENDING.get() {
        PENDING.set(false);
        true
    } else {
        thread_exit_pending()
    }
}

/// A native trap's flag as a result, without resuming a PHP bailout.
/// # Errors
/// Returns `Bailout` when the native leaf reports a nonzero contained-bailout flag.
pub fn trapped(bailed: c_int) -> Result<(), Bailout> {
    if bailed != 0 || thread_exit_pending() {
        Err(Bailout)
    } else {
        Ok(())
    }
}

/// `zend_call_function` of `cache`'s callable (an empty cache resolves `name`,
/// when given) with `args`, into `retval`, inside `zend_try`: whether the call
/// succeeded. Nothing is called, and the result slot is left as it was, while
/// a bailout is pending: upstream never ran past it.
/// # Errors
/// The callable bailed out (`exit()`, a fatal error), or a bailout is pending.
pub fn call(
    cache: Callee<'_>,
    args: Zvals<'_>,
    name: Option<&Value>,
    retval: &Value,
) -> Result<bool, Bailout> {
    if pending() {
        return Err(Bailout);
    }
    let mut result = 0;
    if swr_zend_try_call(cache.cache, args, name, retval, &mut result) != 0 {
        return Err(Bailout);
    }
    Ok(result != 0)
}
