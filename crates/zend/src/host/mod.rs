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
    Create, NO_THEN, Then, exception, function_call, named_call, named_call_into,
    uncaught_exception,
};
