//! Callables kept past the call that passed them, with their bound arguments:
//! Swoole's `zend::Function`, as timer and event callbacks hold them.
use crate::{
    Call, Invalid,
    bailout::{self, Bailout},
    sys::{
        IS_LONG, swr_zend_callable, swr_zend_callable_release, swr_zend_callable_release_quiet,
        zend_fcall_info_cache, zend_value, zval,
    },
    value::emalloc,
};
use std::{ffi::c_void, mem::MaybeUninit, ptr};

// What takes one of this module's handles whole. Each handle is its record's
// pointer (`repr(transparent)`), made only here, so C receives a live record in
// the state the handle's type says; none of these runs PHP code.
unsafe extern "C" {
    /// `sw_zend_fci_cache_persist`: one more reference to the object and the
    /// closure of the function `parse` resolved. The handle is spent: `keep`
    /// makes the [`Callable`] of the same record.
    safe fn swr_zend_callable_hold(parsed: Parsed);
    /// `efree` of the record of a callable that holds nothing.
    #[link_name = "swr_zend_efree"]
    safe fn discard(parsed: Parsed);
    /// `efree` of the record of a callable whose references were released.
    #[link_name = "swr_zend_efree"]
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
#[derive(Debug)]
#[repr(transparent)]
pub struct Parsed(*mut swr_zend_callable);

impl Callable {
    /// `Z_PARAM_FUNC` of argument `n`, then `Z_PARAM_VARIADIC('*')` of the
    /// arguments after it, which stay in the frame until [`Parsed::keep`].
    /// # Errors
    /// Argument `n` is not callable; Zend has raised its parameter error.
    pub fn parse(call: &Call, n: u32) -> Result<Parsed, Invalid> {
        let (fci, fcc) = call.callable(n)?;
        // Zend's allocator, for one callable (`ecalloc`).
        let raw = emalloc(size_of::<swr_zend_callable>()).cast::<swr_zend_callable>();
        // SAFETY: fresh storage, which `emalloc` never returns null for and
        // aligns for the record: written whole before anything reads it.
        unsafe { raw.write(swr_zend_callable { fci, fcc }) };
        let mut this = Parsed(raw);
        let callable = this.record();
        callable.fci.param_count = call.num_args().saturating_sub(n);
        callable.fci.params = if callable.fci.param_count > 0 {
            call.arg(n + 1)
        } else {
            ptr::null_mut()
        };
        Ok(this)
    }

    /// The callable for C++ (`zend::Function *`).
    #[must_use]
    pub fn as_raw(&self) -> *mut c_void {
        self.0.cast()
    }

    /// The callable as timer or event data, which owns it from now on.
    #[must_use]
    pub fn into_raw(self) -> *mut c_void {
        self.0.cast()
    }

    /// The kept callable's cache, bound arguments and callable zval, for a call.
    #[must_use]
    pub fn parts(&self) -> (*mut zend_fcall_info_cache, u32, *mut zval, *mut zval) {
        // SAFETY: a parsed callable; the borrow ends here, before PHP runs.
        let callable = unsafe { &mut *self.0 };
        (
            &raw mut callable.fcc,
            callable.fci.param_count,
            callable.fci.params,
            &raw mut callable.fci.function_name,
        )
    }

    /// The callback and destructor of a C owner, such as a timer, whose data is
    /// this callable's [`Callable::as_raw`]: each runs `H`'s with the callable.
    #[must_use]
    pub fn callbacks<H: Holder>() -> (Callback, Callback) {
        (run::<H>, free::<H>)
    }

    /// Drop the arguments, object and closure `keep` held, then free the
    /// callable (`sw_zend_fci_params_discard`, `sw_zend_fci_cache_discard`).
    /// # Errors
    /// A destructor bailed out; the callable is freed anyway.
    pub fn release(self) -> Result<(), Bailout> {
        // SAFETY: a kept callable; the quiet release runs no PHP code.
        let released = if unsafe { swr_zend_callable_release_quiet(self.0) } != 0 {
            Ok(())
        } else {
            // SAFETY: the release body is C; the callable was kept.
            unsafe { bailout::trap(swr_zend_callable_release, self.0.cast()) }
        };
        // Emalloc'd by `parse`, and released above.
        free_released(self);
        released
    }
}

impl Parsed {
    /// The callable's record, which only this owner reaches until it is kept.
    fn record(&mut self) -> &mut swr_zend_callable {
        // SAFETY: `parse`'s live allocation, written before this is made; the
        // exclusive borrow is the only way to it.
        unsafe { &mut *self.0 }
    }

    /// Keep the callable past this call: copy its arguments out of the frame
    /// (`sw_zend_fci_params_persist`), with `first` before them when given (a
    /// tick's timer id), and hold its object and closure
    /// (`sw_zend_fci_cache_persist`).
    #[must_use]
    pub fn keep(mut self, first: Option<i64>) -> Callable {
        let callable = self.record();
        let count = callable.fci.param_count as usize;
        let lead = usize::from(first.is_some());
        if count + lead > 0 {
            // Zend's allocator, for the arguments.
            let params = emalloc((count + lead) * size_of::<zval>()).cast::<zval>();
            // SAFETY: the allocation above, which `emalloc` never returns null
            // for and aligns for a zval: `count + lead` unwritten zvals that
            // only this reaches until they are written below.
            let slots = unsafe {
                std::slice::from_raw_parts_mut(params.cast::<MaybeUninit<zval>>(), count + lead)
            };
            let mut slots = slots.iter_mut();
            // `ZVAL_LONG(params, value)`, in the slot `lead` counted.
            if let Some(value) = first
                && let Some(slot) = slots.next()
            {
                slot.write(long(value));
            }
            for (index, slot) in slots.enumerate() {
                // `index` is below the frame's argument count.
                let source = callable.fci.params.wrapping_add(index);
                // SAFETY: a live argument, which the frame holds for this call.
                let value = unsafe { &*source.cast::<crate::Value>() };
                // `ZVAL_COPY` takes a reference.
                slot.write(value.copy().into_zval());
            }
            callable.fci.params = params;
            #[expect(
                clippy::cast_possible_truncation,
                reason = "a frame's argument count plus one"
            )]
            let total = (count + lead) as u32;
            callable.fci.param_count = total;
        }
        let record = self.0;
        swr_zend_callable_hold(self);
        Callable(record)
    }

    /// The callable for C++ (`zend::Function *`), which is [`Parsed::keep`]'s
    /// `Callable` too.
    #[must_use]
    pub fn as_raw(&self) -> *mut c_void {
        self.0.cast()
    }

    /// Free a callable that was never kept.
    pub fn discard(self) {
        // Emalloc'd by `parse`; nothing was taken from the frame.
        discard(self);
    }
}

/// A C owner's callback over the data it was given.
pub type Callback = unsafe extern "C-unwind" fn(*mut c_void);

/// What a C owner of a callable does with it ([`Callable::callbacks`]).
pub trait Holder {
    /// The owner's callback, with its callable.
    fn run(callable: &Callable);
    /// The owner's destructor, which takes the callable back to release it.
    fn free(callable: Callable);
}

/// # Safety
/// `data` is a live callable's [`Callable::as_raw`].
unsafe extern "C-unwind" fn run<H: Holder>(data: *mut c_void) {
    H::run(&Callable(data.cast()));
}

/// # Safety
/// `data` is a callable's [`Callable::as_raw`], not used again.
unsafe extern "C-unwind" fn free<H: Holder>(data: *mut c_void) {
    H::free(Callable(data.cast()));
}

/// `ZVAL_LONG`.
fn long(value: i64) -> zval {
    let mut zv = crate::UNDEF;
    zv.value = zend_value { lval: value };
    zv.u1.type_info = IS_LONG;
    zv
}
