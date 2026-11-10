//! `Swoole\Lock`: a process-shared mutex, rwlock or spinlock in Swoole's global
//! memory pool, over the locks crate. Like upstream, a lock outlives its object:
//! forked workers may still hold it.
use std::sync::atomic::AtomicU64;
use std::{
    cell::OnceCell,
    ffi::{CStr, c_int},
};
use swoole_rs_locks::{Lock, swr_v1_locks_size};
use swoole_rs_memory::members::pool;
use swoole_rs_phpstubs::lock;
use swoole_rs_zend::{
    Call, Custom, CustomClass, Minit,
    bailout::{self, Bailout},
    method_table,
    sys::ZEND_ACC_PUBLIC,
};

/// `swoole::Lock::Type`.
pub(crate) const RW_LOCK: i64 = 1;
pub(crate) const MUTEX: i64 = 3;
pub(crate) const SPIN_LOCK: i64 = 5;
/// `LOCK_EX` from `<sys/file.h>` (Linux and macOS).
pub(crate) const LOCK_EX: i64 = 2;

/// Bit `1 << type` for each lock type the build has: upstream's `HAVE_RWLOCK`
/// (both platforms) and `HAVE_SPINLOCK` (Linux; macOS has no `pthread_spin_lock`).
pub(crate) const KINDS: u32 = 1 << MUTEX
    | 1 << RW_LOCK
    | if cfg!(target_os = "linux") {
        1 << SPIN_LOCK
    } else {
        0
    };
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
        let message = c"must call constructor first";
        bailout::defer(swoole_rs_phpswoole::swoole_fatal_error(
            WRONG_OPERATION,
            message,
        ));
    }
    lock
}

method_table! {
    static METHODS = [
        c"__construct" => zim_swoole_lock___construct, &lock::arginfo_class_Swoole_Lock___construct, ZEND_ACC_PUBLIC;
        c"lock" => zim_swoole_lock_lock, &lock::arginfo_class_Swoole_Lock_lock, ZEND_ACC_PUBLIC;
        c"unlock" => zim_swoole_lock_unlock, &lock::arginfo_class_Swoole_Lock_unlock, ZEND_ACC_PUBLIC;
    ];
}

/// The lock types the build has, under their class and global constant names.
fn types() -> impl Iterator<Item = (&'static CStr, &'static CStr, i64)> {
    [
        (c"MUTEX", c"SWOOLE_MUTEX", MUTEX),
        (c"RWLOCK", c"SWOOLE_RWLOCK", RW_LOCK),
        (c"SPINLOCK", c"SWOOLE_SPINLOCK", SPIN_LOCK),
    ]
    .into_iter()
    .filter(|(_, _, kind)| KINDS & (1 << kind) != 0)
}

swoole_rs_zend::minit!(lock_minit => register);

/// `php_swoole_lock_minit`: register the class and its constants.
#[expect(
    clippy::unnecessary_wraps,
    reason = "minit! raises the bailout a MINIT body returns"
)]
fn register(minit: &Minit) -> Result<(), Bailout> {
    minit.register_object::<Held>(c"Swoole\\Lock", &METHODS.0, 0);
    for (name, _, kind) in types() {
        minit.class_constant_long(&LOCK, name, kind);
    }
    minit.property_long(&LOCK, c"errCode", 0);
    for (_, name, kind) in types() {
        minit.long_constant(name, kind);
    }
    Ok(())
}

/// A shared lock of `kind` in the pool, as `new Mutex(true)` (and its `RWLock`
/// and `SpinLock` twins) makes one. Upstream throws a C++ exception, which
/// terminates the process, when the pool or the lock's init fails.
fn new_lock(kind: i32) -> Lock {
    let words = swr_v1_locks_size(kind).div_ceil(size_of::<AtomicU64>());
    pool::words(words)
        .and_then(|storage| Lock::shared(kind, storage))
        .unwrap_or_else(|| std::process::abort())
}

/// `__construct(int $type = SWOOLE_MUTEX)`.
fn construct(call: &Call) {
    let Some(held) = call.custom::<Held>() else {
        return;
    };
    if held.0.get().is_some() {
        call.constructed_twice();
        call.set_bool(false);
        return;
    }
    let Ok(kind) = call.arity(0, 1).and_then(|()| call.long(1, MUTEX)) else {
        call.set_bool(false);
        return;
    };
    let supported = (0..32).contains(&kind) && KINDS & (1 << kind) != 0;
    let Some(kind) = i32::try_from(kind).ok().filter(|_| supported) else {
        // Upstream passes the format unformatted, with the type as the code.
        swoole_rs_phpswoole::exception().throw(c"lock type[%d] is not support", kind);
        call.set_bool(false);
        return;
    };
    held.0.get_or_init(|| new_lock(kind));
    call.set_bool(true);
}

/// `SW_LOCK_CHECK_RETURN`: true on success, else false with `$this->errCode`.
pub(crate) fn check(call: &Call, result: i32) {
    if result != 0 {
        call.update_property_long(None, c"errCode", result.into());
    }
    call.set_bool(result == 0);
}

/// `swoole::sec2msec(double)`, narrowed to the `int` `Lock::lock` takes.
#[expect(
    clippy::cast_possible_truncation,
    reason = "upstream casts to long, then passes an int"
)]
pub(crate) fn milliseconds(seconds: f64) -> i32 {
    (seconds * 1000.0) as i64 as i32
}

/// `lock(int $operation = LOCK_EX, float $timeout = -1): bool` of `lock`.
pub(crate) fn lock_with(call: &Call, lock: impl FnOnce(&Call) -> Option<&Lock>) {
    let parsed = call
        .arity(0, 2)
        .and_then(|()| Ok((call.long(1, LOCK_EX)?, call.double(2, -1.0)?)));
    let Ok((operation, timeout)) = parsed else {
        call.set_bool(false);
        return;
    };
    if let Some(lock) = lock(call) {
        #[expect(clippy::cast_possible_truncation, reason = "upstream passes an int")]
        check(
            call,
            lock.lock(operation as i32, milliseconds(timeout), CAPABILITIES),
        );
    }
}

fn lock(call: &Call) {
    lock_with(call, constructed);
}

/// `unlock(): bool`.
fn unlock(call: &Call) {
    if let Some(lock) = constructed(call) {
        check(call, lock.unlock());
    }
}

swoole_rs_zend::handlers! {
    zim_swoole_lock___construct => construct;
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
