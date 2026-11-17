//! Callables kept past the call that passed them, with their bound arguments:
//! Swoole's `zend::Function`, as timer and event callbacks hold them.
use crate::{
    Call, Emalloc, Invalid,
    bailout::{self, Bailout},
    sys::{IS_LONG, swr_zend_callable, zend_value, zval},
};
use std::{ffi::c_int, ptr};

// Callable is a linear pointer handle made only here, so C receives a live
// record in the state its type says. Parsed retains Rust ownership until keep.
unsafe extern "C" {
    /// `sw_zend_fci_cache_persist`: one more reference to the object and the
    /// closure of the function `parse` resolved. `keep` hands its boxed
    /// record to the native owner after this returns.
    safe fn swr_zend_callable_hold(parsed: &mut swr_zend_callable);
    /// `sw_zend_fci_params_discard` and `sw_zend_fci_cache_discard` when no
    /// release can run PHP code (no arguments, and neither the object nor the
    /// closure loses its last reference), then nonzero; otherwise zero, and
    /// the record keeps everything. The handle is spent either way: `release`
    /// finishes with the record, as `keep` does with a `Parsed` one.
    safe fn swr_zend_callable_release_quiet(callable: Callable) -> c_int;
    /// The release `swr_zend_callable_release_quiet` declined, inside
    /// `zend_try`, as `swr_zend_try` returns. The handle is spent, as there.
    safe fn swr_zend_try_callable_release(callable: Callable) -> c_int;
    /// `efree` of the record of a callable whose references were released.
    #[link_name = "swr_zend_efree"]
    #[expect(
        clashing_extern_declarations,
        reason = "Callable and the allocator's NonNull both have the C pointer ABI; this handle proves the record is owned"
    )]
    safe fn free_released(callable: Callable);
}

/// An emalloc'd `swr_zend_callable`, laid out as Swoole's `zend::Function`, so
/// C++ can call it as one, that holds its arguments, object and closure
/// ([`Parsed::keep`]).
#[derive(Debug)]
#[repr(transparent)]
pub struct Callable(*mut swr_zend_callable);

/// A callable [`Callable::parse`] read from a call, made only once Zend
/// resolved it (its cache names a function), whose arguments, object and
/// closure are still the frame's: [`Parsed::keep`] makes it a [`Callable`], or
/// [`Parsed::discard`] frees it. Releasing it as a `Callable` would release
/// references it never took.
pub struct Parsed(Box<swr_zend_callable, Emalloc>);

impl std::fmt::Debug for Parsed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Parsed")
            .field(&Box::as_ptr(&self.0))
            .finish()
    }
}

impl Callable {
    /// `Z_PARAM_FUNC` of argument `n`, then `Z_PARAM_VARIADIC('*')` of the
    /// arguments after it, which stay in the frame until [`Parsed::keep`].
    /// # Errors
    /// Argument `n` is not callable, or is a magic-method callable, whose cache
    /// names no function; the parameter error is raised.
    pub fn parse(call: &Call, n: u32) -> Result<Parsed, Invalid> {
        // Zend's allocator, for one empty callable (`ecalloc`).
        let mut this = Parsed(Box::new_in(
            crate::zeroed::<swr_zend_callable>(),
            Emalloc::default(),
        ));
        let callable = this.record();
        if let Err(invalid) = call.callable(n, &mut callable.fci, &mut callable.fcc) {
            this.discard();
            return Err(invalid);
        }
        let callable = this.record();
        // Zend resolves a `__call` or `__callStatic` method to a trampoline and
        // releases it, leaving a cache that names no function to keep.
        if callable.fcc.function_handler.is_null() {
            call.argument_type_error(
                n,
                c"must be a callable that is not a __call or __callStatic method",
            );
            this.discard();
            return Err(Invalid);
        }
        callable.fci.param_count = call.num_args().saturating_sub(n);
        callable.fci.params = if callable.fci.param_count > 0 {
            call.arg(n + 1)
        } else {
            ptr::null_mut()
        };
        Ok(this)
    }

    /// The kept callable's cache, bound arguments and callable zval, for a call:
    /// the record keeps them while borrowed, and nothing changes the cache.
    #[must_use]
    pub fn parts(&self) -> (crate::Fcc<'_>, u32, *mut zval, *mut zval) {
        // SAFETY: a parsed callable, borrowed shared, so a second call's borrow
        // leaves the first's cache borrow valid; nothing writes the record while
        // `self` is lent.
        let callable = unsafe { &*self.0 };
        // The callable zval, without a borrow: the caller may write through it.
        let name = self
            .0
            .wrapping_byte_add(
                std::mem::offset_of!(swr_zend_callable, fci)
                    + std::mem::offset_of!(crate::sys::zend_fcall_info, function_name),
            )
            .cast();
        (
            // A kept callable holds its function's object and closure.
            crate::Fcc(&callable.fcc),
            callable.fci.param_count,
            callable.fci.params,
            name,
        )
    }

    /// The kept callable's cache and bound arguments, lent for
    /// [`crate::host::function_call`]: the record keeps both while borrowed.
    #[must_use]
    #[inline]
    pub fn call_parts(&self) -> (crate::host::Callee<'_>, crate::Zvals<'_>) {
        let (_, argc, argv, _) = self.parts();
        // The record's cache, without a borrow: Zend may write a cache it calls.
        let cache = self
            .0
            .wrapping_byte_add(std::mem::offset_of!(swr_zend_callable, fcc))
            .cast();
        (
            crate::host::Callee(cache, std::marker::PhantomData),
            crate::Zvals::counted(argv, argc),
        )
    }

    /// Drop the arguments, object and closure `keep` held, then free the
    /// callable (`sw_zend_fci_params_discard`, `sw_zend_fci_cache_discard`).
    /// # Errors
    /// A destructor bailed out; the callable is freed anyway.
    pub fn release(self) -> Result<(), Bailout> {
        let record = self.0;
        let released = if swr_zend_callable_release_quiet(self) != 0 {
            Ok(())
        } else {
            // The callable, still kept: the quiet release touched nothing.
            bailout::trapped(swr_zend_try_callable_release(Callable(record)))
        };
        // Emalloc'd by `parse`, and released above.
        free_released(Callable(record));
        released
    }
}

impl Parsed {
    /// The callable's record, which only this owner reaches until it is kept.
    fn record(&mut self) -> &mut swr_zend_callable {
        &mut self.0
    }

    /// Keep the callable past this call: copy its arguments out of the frame
    /// (`sw_zend_fci_params_persist`), with `first` before them when given (a
    /// tick's timer id), and hold its object and closure
    /// (`sw_zend_fci_cache_persist`).
    #[must_use]
    #[inline]
    pub fn keep(mut self, first: Option<i64>) -> Callable {
        let callable = self.record();
        let count = callable.fci.param_count as usize;
        let lead = usize::from(first.is_some());
        if count + lead > 0 {
            keep_params(callable, first, count, lead);
        }
        swr_zend_callable_hold(&mut self.0);
        Callable(Box::into_raw_with_allocator(self.0).0)
    }

    /// Free a callable that was never kept.
    pub fn discard(self) {
        // No argument references were taken from the frame.
        drop(self);
    }
}

/// Copy bound arguments only when there are any; keep the empty-callable path small.
fn keep_params(callable: &mut swr_zend_callable, first: Option<i64>, count: usize, lead: usize) {
    // Zend's allocator, for the arguments.
    let mut params = Vec::with_capacity_in(count + lead, Emalloc::default());
    if let Some(value) = first {
        params.push(long(value));
    }
    // The frame's arguments, which it holds for this call.
    let args = crate::Zvals::counted(callable.fci.params, callable.fci.param_count);
    for value in args.values() {
        params.push(value.copy().into_zval());
    }
    callable.fci.params = Box::into_raw_with_allocator(params.into_boxed_slice())
        .0
        .cast();
    let total = u32::try_from(count + lead).unwrap_or(u32::MAX);
    callable.fci.param_count = total;
}

/// `ZVAL_LONG`.
fn long(value: i64) -> zval {
    let mut zv = crate::UNDEF;
    zv.value = zend_value { lval: value };
    zv.u1.type_info = IS_LONG;
    zv
}
