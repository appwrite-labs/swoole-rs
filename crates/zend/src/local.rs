//! PHP-thread state ([`php_local!`]): per thread on a thread-safe PHP, one process
//! value otherwise, as Swoole's `SW_THREAD_LOCAL`. A non-thread-safe build pays no
//! TLS lookup per access, and thread-context coroutines, each on its own OS thread,
//! reach the same value as the loop's thread. Tests unwind and run in parallel, so
//! they keep it per thread.
//!
//! Every access takes a [`Php`]: a lease on running PHP for the token's lifetime,
//! which a non-thread-safe PHP grants one thread at a time. A thread outside such a
//! scope (a Rust thread, or a PHP thread once the call that leased it returned)
//! holds none, so it can't reach a process value at the same time.
use crate::sys;
use std::{ffi::c_int, marker::PhantomData};

// What a [`Php`] reads of its thread's executor: each takes no argument, reads
// one value and runs no PHP code. Private, so only a lease's methods call them.
unsafe extern "C" {
    safe fn swr_zend_facade_function_table() -> *mut sys::HashTable;
    safe fn swr_zend_stream_vm_active() -> bool;
    safe fn swr_zend_stream_default_timeout() -> sys::zend_long;
    safe fn swr_zend_http_max_input_vars() -> sys::zend_long;
    safe fn swr_zend_error_reporting() -> c_int;
    safe fn swr_zend_vm_interrupt() -> *mut sys::zend_atomic_bool;
    /// `CG(function_table)`'s function `name`, or null.
    safe fn swr_zend_compiled_function_find(
        name: crate::helpers::CText<'_>,
    ) -> *mut sys::zend_function;
    safe fn swr_zend_objects_top() -> u32;
    safe fn swr_zend_resource_count() -> u32;
    /// Whether the executor's object store holds a live object with class-name
    /// handlers at `handle`, any `u32`; `out`, if given, then holds it with a
    /// reference of its own, in place of what it held, which is not released
    /// (false, with `out` untouched, once the object's free has begun).
    safe fn swr_zend_objects_copy(handle: u32, out: Option<&crate::Owned>) -> bool;
}

/// This thread runs PHP for `'a`: it executes only while it is the one thread
/// running PHP (a thread-context coroutine that yields is parked until it holds the
/// baton again). Neither `Send` nor `Sync`, so it stays on its thread.
#[derive(Clone, Copy)]
pub struct Php<'a>(pub(crate) PhantomData<&'a *const ()>);

impl Php<'_> {
    /// # Safety
    /// For all of `'a`, this thread runs PHP: it executes only while no other
    /// thread runs PHP, as within a Zend call or a callback of the loop PHP drives.
    #[must_use]
    pub unsafe fn assume<'a>() -> Php<'a> {
        Php(PhantomData)
    }
}

impl<'a> Php<'a> {
    /// `EG(function_table)`: the functions the running request can call.
    #[must_use]
    pub fn function_table(self) -> &'a crate::Array {
        // SAFETY: this thread runs PHP, whose executor has its function table,
        // which nothing frees while PHP runs.
        unsafe { &*swr_zend_facade_function_table().cast() }
    }

    /// `EG(active)`: whether the executor is running, so a coroutine can wait.
    #[must_use]
    pub fn vm_active(self) -> bool {
        swr_zend_stream_vm_active()
    }

    /// `FG(default_socket_timeout)`: the `default_socket_timeout` ini value, in seconds.
    #[must_use]
    pub fn default_socket_timeout(self) -> sys::zend_long {
        swr_zend_stream_default_timeout()
    }

    /// `PG(max_input_vars)`: the `max_input_vars` ini value.
    #[must_use]
    pub fn max_input_vars(self) -> sys::zend_long {
        swr_zend_http_max_input_vars()
    }

    /// `EG(error_reporting)`: the level `@` saves and restores.
    #[must_use]
    pub fn error_reporting(self) -> c_int {
        swr_zend_error_reporting()
    }

    /// `&EG(vm_interrupt)`: this thread's executor's interrupt flag, which lives
    /// as long as its executor does. Only the address is taken.
    #[must_use]
    pub fn vm_interrupt(self) -> *mut sys::zend_atomic_bool {
        swr_zend_vm_interrupt()
    }

    /// `zend::get_function(CG(function_table), name)`: the function this
    /// thread's compiler knows as `name` (lower case), or null.
    #[must_use]
    pub fn compiled_function(self, name: &std::ffi::CStr) -> *mut sys::zend_function {
        swr_zend_compiled_function_find(name.into())
    }

    /// `EG(objects_store).top`: one past the highest object handle in use.
    #[must_use]
    pub fn objects_top(self) -> u32 {
        swr_zend_objects_top()
    }

    /// `zend_array_count(&EG(regular_list))`: the request's open resources.
    #[must_use]
    pub fn resource_count(self) -> u32 {
        swr_zend_resource_count()
    }

    /// Whether the executor's object store holds a live object at `handle`
    /// (Swoole's `object_valid`).
    #[must_use]
    pub fn object_live(self, handle: u32) -> bool {
        swr_zend_objects_copy(handle, None)
    }

    /// The live object the executor's store holds at `handle`, with a
    /// reference of its own; None for no such object, or one being freed.
    #[must_use]
    pub fn object_at(self, handle: u32) -> Option<crate::Owned> {
        let found = crate::Owned::default();
        swr_zend_objects_copy(handle, Some(&found)).then_some(found)
    }

    /// The handler of the internal function the running request calls `name`
    /// (lower case, as `EG(function_table)` keys it); None for no such function
    /// or one PHP code defines, which has no handler.
    #[must_use]
    pub fn internal_handler(self, name: &[u8]) -> Option<sys::zif_handler> {
        let function = self.function_table().find(name)?.pointer()?;
        // The executor's function table holds pointers to live functions only,
        // which it keeps while this reads one.
        crate::Function(function.cast(), PhantomData).internal_handler()
    }

    /// `tsrm_env_lock()`, held until the guard drops (`tsrm_env_unlock()`):
    /// nothing on a PHP without thread safety. The lock is not recursive: a
    /// second guard on the same thread waits for the first forever.
    #[must_use]
    pub fn env_lock(self) -> EnvLock<'a> {
        // SAFETY: this thread runs PHP, so TSRM has made its environment mutex;
        // the guard unlocks it once, on this thread.
        unsafe { sys::swr_zend_runtime_env_lock(true) };
        EnvLock(PhantomData)
    }
}

/// PHP's environment lock ([`Php::env_lock`]), released on drop. Neither `Send`
/// nor `Sync`: the thread that locked it unlocks it.
pub struct EnvLock<'a>(PhantomData<&'a *const ()>);

impl Drop for EnvLock<'_> {
    fn drop(&mut self) {
        // SAFETY: `env_lock` locked the mutex on this thread, and only this
        // drop unlocks it.
        unsafe { sys::swr_zend_runtime_env_lock(false) };
    }
}

/// `thread_local!` on ZTS and in tests, a process static otherwise, either behind a
/// [`PhpLocal`]. The choice is this crate's `cfg(zts)`, so callers need no build
/// script of their own.
#[cfg(any(zts, panic = "unwind"))]
#[macro_export]
macro_rules! php_local {
    ($($(#[$attr:meta])* static $name:ident: $ty:ty = $init:expr;)+) => {$(
        $(#[$attr])*
        static $name: $crate::local::PhpLocal<$ty> = {
            ::std::thread_local! { static KEY: $ty = const { $init }; }
            $crate::local::PhpLocal::new(&KEY)
        };
    )+};
}

/// See the other `php_local!`.
#[cfg(not(any(zts, panic = "unwind")))]
#[macro_export]
macro_rules! php_local {
    ($($(#[$attr:meta])* static $name:ident: $ty:ty = $init:expr;)+) => {$(
        $(#[$attr])*
        static $name: $crate::local::PhpLocal<$ty> = $crate::local::PhpLocal::new($init);
    )+};
}

/// A [`php_local!`] value, reached only with a [`Php`].
#[cfg(any(zts, panic = "unwind"))]
pub struct PhpLocal<T: 'static>(&'static std::thread::LocalKey<T>);

/// A [`php_local!`] value, reached only with a [`Php`].
#[cfg(not(any(zts, panic = "unwind")))]
pub struct PhpLocal<T>(T);

// SAFETY: built only for a non-thread-safe PHP, which runs PHP on one thread at a
// time (the request thread, or under thread contexts the coroutine thread holding
// the scheduler's baton, whose hand-over orders each access after the last). The
// value is reached only through a live `Php`, which leases running PHP for its
// lifetime, can't leave its thread, and is borrowed only for the access.
#[cfg(not(any(zts, panic = "unwind")))]
unsafe impl<T> Sync for PhpLocal<T> {}

impl<T> PhpLocal<T> {
    #[cfg(any(zts, panic = "unwind"))]
    #[must_use]
    #[doc(hidden)]
    pub const fn new(key: &'static std::thread::LocalKey<T>) -> Self {
        Self(key)
    }

    #[cfg(not(any(zts, panic = "unwind")))]
    #[must_use]
    #[doc(hidden)]
    pub const fn new(value: T) -> Self {
        Self(value)
    }

    fn with<R>(&'static self, _php: Php<'_>, f: impl FnOnce(&T) -> R) -> R {
        #[cfg(any(zts, panic = "unwind"))]
        return self.0.with(f);
        #[cfg(not(any(zts, panic = "unwind")))]
        f(&self.0)
    }
}

impl<T> PhpLocal<std::cell::RefCell<T>> {
    pub fn with_borrow<R>(&'static self, php: Php<'_>, f: impl FnOnce(&T) -> R) -> R {
        self.with(php, |cell| f(&cell.borrow()))
    }

    pub fn with_borrow_mut<R>(&'static self, php: Php<'_>, f: impl FnOnce(&mut T) -> R) -> R {
        self.with(php, |cell| f(&mut cell.borrow_mut()))
    }
}

impl<T: Copy> PhpLocal<std::cell::Cell<T>> {
    #[must_use]
    pub fn get(&'static self, php: Php<'_>) -> T {
        self.with(php, std::cell::Cell::get)
    }

    pub fn set(&'static self, php: Php<'_>, value: T) {
        self.with(php, |cell| cell.set(value));
    }
}
