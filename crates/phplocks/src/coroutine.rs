//! `Swoole\\Coroutine\\Lock`: PHP object ownership over the Rust coroutine lock.
use std::{cell::OnceCell, ffi::c_int};
use swoole_rs_locks::coroutine::{CoroutineHost, CoroutineLock};
use swoole_rs_memory::members::pool;
use swoole_rs_phpstubs::coroutine_lock;
use swoole_rs_zend::{
    Call, Custom, CustomClass, Minit,
    bailout::{self, Bailout},
    method_table,
    sys::ZEND_ACC_PUBLIC,
};

/// `SW_ERROR_WRONG_OPERATION`.
const WRONG_OPERATION: c_int = 509;
static CLASS: CustomClass<Held> = CustomClass::new();

/// The lock's effects, as upstream's `CoroutineLock` reaches them: the current
/// coroutine, the global pool for a shared word, and either `System::sleep`
/// backoff or, with `io_uring` (whose engine always has futexes), the ring's futex.
static HOST: CoroutineHost = CoroutineHost {
    current: host::current,
    alloc_shared: host::alloc_shared,
    #[cfg(not(feature = "iouring"))]
    sleep: Some(host::sleep),
    #[cfg(feature = "iouring")]
    sleep: None,
    #[cfg(not(feature = "iouring"))]
    wait: None,
    #[cfg(feature = "iouring")]
    wait: Some(host::wait),
    #[cfg(not(feature = "iouring"))]
    wake: None,
    #[cfg(feature = "iouring")]
    wake: Some(host::wake),
    warning: host::warning,
};

mod host {
    pub(super) fn current() -> Option<i64> {
        swoole_rs_phpcoroutines::Coroutine::current().map(swoole_rs_phpcoroutines::Coroutine::cid)
    }
    pub(super) fn alloc_shared() -> Option<&'static std::sync::atomic::AtomicU32> {
        super::pool::words(1)?.first()
    }
    #[cfg(not(feature = "iouring"))]
    /// The lock sleeps only inside a coroutine.
    pub(super) fn sleep(seconds: f64) -> i32 {
        swoole_rs_phpcoroutines::sleep(seconds)
    }
    /// `Iouring::futex_wait`.
    #[cfg(feature = "iouring")]
    pub(super) fn wait(word: &std::sync::atomic::AtomicU32) -> i32 {
        swoole_rs_hooks::iouring::futex_wait(word)
    }
    /// `Iouring::futex_wakeup`.
    #[cfg(feature = "iouring")]
    pub(super) fn wake(word: &std::sync::atomic::AtomicU32) -> i32 {
        swoole_rs_hooks::iouring::futex_wakeup(word)
    }
    pub(super) fn warning() {
        swoole_rs_corebase::log::warning(
            c"swoole::CoroutineLock::lock_impl",
            format_args!("The coroutine lock can only be used in a coroutine environment"),
        );
    }
}

/// The object's lock, unset until `__construct`.
pub(crate) struct Held(OnceCell<Box<CoroutineLock>>);

impl Custom for Held {
    fn class() -> &'static CustomClass<Self> {
        &CLASS
    }
    fn new() -> Self {
        Self(OnceCell::new())
    }
}

/// The constructed lock. Operations take `&CoroutineLock`, so the borrow may
/// span suspension while another coroutine unlocks: Zend holds `$this`, and so
/// the lock, throughout the method.
fn constructed(call: &Call) -> Option<&CoroutineLock> {
    let lock = call.custom::<Held>()?.0.get();
    if lock.is_none() {
        let message = c"must call constructor first";
        bailout::defer(swoole_rs_phpswoole::swoole_fatal_error(
            WRONG_OPERATION,
            message,
        ));
    }
    lock.map(|lock| &**lock)
}
method_table! {
    static METHODS = [
        c"__construct" => zim_swoole_coroutine_lock___construct, &coroutine_lock::arginfo_class_Swoole_Coroutine_Lock___construct, ZEND_ACC_PUBLIC;
        c"lock" => zim_swoole_coroutine_lock_lock, &coroutine_lock::arginfo_class_Swoole_Coroutine_Lock_lock, ZEND_ACC_PUBLIC;
        c"unlock" => zim_swoole_coroutine_lock_unlock, &coroutine_lock::arginfo_class_Swoole_Coroutine_Lock_unlock, ZEND_ACC_PUBLIC;
    ];
}
/// `php_swoole_coroutine_lock_minit`.
/// # Errors
/// A registration bailed out.
pub fn coroutine_lock_minit(minit: &Minit) -> Result<(), Bailout> {
    minit.register_object::<Held>(c"Swoole\\Coroutine\\Lock", &METHODS.0, 0);
    minit.property_long(&CLASS, c"errCode", 0, ZEND_ACC_PUBLIC);
    Ok(())
}
fn construct(call: &Call) {
    let Some(held) = call.custom::<Held>() else {
        return;
    };
    if held.0.get().is_some() {
        call.throw_constructed_twice();
        call.set_bool(false);
        return;
    }
    let Ok(shared) = call.arity(0, 1).and_then(|()| call.bool(1, false)) else {
        call.set_bool(false);
        return;
    };
    held.0.get_or_init(|| CoroutineLock::new(shared, &HOST));
    call.set_bool(true);
}
fn check(call: &Call, result: Result<(), i32>) {
    super::lock::check(call, result.err().unwrap_or(0));
}
fn lock(call: &Call) {
    let Ok(operation) = call
        .arity(0, 1)
        .and_then(|()| call.long(1, super::lock::LOCK_EX))
    else {
        call.set_bool(false);
        return;
    };
    let Some(owner) = constructed(call) else {
        return;
    };
    // LOCK_NB is 4 on both supported platforms; checking the zend_long avoids truncation.
    let blocking = operation & 4 == 0;
    check(call, owner.acquire(blocking));
}
fn unlock(call: &Call) {
    let Some(owner) = constructed(call) else {
        return;
    };
    check(call, owner.unlock());
}
swoole_rs_zend::handlers! {
    zim_swoole_coroutine_lock___construct => construct;
    zim_swoole_coroutine_lock_lock => lock;
    zim_swoole_coroutine_lock_unlock => unlock;
}
