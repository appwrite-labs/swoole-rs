//! Swoole's PHP host helpers (`php_swoole_cxx`): long-lived callables, callback
//! invocation, array properties, known strings, and the C exports
//! native/phphelpers' headers call.
pub mod callable;
mod common;
mod known_strings;
mod private;
mod utilities;

pub use common::{known_free, known_init, sw_zend_known_strings};
pub use private::{
    Callee, Create, Creator, NO_THEN, Then, Trap, exception, exception_owned, function_call,
    named_call, named_call_into, uncaught_exception,
};
pub(crate) use private::{CalleePtr, user_call};
