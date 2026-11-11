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
use std::marker::PhantomData;

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
