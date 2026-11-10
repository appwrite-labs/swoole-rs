//! Request-path state: per thread on a thread-safe PHP, one process value
//! otherwise, as Swoole's `SW_THREAD_LOCAL` (and #1916's worker globals), so a
//! non-thread-safe build pays no TLS lookup per access. Tests unwind and run in
//! parallel, so they keep it per thread.

/// `thread_local!` on ZTS and in tests; a [`Process`] static otherwise. Both
/// offer the `LocalKey` methods the crate uses on `Cell` and `RefCell`.
macro_rules! php_local {
    ($($(#[$attr:meta])* static $name:ident: $ty:ty = $init:expr;)+) => {$(
        #[cfg(any(zts, panic = "unwind"))]
        thread_local! { $(#[$attr])* static $name: $ty = const { $init }; }
        #[cfg(not(any(zts, panic = "unwind")))]
        $(#[$attr])*
        static $name: crate::local::Process<$ty> = crate::local::Process($init);
    )+};
}

#[cfg(not(any(zts, panic = "unwind")))]
pub(crate) struct Process<T>(pub(crate) T);

// SAFETY: built only for a non-thread-safe PHP, whose one PHP thread is the
// only one that runs the request path.
#[cfg(not(any(zts, panic = "unwind")))]
unsafe impl<T> Sync for Process<T> {}

#[cfg(not(any(zts, panic = "unwind")))]
impl<T> Process<std::cell::RefCell<T>> {
    pub(crate) fn with_borrow_mut<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        f(&mut self.0.borrow_mut())
    }
}

#[cfg(not(any(zts, panic = "unwind")))]
impl<T: Copy> Process<std::cell::Cell<T>> {
    pub(crate) fn get(&self) -> T {
        self.0.get()
    }
    pub(crate) fn set(&self, value: T) {
        self.0.set(value);
    }
    pub(crate) fn replace(&self, value: T) -> T {
        self.0.replace(value)
    }
}
