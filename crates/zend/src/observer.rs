//! Function-call observers (`zend_observer_fcall_register`): the three entries
//! Zend calls hand an [`Observer`] the function the observed frame calls.
use crate::{
    Function, Minit,
    opcode::Executing,
    sys::{zend_execute_data, zend_observer_fcall_handlers, zend_observer_fcall_register, zval},
};
use std::marker::PhantomData;

/// An observer of function calls.
pub trait Observer {
    /// At a function's first call: whether its calls are observed from then on.
    fn observes(function: Function<'_>) -> bool;
    /// Before an observed function's call runs.
    fn begin(function: Function<'_>);
    /// After it ran, however it ended.
    fn end(function: Function<'_>);
}

/// A frame Zend observes, live for `'a`. Only the entries below make one.
struct Observed<'a>(*mut zend_execute_data, PhantomData<&'a zend_execute_data>);

impl<'a> Observed<'a> {
    /// The function the frame calls, which Zend keeps for the call; None for
    /// a frame without one.
    fn function(&self) -> Option<Function<'a>> {
        // Per Zend's observer contract, a live frame executing on this thread.
        let function = Executing(self.0, PhantomData).read(|frame| frame.func);
        (!function.is_null()).then_some(Function(function, PhantomData))
    }
}

/// # Safety
/// Zend's `zend_observer_fcall_begin_handler` contract: a live frame.
unsafe extern "C" fn begin<O: Observer>(frame: *mut zend_execute_data) {
    if let Some(function) = Observed(frame, PhantomData).function() {
        O::begin(function);
    }
}

/// # Safety
/// Zend's `zend_observer_fcall_end_handler` contract: a live frame.
unsafe extern "C" fn end<O: Observer>(frame: *mut zend_execute_data, _: *mut zval) {
    if let Some(function) = Observed(frame, PhantomData).function() {
        O::end(function);
    }
}

/// # Safety
/// Zend's `zend_observer_fcall_init` contract: a live frame.
unsafe extern "C" fn init<O: Observer>(
    frame: *mut zend_execute_data,
) -> zend_observer_fcall_handlers {
    match Observed(frame, PhantomData).function() {
        Some(function) if O::observes(function) => zend_observer_fcall_handlers {
            begin: Some(begin::<O>),
            end: Some(end::<O>),
        },
        _ => zend_observer_fcall_handlers {
            begin: None,
            end: None,
        },
    }
}

/// Observe function calls with `O` from now on. MINIT (the token) is when Zend
/// takes observers.
pub fn register<O: Observer>(_minit: &Minit) {
    // SAFETY: MINIT, on its thread; the entries live for the process.
    unsafe { zend_observer_fcall_register(Some(init::<O>)) };
}
