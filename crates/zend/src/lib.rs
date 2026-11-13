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
pub mod observer;
pub mod opcode;
pub mod pdo;
pub mod registration;
pub mod string;
pub mod value;
mod zval;

pub use access::{key_ptr, set_key};
pub use call::{Call, Invalid, kind};
pub use callable::{Callable, Callback, Holder, Parsed};
pub use class::{Class, ClassEntry, lookup_class, unset_property_deny};
pub use descriptor::{
    FatalError, contain, fatal_error, last_fatal_error, memory_usage, pause_shutdown,
    restore_shutdown, with_unlimited_stack,
};
pub use error::{
    E_ERROR, E_WARNING, Message, report, throw_error, throw_trapped, type_error, type_name,
    zend_error,
};
pub use function::{Function, create_object_deny};
pub use helpers::{
    EMPTY_FCC, EMPTY_FCI, Record, is_main_thread, long_zval, module_loaded, numeric, object_zval,
    time, zeroed,
};
pub use local::{EnvLock, Php};
pub use minit::{Minit, Mshutdown, Parent};
pub use object::{
    Custom, CustomClass, Object, Props, Weak, custom as custom_of, init_object, new_object,
    set_object,
};
pub use swoole_rs_zend_sys as sys;
pub use sys::add_next_index_long;
pub use value::push_str as push_string;
pub use zval::{
    Array, ArrayMut, Entries, HeldArray, Interned, Key, Owned, OwnedArray, Separated, UNDEF, Value,
    ZStr, ZString,
};

/// Export Zend handlers under Zend's own names (`zim_<class>_<method>` for
/// methods, `zif_<function>` for functions) so upstream's tables name them
/// unchanged; each runs `body(&Call)`, then raises a bailout it deferred
/// ([`bailout::finish`]).
#[macro_export]
macro_rules! handlers {
    // Hot handlers whose bodies raise (`bailout::finish`) any bailout they defer.
    (leaf: $($name:ident => $body:path;)*) => {
        $crate::handlers!(@then {} $($name => $body;)*);
    };
    ($($name:ident => $body:path;)*) => {
        $crate::handlers!(@then { $crate::bailout::finish(); } $($name => $body;)*);
    };
    // Each handler, ending with `$then`.
    (@then $then:block $($name:ident => $body:path;)*) => {$(
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
            $then
        }
    )*};
}
