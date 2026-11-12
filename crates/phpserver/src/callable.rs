//! A Server or Port callback: zend's owned callable, shared. A slot holds one
//! reference and each call holds its own, so PHP code that replaces the slot
//! while the callback runs (`on()` from inside it, or a destructor) drops only
//! the slot's.
use std::rc::Rc;
use swoole_rs_zend::{Value, host::callable};

/// One callable, as a slot or a call holds it.
pub(crate) type Callable = Rc<callable::Callable>;

/// `sw_callable_create(value)`, or `None` after its warning (a bailout the
/// warning's handler raised stays deferred).
pub(crate) fn create(value: &Value) -> Option<Callable> {
    callable::Callable::new(value).ok().flatten()
}
