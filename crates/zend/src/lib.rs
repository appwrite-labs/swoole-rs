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
mod descriptor;
mod error;
mod frame;
mod function;
mod helpers;
pub mod host;
pub mod local;
mod minit;
mod object;
pub mod registration;
pub mod string;
pub mod value;
mod zval;

pub use access::{key_ptr, set_key, string_of};
pub use call::{Call, Invalid, kind, type_of};
pub use callable::{Callable, Callback, Holder, Parsed};
pub use class::{Class, class_name, unset_property_deny};
pub use descriptor::{contain, last_fatal_error, pause_shutdown, restore_shutdown};
pub use error::{E_ERROR, E_WARNING, Message, report, throw_exception, zend_error};
pub use function::create_object_deny;
pub use helpers::{
    EMPTY_FCC, EMPTY_FCI, each_value, long_zval, module_loaded, property, time, update_property,
};
pub use local::Php;
pub use minit::{Minit, Parent};
pub use object::{Custom, CustomClass, Object, Props, init_object, new_object, set_object};
pub use swoole_rs_zend_sys as sys;
pub use sys::add_next_index_long;
pub use value::{get_long as to_long, obj_of as object_of, push_str as push_string};
pub use zval::{Array, ArrayMut, Entries, Key, Owned, OwnedArray, UNDEF, Value, ZStr, ZString};

/// Export Zend handlers under Zend's own names (`zim_<class>_<method>` for
/// methods, `zif_<function>` for functions) so upstream's tables name them
/// unchanged; each runs `body(&Call)`, then raises a bailout it deferred
/// ([`bailout::finish`]).
#[macro_export]
macro_rules! handlers {
    // Hot handlers whose bodies raise (`bailout::finish`) any bailout they defer.
    (leaf: $($name:ident => $body:path;)*) => {$(
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
        }
    )*};
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
