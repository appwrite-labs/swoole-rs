//! PHP-thread state: per thread on a thread-safe PHP, one process value
//! otherwise, as Swoole's `SW_THREAD_LOCAL`. A non-thread-safe build pays no TLS
//! lookup per access, and thread-context coroutines, each on its own OS thread,
//! reach the same value as the loop's thread. Tests unwind and run in parallel,
//! so they keep it per thread.

/// `thread_local!` on ZTS and in tests; a [`Process`] static otherwise. Both
/// offer the `LocalKey` methods callers use on `Cell` and `RefCell`. The choice
/// is this crate's `cfg(zts)`, so callers need no build script of their own.
#[cfg(any(zts, panic = "unwind"))]
#[macro_export]
macro_rules! php_local {
    ($($(#[$attr:meta])* static $name:ident: $ty:ty = $init:expr;)+) => {$(
        ::std::thread_local! { $(#[$attr])* static $name: $ty = const { $init }; }
    )+};
}

/// See the other `php_local!`.
#[cfg(not(any(zts, panic = "unwind")))]
#[macro_export]
macro_rules! php_local {
    ($($(#[$attr:meta])* static $name:ident: $ty:ty = $init:expr;)+) => {$(
        $(#[$attr])*
        static $name: $crate::local::Process<$ty> = $crate::local::Process($init);
    )+};
}

/// A [`php_local!`] value on a non-thread-safe PHP.
#[cfg(not(any(zts, panic = "unwind")))]
#[doc(hidden)]
pub struct Process<T>(pub T);

// SAFETY: built only for a non-thread-safe PHP, where one thread at a time runs
// PHP: the request thread, or under thread contexts the coroutine thread holding
// the scheduler's baton, whose hand-over orders each access after the last.
#[cfg(not(any(zts, panic = "unwind")))]
unsafe impl<T> Sync for Process<T> {}

#[cfg(not(any(zts, panic = "unwind")))]
impl<T> Process<std::cell::RefCell<T>> {
    pub fn with_borrow<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        f(&self.0.borrow())
    }
    pub fn with_borrow_mut<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        f(&mut self.0.borrow_mut())
    }
}

#[cfg(not(any(zts, panic = "unwind")))]
impl<T: Copy> Process<std::cell::Cell<T>> {
    pub fn get(&self) -> T {
        self.0.get()
    }
    pub fn set(&self, value: T) {
        self.0.set(value);
    }
    pub fn replace(&self, value: T) -> T {
        self.0.replace(value)
    }
}
