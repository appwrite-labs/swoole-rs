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

mod allocator;
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
mod offset;
pub mod opcode;
pub mod pdo;
pub mod registration;
pub mod string;
pub mod thread;
pub mod value;
mod zval;

pub use allocator::Emalloc;
pub use call::{Call, CallFrame, Entered, Invalid, ReturnSlot, Returned};
pub use callable::{Callable, Parsed};
pub use class::{Class, ClassEntry, Declared, deny_unset, lookup_class};
pub use descriptor::{
    FatalError, StreamContext, contain, contain_with, fatal_error, last_fatal_error, memory_usage,
    pause_shutdown, restore_shutdown, with_unlimited_stack,
};
pub use error::{
    E_ERROR, E_WARNING, Message, report, throw_error, throw_trapped, type_error, type_name,
    value_error, zend_error,
};
pub use function::{Fcc, Function};
pub use helpers::{
    EMPTY_FCC, EMPTY_FCI, Record, is_main_thread, long_zval, mkdir_recursive, module_loaded,
    numeric, time, zeroed,
};
pub use local::{EnvLock, Fatal, Php};
pub use minit::{Lifecycle, Minfo, Minit, Mshutdown, Parent, Request, Shutdown, Startup};
pub use object::{Custom, CustomClass, Object, ObjectRef, Props, Weak, new_object};
pub use swoole_rs_zend_sys as sys;
pub use sys::add_next_index_long;
pub use zval::{
    Array, ArrayMut, Entries, HeldArray, Insertions, Interned, Kept, Key, Owned, OwnedArray,
    Separated, UNDEF, Value, ZStr, ZString, Zvals,
};

/// Export Zend handlers under Zend's own names (`zim_<class>_<method>` for
/// methods, `zif_<function>` for functions) so upstream's tables name them
/// unchanged; each runs `body(&mut Call)` (or `body(&Call)`), then raises a
/// bailout it deferred
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
        #[unsafe(no_mangle)]
        pub extern "C" fn $name(frame: $crate::Entered<'_>, value: $crate::Returned<'_>) {
            $crate::Call::run(frame, value, |call| $body(call));
            $then
        }
    )*};
}
