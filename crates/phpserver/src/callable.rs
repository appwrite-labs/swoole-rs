//! A Server or Port callback: a callable `sw_callable_create` made, which
//! `sw_callable_free` releases when it drops.
use std::{ffi::c_void, ptr::NonNull};
use swoole_rs_zend::Value;

/// One callable, owned: a slot holds it until it drops. `repr(transparent)`
/// over a non-null pointer, so `Option<Callable>` is laid out as the `zend::Function *`
/// C++ reads from a slot.
#[repr(transparent)]
pub(crate) struct Callable(NonNull<c_void>);

impl Callable {
    /// `sw_callable_create(value)`, or `None` after its warning (a bailout the
    /// warning's handler raised stays deferred).
    pub(crate) fn create(value: &Value) -> Option<Self> {
        // SAFETY: a live zval, on the PHP thread that holds it.
        let callable = unsafe { swoole_rs_phpruntime::callable_create(value.as_ptr()) };
        NonNull::new(callable).map(Self)
    }

    /// The handle `crate::dispatch::invoke` and C++ call.
    pub(crate) fn as_raw(&self) -> *mut c_void {
        self.0.as_ptr()
    }
}

impl Drop for Callable {
    /// `sw_callable_free`: the callable goes even when a destructor leaves a
    /// bailout pending, which the dropping code stops on.
    fn drop(&mut self) {
        // SAFETY: the callable `create` made, which only this drop frees.
        unsafe { swoole_rs_phpruntime::callable_free(self.0.as_ptr()) };
    }
}
