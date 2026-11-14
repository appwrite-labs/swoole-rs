//! Function-call observers (`zend_observer_fcall_register`): the three entries
//! Zend calls hand an [`Observer`] the function the observed frame calls.
use crate::{
    Function, Minit,
    opcode::Executing,
    sys::{zend_execute_data, zend_observer_fcall_handlers, zval},
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

/// A frame Zend observes, live for `'a`: the `zend_execute_data *` Zend passes
/// the entries below, whose layout it has. Nothing in Rust makes one: Zend's
/// observer contract (a live frame, executing on this thread) stands for the
/// constructor.
#[repr(transparent)]
struct Observed<'a>(*mut zend_execute_data, PhantomData<&'a zend_execute_data>);

impl<'a> Observed<'a> {
    /// The function the frame calls, which Zend keeps for the call; None for
    /// a frame without one.
    fn function(&self) -> Option<Function<'a>> {
        // A live frame executing on this thread, per the type.
        let function = Executing(self.0, PhantomData).read(|frame| frame.func);
        (!function.is_null()).then_some(Function(function, PhantomData))
    }
}

/// `zend_observer_fcall_handlers`, with the entries typed as what Zend passes them.
#[repr(C)]
struct Handlers {
    begin: Option<extern "C" fn(Observed<'_>)>,
    end: Option<extern "C" fn(Observed<'_>, *mut zval)>,
}

const _: () = assert!(
    size_of::<Handlers>() == size_of::<zend_observer_fcall_handlers>()
        && align_of::<Handlers>() == align_of::<zend_observer_fcall_handlers>(),
    "Handlers is zend_observer_fcall_handlers"
);

// Zend's own declaration, with the init entry typed as Zend calls it: with the
// frame it observes. Registering only appends `init` to Zend's list of
// observers, and runs no PHP code. Private, so only [`register`] calls it, at MINIT.
unsafe extern "C" {
    safe fn zend_observer_fcall_register(init: Option<extern "C" fn(Observed<'_>) -> Handlers>);
}

/// `zend_observer_fcall_begin_handler`.
extern "C" fn begin<O: Observer>(frame: Observed<'_>) {
    if let Some(function) = frame.function() {
        O::begin(function);
    }
}

/// `zend_observer_fcall_end_handler`.
extern "C" fn end<O: Observer>(frame: Observed<'_>, _: *mut zval) {
    if let Some(function) = frame.function() {
        O::end(function);
    }
}

/// `zend_observer_fcall_init`.
extern "C" fn init<O: Observer>(frame: Observed<'_>) -> Handlers {
    match frame.function() {
        Some(function) if O::observes(function) => Handlers {
            begin: Some(begin::<O>),
            end: Some(end::<O>),
        },
        _ => Handlers {
            begin: None,
            end: None,
        },
    }
}

/// Observe function calls with `O` from now on. MINIT (the token) is when Zend
/// takes observers.
pub fn register<O: Observer>(_minit: &Minit) {
    zend_observer_fcall_register(Some(init::<O>));
}
