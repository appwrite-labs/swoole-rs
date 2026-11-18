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
    FatalError, StreamContext, StreamSocket, contain, contain_with, fatal_error, last_fatal_error,
    memory_usage, pause_shutdown, restore_shutdown, with_unlimited_stack,
};
pub use error::{
    E_ERROR, E_NOTICE, E_WARNING, Message, report, throw_error, throw_trapped, type_error,
    type_name, value_error, zend_error,
};
pub use function::{Fcc, Function};
pub use helpers::{
    EMPTY_FCC, EMPTY_FCI, Record, is_main_thread, long_zval, lookup_ipv4, mkdir_recursive,
    module_loaded, numeric, time, zeroed,
};
pub use local::{EnvLock, Fatal, Php};
pub use minit::{Lifecycle, Minfo, Minit, Mshutdown, Parent, Request, Shutdown, Startup};
pub use object::{Custom, CustomClass, FreeProperties, Object, ObjectRef, Props, Weak, new_object};
pub use swoole_rs_zend_sys as sys;

/// Whether PHP is thread-safe (ZTS).
pub const ZTS: bool = cfg!(zts);
pub use sys::add_next_index_long;
pub use zval::{
    Array, ArrayMut, Entries, HeldArray, Insertions, Interned, Kept, Key, Owned, OwnedArray,
    Separated, UNDEF, Value, ZStr, ZString, Zvals,
};

/// Export Zend handlers under Zend's own names (`zim_<class>_<method>` for
/// methods, `zif_<function>` for functions) so upstream's tables name them
/// unchanged; each runs `body(&mut Call)` (or `body(&Call)`), then raises a
/// bailout it deferred
/// at its native boundary.
#[macro_export]
macro_rules! handlers {
    ($($name:ident => $body:path;)*) => {
        $crate::handlers!(@then {} $($name => $body;)*);
    };
    (@then $after:block $($name:ident => $body:path;)*) => {
        $crate::exports! {$(
            /// A Zend handler.
            #[unsafe(no_mangle)]
            pub extern "C" fn $name(frame: $crate::Entered<'_>, value: $crate::Returned<'_>) {
                $crate::Call::run(frame, value, |call| $body(call));
                $after
            }
        )*}
    };
}

/// Native bailout boundaries. Each body is an ordinary Rust function, which
/// consumes its arguments and finishes cleanup before the boundary raises a
/// deferred bailout. A return value must be `Copy`, so it needs no cleanup.
/// The generated entry point is unsafe to call from Rust: its native caller
/// must provide the bailout scope and leave no intervening Rust owners.
/// `preserve_pending:` is for destructor callbacks, which must leave an already
/// pending bailout to its original boundary.
///
/// Jump helpers are not part of the Rust API:
/// ```compile_fail
/// use swoole_rs_zend::bailout::{self, Bailout};
/// let owner = std::pin::pin!(String::from("live until dropped"));
/// Bailout.defer();
/// bailout::finish();
/// drop(owner);
/// ```
/// ```compile_fail
/// swoole_rs_zend::thread::exit(|| {});
/// ```
///
/// Generated entries cannot be invoked by safe Rust:
/// ```compile_fail
/// #![forbid(unsafe_code)]
/// swoole_rs_zend::exports! { pub extern "C" fn boundary() {} }
/// boundary();
/// ```
/// The body is checked as safe Rust even inside the unsafe entry point:
/// ```compile_fail
/// swoole_rs_zend::exports! {
///     pub extern "C" fn boundary() { let _ = core::ptr::read(&42_u32); }
/// }
/// ```
/// Nor may a boundary keep an owned result across a possible jump:
/// ```compile_fail
/// #![forbid(unsafe_code)]
/// swoole_rs_zend::exports! {
///     pub extern "C" fn boundary() -> String { String::from("owned") }
/// }
/// ```
#[macro_export]
macro_rules! exports {
    (preserve_pending: $($item:tt)*) => {
        $crate::exports!(@entry true; $($item)*);
    };
    (@entry $preserve:expr; $(
        $(#[$attr:meta])* $vis:vis extern $abi:literal fn $name:ident
        $(<$($lifetime:lifetime),+>)?
        $(<$($generic:ident: $bound:path),+>)?
        ($($arg:ident: $ty:ty),* $(,)?) $(-> $ret:ty)? $body:block
    )*) => {$(
        $(#[$attr])*
        /// # Safety
        /// Enter from a native bailout scope with no intervening Rust owners
        /// whose cleanup a jump would skip.
        $vis unsafe extern $abi fn $name $(<$($lifetime),+>)? $(<$($generic: $bound),+>)?
        ($($arg: $ty),*) $(-> $ret)? {
            fn plain<T: Copy>(value: T) -> T { value }
            fn body $(<$($lifetime),+>)? $(<$($generic: $bound),+>)?
            ($($arg: $ty),*) $(-> $ret)? $body
            let pending = $preserve && $crate::bailout::pending();
            let result = plain(body $(::<$($generic),+>)? ($($arg),*));
            if !pending && $crate::bailout::relay_to_c(Ok(())) {
                // SAFETY: the native-entry contract; the body has returned,
                // and the remaining result is Copy.
                unsafe { $crate::sys::swr_zend_guard_resume() };
            }
            result
        }
    )*};
    (@entry $($invalid:tt)*) => {
        compile_error!("invalid native bailout boundary declaration");
    };
    ($($item:tt)*) => {
        $crate::exports!(@entry false; $($item)*);
    };
}
