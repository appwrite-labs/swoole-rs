//! `Swoole\Thread\Atomic`, `Swoole\Thread\Atomic\Long` and `Swoole\Thread\Lock`
//! (ZTS builds): the word or lock lives in a reference-counted [`Resource`]
//! (upstream's `ThreadResource`), which `Thread\Map`, `ArrayList` and `Queue`
//! (crates/phpthread) share between threads through [`shared`] and
//! [`shared_object`].
use crate::{
    lock::{KINDS, MUTEX, RW_LOCK, SPIN_LOCK, checked, lock_with},
    word::{self, Holder},
};
use std::{
    cell::OnceCell,
    ffi::c_int,
    sync::{
        Arc,
        atomic::{AtomicI64, AtomicU32},
    },
};
use swoole_rs_corebase::log::set_last_error;
use swoole_rs_locks::Lock;
use swoole_rs_phpstubs::{thread_atomic, thread_lock};
use swoole_rs_zend::{
    Call, Custom, CustomClass, E_ERROR, Minit, Owned, Value,
    bailout::{self, Bailout},
    method_table, new_object, report,
    sys::{ZEND_ACC_FINAL, ZEND_ACC_PUBLIC},
};

/// `SW_ERROR_PHP_FATAL_ERROR`.
const PHP_FATAL_ERROR: c_int = 503;

/// What a Thread\Lock or Thread\Atomic object shares, and a container value
/// holds, across threads: upstream's `ThreadResource` subclasses.
#[derive(Debug)]
pub enum Resource {
    Atomic(AtomicU32),
    Long(AtomicI64),
    /// A thread-private lock (`new Mutex(false)` and its `RWLock`, `SpinLock`
    /// twins), destroyed with its last reference.
    Lock(Lock),
}

/// `LockResource(type)`: a spinlock or rwlock where the build has one, else a
/// mutex.
fn lock_kind(requested: i64, kinds: u32) -> i32 {
    let has = |kind: i64| kinds & (1 << kind) != 0;
    match requested {
        SPIN_LOCK if has(SPIN_LOCK) => 5,
        RW_LOCK if has(RW_LOCK) => 1,
        _ => 3,
    }
}

/// The classes whose objects share a [`Resource`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shareable {
    Lock,
    Atomic,
    Long,
}

/// An object's resource (one reference), unset until `__construct`.
trait Shared: Custom {
    fn resource(&self) -> &OnceCell<Arc<Resource>>;
    fn around(resource: OnceCell<Arc<Resource>>) -> Self;
}

macro_rules! shared {
    ($($name:ident: $class:ident;)*) => {$(
        pub(crate) struct $name(OnceCell<Arc<Resource>>);
        static $class: CustomClass<$name> = CustomClass::new();
        impl Custom for $name {
            fn class() -> &'static CustomClass<Self> {
                &$class
            }
            fn new() -> Self {
                Self(OnceCell::new())
            }
        }
        impl Shared for $name {
            fn resource(&self) -> &OnceCell<Arc<Resource>> {
                &self.0
            }
            fn around(resource: OnceCell<Arc<Resource>>) -> Self {
                Self(resource)
            }
        }
    )*};
}

shared! {
    ThreadAtomic: ATOMIC;
    ThreadLong: LONG;
    ThreadLock: LOCK;
}

/// A Thread\Lock, Thread\Atomic or Thread\Atomic\Long object's class and a new
/// reference to its resource (`None` while unconstructed); `None` for any
/// other value.
#[must_use]
pub fn shared(value: &Value) -> Option<(Shareable, Option<Arc<Resource>>)> {
    fn of<T: Shared>(
        value: &Value,
        class: Shareable,
    ) -> Option<(Shareable, Option<Arc<Resource>>)> {
        Some((class, value.custom::<T>()?.resource().get().cloned()))
    }
    of::<ThreadLock>(value, Shareable::Lock)
        .or_else(|| of::<ThreadAtomic>(value, Shareable::Atomic))
        .or_else(|| of::<ThreadLong>(value, Shareable::Long))
}

/// A new object of `class` over `resource` (unconstructed for `None`), as
/// `php_swoole_thread_lock_create` and its Atomic twins made one.
/// # Panics
/// Before MINIT registered the class.
#[must_use]
pub fn shared_object(class: Shareable, resource: Option<&Arc<Resource>>) -> Owned {
    fn object<T: Shared>(resource: Option<&Arc<Resource>>) -> Owned {
        new_object(T::around(
            resource.cloned().map(OnceCell::from).unwrap_or_default(),
        ))
    }
    match class {
        Shareable::Lock => object::<ThreadLock>(resource),
        Shareable::Atomic => object::<ThreadAtomic>(resource),
        Shareable::Long => object::<ThreadLong>(resource),
    }
}

// Thread\Atomic and Thread\Atomic\Long.

impl Holder for ThreadAtomic {
    type Word = AtomicU32;
    fn word(&self) -> Option<&AtomicU32> {
        match self.0.get()?.as_ref() {
            Resource::Atomic(word) => Some(word),
            _ => None,
        }
    }
}

impl Holder for ThreadLong {
    type Word = AtomicI64;
    fn word(&self) -> Option<&AtomicI64> {
        match self.0.get()?.as_ref() {
            Resource::Long(word) => Some(word),
            _ => None,
        }
    }
}

method_table! {
    static ATOMIC_METHODS = [
        c"__construct" => zim_swoole_thread_atomic___construct, &thread_atomic::arginfo_class_Swoole_Thread_Atomic___construct, ZEND_ACC_PUBLIC;
        c"add" => zim_swoole_thread_atomic_add, &thread_atomic::arginfo_class_Swoole_Thread_Atomic_add, ZEND_ACC_PUBLIC;
        c"sub" => zim_swoole_thread_atomic_sub, &thread_atomic::arginfo_class_Swoole_Thread_Atomic_sub, ZEND_ACC_PUBLIC;
        c"get" => zim_swoole_thread_atomic_get, &thread_atomic::arginfo_class_Swoole_Thread_Atomic_get, ZEND_ACC_PUBLIC;
        c"set" => zim_swoole_thread_atomic_set, &thread_atomic::arginfo_class_Swoole_Thread_Atomic_set, ZEND_ACC_PUBLIC;
        c"wait" => zim_swoole_thread_atomic_wait, &thread_atomic::arginfo_class_Swoole_Thread_Atomic_wait, ZEND_ACC_PUBLIC;
        c"wakeup" => zim_swoole_thread_atomic_wakeup, &thread_atomic::arginfo_class_Swoole_Thread_Atomic_wakeup, ZEND_ACC_PUBLIC;
        c"cmpset" => zim_swoole_thread_atomic_cmpset, &thread_atomic::arginfo_class_Swoole_Thread_Atomic_cmpset, ZEND_ACC_PUBLIC;
    ];
}

method_table! {
    static LONG_METHODS = [
        c"__construct" => zim_swoole_thread_atomic_long___construct, &thread_atomic::arginfo_class_Swoole_Thread_Atomic_Long___construct, ZEND_ACC_PUBLIC;
        c"add" => zim_swoole_thread_atomic_long_add, &thread_atomic::arginfo_class_Swoole_Thread_Atomic_Long_add, ZEND_ACC_PUBLIC;
        c"sub" => zim_swoole_thread_atomic_long_sub, &thread_atomic::arginfo_class_Swoole_Thread_Atomic_Long_sub, ZEND_ACC_PUBLIC;
        c"get" => zim_swoole_thread_atomic_long_get, &thread_atomic::arginfo_class_Swoole_Thread_Atomic_Long_get, ZEND_ACC_PUBLIC;
        c"set" => zim_swoole_thread_atomic_long_set, &thread_atomic::arginfo_class_Swoole_Thread_Atomic_Long_set, ZEND_ACC_PUBLIC;
        c"cmpset" => zim_swoole_thread_atomic_long_cmpset, &thread_atomic::arginfo_class_Swoole_Thread_Atomic_Long_cmpset, ZEND_ACC_PUBLIC;
    ];
}

method_table! {
    static LOCK_METHODS = [
        c"__construct" => zim_swoole_thread_lock___construct, &thread_lock::arginfo_class_Swoole_Thread_Lock___construct, ZEND_ACC_PUBLIC;
        c"lock" => zim_swoole_thread_lock_lock, &thread_lock::arginfo_class_Swoole_Thread_Lock_lock, ZEND_ACC_PUBLIC;
        c"unlock" => zim_swoole_thread_lock_unlock, &thread_lock::arginfo_class_Swoole_Thread_Lock_unlock, ZEND_ACC_PUBLIC;
    ];
}

swoole_rs_zend::minit!(thread_locks_minit => register);

/// MINIT (`php_swoole_thread_atomic_minit`, then `php_swoole_thread_lock_minit`,
/// sharing `Swoole\Lock`'s lock types): register the final classes, the lock's
/// constants and `errCode`.
#[expect(
    clippy::unnecessary_wraps,
    reason = "minit! raises the bailout a MINIT body returns"
)]
fn register(minit: &Minit) -> Result<(), Bailout> {
    minit.register_object::<ThreadAtomic>(
        c"Swoole\\Thread\\Atomic",
        &ATOMIC_METHODS.0,
        ZEND_ACC_FINAL,
    );
    minit.register_object::<ThreadLong>(
        c"Swoole\\Thread\\Atomic\\Long",
        &LONG_METHODS.0,
        ZEND_ACC_FINAL,
    );
    minit.register_object::<ThreadLock>(c"Swoole\\Thread\\Lock", &LOCK_METHODS.0, ZEND_ACC_FINAL);
    let types = [
        (c"MUTEX", MUTEX),
        (c"RWLOCK", RW_LOCK),
        (c"SPINLOCK", SPIN_LOCK),
    ];
    for (name, kind) in types {
        if KINDS & (1 << kind) != 0 {
            minit.class_constant_long(&LOCK, name, kind);
        }
    }
    minit.property_long(&LOCK, c"errCode", 0, ZEND_ACC_PUBLIC);
    Ok(())
}

/// `__construct(int $value = 0)`: make the resource, once.
fn construct<T: Shared + Holder>(call: &Call, long: bool, resource: fn() -> Resource) {
    let Ok(value) = call.arity(0, 1).and_then(|()| call.long(1, 0)) else {
        call.set_bool(false);
        return;
    };
    let Some(this) = call.custom::<T>() else {
        return;
    };
    if this.resource().get().is_some() {
        call.throw_constructed_twice();
        // Upstream's Long constructor returns false here, Atomic's nothing.
        if long {
            call.set_bool(false);
        }
        return;
    }
    this.resource().get_or_init(|| Arc::new(resource()));
    if let Some(word) = this.word() {
        word::Word::set(word, value);
    }
}

fn construct_atomic(call: &Call) {
    construct::<ThreadAtomic>(call, false, || Resource::Atomic(AtomicU32::new(0)));
}

fn construct_long(call: &Call) {
    construct::<ThreadLong>(call, true, || Resource::Long(AtomicI64::new(0)));
}

// Thread\Lock.

/// The constructed lock, or the fatal error upstream intends for a method
/// called before the constructor (`php_swoole_fatal_error(E_ERROR, ...)`).
fn constructed(call: &Call) -> Option<&Lock> {
    let resource = call.custom::<ThreadLock>()?.0.get();
    let Some(resource) = resource else {
        set_last_error(PHP_FATAL_ERROR);
        bailout::defer(report(E_ERROR, c"must call constructor first"));
        return None;
    };
    match resource.as_ref() {
        Resource::Lock(lock) => Some(lock),
        _ => None,
    }
}

/// `__construct(int $type = Lock::MUTEX)`.
fn lock_construct(call: &Call) {
    let Some(this) = call.custom::<ThreadLock>() else {
        return;
    };
    if this.0.get().is_some() {
        call.throw_constructed_twice();
        call.set_bool(false);
        return;
    }
    let Ok(requested) = call.arity(0, 1).and_then(|()| call.long(1, MUTEX)) else {
        return;
    };
    // Upstream throws a C++ exception, which terminates the process, when a
    // lock cannot be made.
    let lock = Lock::private(lock_kind(requested, KINDS)).unwrap_or_else(|| std::process::abort());
    this.0.get_or_init(|| Arc::new(Resource::Lock(lock)));
}

/// `lock(int $operation = LOCK_EX, float $timeout = -1): bool`.
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
    zim_swoole_thread_atomic___construct => construct_atomic;
    zim_swoole_thread_atomic_long___construct => construct_long;
    zim_swoole_thread_lock___construct => lock_construct;
}

swoole_rs_zend::handlers! {
    leaf:
    zim_swoole_thread_lock_lock => lock;
    zim_swoole_thread_lock_unlock => unlock;
    zim_swoole_thread_atomic_add => word::add::<ThreadAtomic>;
    zim_swoole_thread_atomic_sub => word::sub::<ThreadAtomic>;
    zim_swoole_thread_atomic_get => word::get::<ThreadAtomic>;
    zim_swoole_thread_atomic_set => word::set::<ThreadAtomic>;
    zim_swoole_thread_atomic_cmpset => word::cmpset::<ThreadAtomic>;
    zim_swoole_thread_atomic_wait => word::wait::<ThreadAtomic>;
    zim_swoole_thread_atomic_wakeup => word::wakeup::<ThreadAtomic>;
    zim_swoole_thread_atomic_long_add => word::add::<ThreadLong>;
    zim_swoole_thread_atomic_long_sub => word::sub::<ThreadLong>;
    zim_swoole_thread_atomic_long_get => word::get::<ThreadLong>;
    zim_swoole_thread_atomic_long_set => word::set::<ThreadLong>;
    zim_swoole_thread_atomic_long_cmpset => word::cmpset::<ThreadLong>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_types_fall_back_to_a_mutex() {
        let all = (1 << MUTEX) | (1 << RW_LOCK) | (1 << SPIN_LOCK);
        assert_eq!(
            lock_kind(SPIN_LOCK, all),
            5,
            "a spinlock where the build has one"
        );
        assert_eq!(
            lock_kind(RW_LOCK, all),
            1,
            "an rwlock where the build has one"
        );
        assert_eq!(lock_kind(SPIN_LOCK, 1 << MUTEX), 3, "no spinlock: a mutex");
        assert_eq!(lock_kind(42, all), 3, "an unknown type: a mutex");
    }

    #[test]
    fn a_lock_resource_locks_and_is_destroyed_with_its_last_reference() {
        let lock = Lock::private(3).expect("a mutex");
        let resource = Arc::new(Resource::Lock(lock));
        let Resource::Lock(lock) = &*resource else {
            unreachable!("made as a lock");
        };
        assert_eq!(lock.lock(2, -1, 0), 0, "locks");
        assert_eq!(lock.unlock(), 0, "unlocks");
        let shared = Arc::clone(&resource);
        drop(resource);
        assert!(matches!(*shared, Resource::Lock(_)), "alive while shared");
    }
}
