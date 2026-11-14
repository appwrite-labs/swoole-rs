//! `Swoole\Lock`: a process-shared mutex, rwlock or spinlock in Swoole's global
//! memory pool, over the locks crate. Like upstream, a lock outlives its object:
//! forked workers may still hold it.
use std::{
    cell::OnceCell,
    ffi::{CStr, c_int},
};
use swoole_rs_corebase::CCast;
use swoole_rs_locks::{Kind, Lock};
use swoole_rs_memory::members::pool;
use swoole_rs_phpstubs::lock;
use swoole_rs_zend::{
    Call, Custom, CustomClass, Minit,
    bailout::{self, Bailout},
    method_table,
    sys::ZEND_ACC_PUBLIC,
};

/// `LOCK_EX` from `<sys/file.h>` (Linux and macOS).
pub(crate) const LOCK_EX: i64 = 2;

/// The timed-lock capabilities `Lock::lock` takes: `HAVE_MUTEX_TIMEDLOCK` (1)
/// and `HAVE_RWLOCK_TIMEDRDLOCK` (2), which Linux has and macOS lacks.
pub(crate) const CAPABILITIES: u32 = if cfg!(target_os = "linux") { 3 } else { 0 };
/// `SW_ERROR_WRONG_OPERATION`.
const WRONG_OPERATION: c_int = 509;
static LOCK: CustomClass<Held> = CustomClass::new();

/// `struct LockObject { Lock *lock; zend_object std; }`: unset until `__construct`.
pub(crate) struct Held(OnceCell<Lock>);

impl Custom for Held {
    fn class() -> &'static CustomClass<Self> {
        &LOCK
    }
    fn new() -> Self {
        Self(OnceCell::new())
    }
}

/// The constructed lock, or Swoole's fatal error (which exits).
fn constructed(call: &Call) -> Option<&Lock> {
    let lock = call.custom::<Held>()?.0.get();
    if lock.is_none() {
        unconstructed();
    }
    lock
}

#[cold]
fn unconstructed() {
    let message = c"must call constructor first";
    bailout::defer(swoole_rs_phpswoole::swoole_fatal_error(
        WRONG_OPERATION,
        message,
    ));
}

method_table! {
    static METHODS = [
        c"__construct" => zim_swoole_lock___construct, &lock::arginfo_class_Swoole_Lock___construct, ZEND_ACC_PUBLIC;
        c"lock" => zim_swoole_lock_lock, &lock::arginfo_class_Swoole_Lock_lock, ZEND_ACC_PUBLIC;
        c"unlock" => zim_swoole_lock_unlock, &lock::arginfo_class_Swoole_Lock_unlock, ZEND_ACC_PUBLIC;
    ];
}

/// A lock type's class constant name and its global one.
pub(crate) const fn names(kind: Kind) -> (&'static CStr, &'static CStr) {
    match kind {
        Kind::Mutex => (c"MUTEX", c"SWOOLE_MUTEX"),
        Kind::RwLock => (c"RWLOCK", c"SWOOLE_RWLOCK"),
        #[cfg(target_os = "linux")]
        Kind::Spin => (c"SPINLOCK", c"SWOOLE_SPINLOCK"),
    }
}

/// `php_swoole_lock_minit`: register the class and its constants.
/// # Errors
/// A registration bailed out.
pub fn lock_minit(minit: &Minit) -> Result<(), Bailout> {
    minit.register_object::<Held>(c"Swoole\\Lock", METHODS.rows(), 0);
    for &kind in Kind::ALL {
        minit.class_constant_long(&LOCK, names(kind).0, kind.code());
    }
    minit.property_long(&LOCK, c"errCode", 0, ZEND_ACC_PUBLIC);
    for &kind in Kind::ALL {
        minit.long_constant(names(kind).1, kind.code());
    }
    Ok(())
}

/// A shared lock of `kind` in the pool, as `new Mutex(true)` (and its `RWLock`
/// and `SpinLock` twins) makes one. Upstream throws a C++ exception, which
/// terminates the process, when the pool or the lock's init fails.
fn new_lock(kind: Kind) -> Lock {
    pool::words(Lock::words(kind))
        .and_then(|storage| Lock::shared(kind, storage))
        .unwrap_or_else(|| swoole_rs_corebase::fatal("a shared lock can't be made"))
}

/// `__construct(int $type = SWOOLE_MUTEX)`.
fn construct(call: &Call) {
    let Some(held) = call.custom::<Held>() else {
        return;
    };
    if held.0.get().is_some() {
        call.throw_constructed_twice();
        call.set_bool(false);
        return;
    }
    let Ok(code) = call
        .arity(0, 1)
        .and_then(|()| call.long(1, Kind::Mutex.code()))
    else {
        call.set_bool(false);
        return;
    };
    let Some(kind) = Kind::from_code(code) else {
        // Upstream passes the format unformatted, with the type as the code.
        swoole_rs_phpswoole::exception().throw(c"lock type[%d] is not support", code);
        call.set_bool(false);
        return;
    };
    held.0.get_or_init(|| new_lock(kind));
    call.set_bool(true);
}

/// `SW_LOCK_CHECK_RETURN`: true on success, else false with `$this->errCode`.
#[inline]
pub(crate) fn check(call: &Call, result: i32) {
    if result != 0 {
        call.update_property_long(None, c"errCode", result.into());
    }
    call.set_bool(result == 0);
}

/// [`check`] for a leaf handler: a failure's `errCode` write may leave a bailout,
/// which this raises (no frame here owns anything that needs a drop).
pub(crate) fn checked(call: &Call, result: i32) {
    if result == 0 {
        call.set_bool(true);
    } else {
        refused(call, result);
    }
}

#[cold]
fn refused(call: &Call, result: i32) {
    check(call, result);
    bailout::finish();
}

/// `ZEND_PARSE_PARAMETERS_END_EX(RETURN_FALSE)` in a leaf handler: raise a bailout
/// a user error handler left in a coercion.
#[cold]
fn failed(call: &Call) {
    call.set_bool(false);
    bailout::finish();
}

/// `swoole::sec2msec(double)`, narrowed to the `int` `Lock::lock` takes.
pub(crate) fn milliseconds(seconds: f64) -> i32 {
    (seconds * 1000.0).c_cast::<i64>().c_cast::<i32>()
}

/// `lock(int $operation = LOCK_EX, float $timeout = -1): bool` of `lock`, for
/// a leaf handler: `lock` defers the fatal error of an unconstructed object.
pub(crate) fn lock_with(call: &Call, lock: impl FnOnce(&Call) -> Option<&Lock>) {
    let parsed = call
        .arity(0, 2)
        .and_then(|()| Ok((call.long(1, LOCK_EX)?, call.double(2, -1.0)?)));
    let Ok((operation, timeout)) = parsed else {
        return failed(call);
    };
    match lock(call) {
        Some(lock) => checked(
            call,
            lock.lock(
                operation.c_cast::<i32>(),
                milliseconds(timeout),
                CAPABILITIES,
            ),
        ),
        None => bailout::finish(),
    }
}

fn lock(call: &Call) {
    lock_with(call, constructed);
}

/// `unlock(): bool`.
fn unlock(call: &Call) {
    match constructed(call) {
        Some(lock) => checked(call, lock.unlock()),
        None => bailout::finish(),
    }
}

swoole_rs_zend::handlers! {
    zim_swoole_lock___construct => construct;
}

// Each raises the bailout its failures can leave; the hot path has none.
swoole_rs_zend::handlers! {
    leaf:
    zim_swoole_lock_lock => lock;
    zim_swoole_lock_unlock => unlock;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeouts_convert_like_sec2msec() {
        assert_eq!(milliseconds(-1.0), -1000, "the default blocks");
        assert_eq!(milliseconds(0.0015), 1, "truncates toward zero");
        assert_eq!(milliseconds(2.5), 2500, "seconds to milliseconds");
    }
}
