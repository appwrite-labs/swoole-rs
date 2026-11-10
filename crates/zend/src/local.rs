//! PHP-thread state ([`php_local!`]): per thread on a thread-safe PHP, one process
//! value otherwise, as Swoole's `SW_THREAD_LOCAL`. A non-thread-safe build pays no
//! TLS lookup per access, and thread-context coroutines, each on its own OS thread,
//! reach the same value as the loop's thread. Tests unwind and run in parallel, so
//! they keep it per thread.
//!
//! Every access takes a [`Php`]: proof that this thread runs PHP, which a
//! non-thread-safe PHP lets one thread at a time do. A Rust thread that never ran
//! PHP can't get one, so it can't reach a process value at the same time.
use std::marker::PhantomData;

/// Proof that this thread runs PHP. Neither `Send` nor `Sync`, so it stays on the
/// thread that got it.
#[derive(Clone, Copy)]
pub struct Php(PhantomData<*const ()>);

#[cfg(not(any(zts, panic = "unwind")))]
thread_local! {
    /// Whether this thread has run MINIT or a Zend call that asked for a [`Php`].
    static PHP_THREAD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

impl Php {
    /// This thread's proof once it has run PHP (MINIT, or a [`Call::php`](crate::Call::php)).
    /// Per-thread state needs none, so ZTS and tests always have one.
    #[must_use]
    pub fn running() -> Option<Self> {
        #[cfg(not(any(zts, panic = "unwind")))]
        if !PHP_THREAD.get() {
            return None;
        }
        Some(Self(PhantomData))
    }

    /// Records that this thread runs PHP; only Zend's own entry points call it.
    pub(crate) fn enter() -> Self {
        #[cfg(not(any(zts, panic = "unwind")))]
        PHP_THREAD.set(true);
        Self(PhantomData)
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
// value is reached only through a `Php`, which only such a thread can get and which
// can't leave it.
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

    fn with<R>(&'static self, _php: Php, f: impl FnOnce(&T) -> R) -> R {
        #[cfg(any(zts, panic = "unwind"))]
        return self.0.with(f);
        #[cfg(not(any(zts, panic = "unwind")))]
        f(&self.0)
    }
}

impl<T> PhpLocal<std::cell::RefCell<T>> {
    pub fn with_borrow<R>(&'static self, php: Php, f: impl FnOnce(&T) -> R) -> R {
        self.with(php, |cell| f(&cell.borrow()))
    }

    pub fn with_borrow_mut<R>(&'static self, php: Php, f: impl FnOnce(&mut T) -> R) -> R {
        self.with(php, |cell| f(&mut cell.borrow_mut()))
    }
}

impl<T: Copy> PhpLocal<std::cell::Cell<T>> {
    #[must_use]
    pub fn get(&'static self, php: Php) -> T {
        self.with(php, std::cell::Cell::get)
    }

    pub fn set(&'static self, php: Php, value: T) {
        self.with(php, |cell| cell.set(value));
    }
}
