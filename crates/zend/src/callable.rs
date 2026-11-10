//! Callables kept past the call that passed them, with their bound arguments:
//! Swoole's `zend::Function`, as timer and event callbacks hold them.
use crate::{
    Call, Invalid,
    bailout::{self, Bailout},
    sys::{
        IS_LONG, swr_zend_callable, swr_zend_callable_hold, swr_zend_callable_release,
        swr_zend_callable_release_quiet, swr_zend_efree, swr_zend_emalloc, swr_zval_try_addref,
        zend_fcall_info_cache, zend_value, zval,
    },
};
use std::{ffi::c_void, ptr};

/// An emalloc'd `swr_zend_callable`, laid out as Swoole's `zend::Function`, so
/// C++ can call it as one.
#[derive(Debug)]
pub struct Callable(*mut swr_zend_callable);

impl Callable {
    /// `Z_PARAM_FUNC` of argument `n`, then `Z_PARAM_VARIADIC('*')` of the
    /// arguments after it, which stay in the frame until [`Callable::keep`].
    /// # Errors
    /// Argument `n` is not callable; Zend has raised its parameter error.
    pub fn parse(call: &Call, n: u32) -> Result<Self, Invalid> {
        // SAFETY: Zend's allocator, for one callable (`ecalloc`).
        let raw =
            unsafe { swr_zend_emalloc(size_of::<swr_zend_callable>()) }.cast::<swr_zend_callable>();
        // SAFETY: fresh storage; all-zero is an empty callable.
        unsafe { raw.write_bytes(0, 1) };
        let this = Self(raw);
        // SAFETY: as above.
        let callable = unsafe { &mut *raw };
        // SAFETY: argument `n` is passed (the caller checked the arity).
        let parsed = unsafe {
            bailout::parse_func(call.arg(n), n, &raw mut callable.fci, &raw mut callable.fcc)
        };
        if let Err(invalid) = parsed {
            this.discard();
            return Err(invalid);
        }
        callable.fci.param_count = call.num_args().saturating_sub(n);
        callable.fci.params = if callable.fci.param_count > 0 {
            call.arg(n + 1)
        } else {
            ptr::null_mut()
        };
        Ok(this)
    }

    /// Keep the callable past this call: copy its arguments out of the frame
    /// (`sw_zend_fci_params_persist`), with `first` before them when given (a
    /// tick's timer id), and hold its object and closure
    /// (`sw_zend_fci_cache_persist`).
    pub fn keep(&mut self, first: Option<i64>) {
        // SAFETY: a parsed callable.
        let callable = unsafe { &mut *self.0 };
        let count = callable.fci.param_count as usize;
        let lead = usize::from(first.is_some());
        if count + lead > 0 {
            // SAFETY: Zend's allocator, for the arguments.
            let params =
                unsafe { swr_zend_emalloc((count + lead) * size_of::<zval>()) }.cast::<zval>();
            for index in 0..count {
                // `index` is below the frame's argument count.
                let source = callable.fci.params.wrapping_add(index);
                // SAFETY: a live argument; `ZVAL_COPY` takes a reference.
                unsafe { swr_zval_try_addref(source) };
                // SAFETY: as above.
                let value = unsafe { source.read() };
                // SAFETY: `params` has room for `count + lead` zvals.
                unsafe { params.wrapping_add(index + lead).write(value) };
            }
            if let Some(value) = first {
                // SAFETY: as above; `ZVAL_LONG(params, value)`.
                unsafe { params.write(long(value)) };
            }
            callable.fci.params = params;
            #[expect(
                clippy::cast_possible_truncation,
                reason = "a frame's argument count plus one"
            )]
            let total = (count + lead) as u32;
            callable.fci.param_count = total;
        }
        // SAFETY: a parsed callable.
        unsafe { swr_zend_callable_hold(self.0) };
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

    /// The kept callable's cache and bound arguments, for a call.
    #[must_use]
    pub fn parts(&self) -> (*mut zend_fcall_info_cache, u32, *mut zval) {
        // SAFETY: a parsed callable; the borrow ends here, before PHP runs.
        let callable = unsafe { &mut *self.0 };
        (
            &raw mut callable.fcc,
            callable.fci.param_count,
            callable.fci.params,
        )
    }

    /// The callback and destructor of a C owner, such as a timer, whose data is
    /// this callable's [`Callable::as_raw`]: each runs `H`'s with the callable.
    #[must_use]
    pub fn callbacks<H: Holder>() -> (Callback, Callback) {
        (run::<H>, free::<H>)
    }

    /// The callable `into_raw` returned.
    /// # Safety
    /// `raw` came from `into_raw` and is not used again.
    #[must_use]
    pub unsafe fn from_raw(raw: *mut c_void) -> Self {
        Self(raw.cast())
    }

    /// Free a callable that was never kept.
    pub fn discard(self) {
        // SAFETY: emalloc'd by `parse`; nothing was taken from the frame.
        unsafe { swr_zend_efree(self.0.cast()) };
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
        // SAFETY: emalloc'd by `parse`.
        unsafe { swr_zend_efree(self.0.cast()) };
        released
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
    // SAFETY: all-zero bytes are a valid zval (IS_UNDEF).
    let mut zv: zval = unsafe { std::mem::zeroed() };
    zv.value = zend_value { lval: value };
    zv.u1.type_info = IS_LONG;
    zv
}
