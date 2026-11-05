//! Thin safe layer over `swoole-rs-zend-sys` for PHP classes written in Rust:
//! call frames (arguments, `$this`, return values), objects that embed their
//! `zend_object`, class registration and errors. It grows as callers need it.
//! See docs/zend.md.
//!
//! Without the `php` feature the crate is empty.
#![cfg(feature = "php")]
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

mod access;
pub mod arginfo;
pub mod bailout;
mod call;
mod callable;
mod class;
mod error;
mod frame;
mod function;
mod helpers;
mod object;
pub mod registration;
pub mod string;
pub mod value;

pub use access::{declare_null, push_string, set_key, string_of, to_long, value_of};
pub use call::{Call, Invalid, type_of};
pub use callable::Callable;
pub use class::{
    Class, class_name, declare_bool, declare_long, declare_string, register_double_constant,
    register_long_constant, unset_property_deny,
};
pub use error::{
    E_ERROR, E_WARNING, Message, php_error, throw_constructed_twice, throw_error, throw_exception,
    zend_error,
};
pub use function::create_object_deny;
pub use helpers::{
    add_assoc_long, add_next_index_long, declare_property_null, each_value, long_zval, object_of,
    update_property, update_property_long,
};
pub use object::{Object, init_object, set_object};
pub use swoole_rs_zend_sys as sys;

/// Export Zend handlers under Zend's own names (`zim_<class>_<method>` for
/// methods, `zif_<function>` for functions) so upstream's tables name them
/// unchanged; each runs `body(&Call)`, then raises a bailout it deferred
/// ([`bailout::finish`]).
#[macro_export]
macro_rules! handlers {
    ($($name:ident => $body:path;)*) => {$(
        /// A Zend handler.
        /// # Safety
        /// Called by Zend with a live frame and return slot.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(
            frame: *mut $crate::sys::zend_execute_data,
            value: *mut $crate::sys::zval,
        ) {
            // SAFETY: per Zend's handler contract.
            $body(&unsafe { $crate::Call::new(frame, value) });
            $crate::bailout::finish();
        }
    )*};
}
