//! `Swoole\Thread\Atomic`, `Swoole\Thread\Atomic\Long` and `Swoole\Thread\Lock`
//! (ZTS builds): the word or lock lives in a reference-counted [`Resource`]
//! (upstream's `ThreadResource`), which `Thread\Map`, `ArrayList` and `Queue`
//! (crates/phpthread) share between threads through [`shared`] and
//! [`shared_object`].
use crate::{
    lock::{CAPABILITIES, KINDS, LOCK_EX, MUTEX, RW_LOCK, SPIN_LOCK, check, milliseconds},
    word::{self, Word, methods},
};
use std::{
    alloc::{Layout, alloc_zeroed, dealloc},
    ffi::{c_char, c_int, c_void},
    ptr,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicI64, AtomicU32},
    },
};
use swoole_rs_locks::{
    swr_v1_locks_align, swr_v1_locks_destroy, swr_v1_locks_init, swr_v1_locks_lock,
    swr_v1_locks_size, swr_v1_locks_unlock,
};
use swoole_rs_phpstubs::{thread_atomic, thread_lock};
use swoole_rs_zend::{
    Call, Class, Minit, Object, bailout, method_table, set_object,
    sys::{
        ZEND_ACC_FINAL, ZEND_ACC_NOT_SERIALIZABLE, ZEND_ACC_PUBLIC, zend_class_entry, zend_object,
        zend_object_handlers, zval,
    },
    unset_property_deny,
};

static ATOMIC: Class = Class::new();
static LONG: Class = Class::new();
static LOCK: Class = Class::new();
/// `php_swoole_fatal_error(E_ERROR, "%s", message)`, fixed at MINIT.
static UNCONSTRUCTED: OnceLock<unsafe extern "C" fn(*const c_char)> = OnceLock::new();

/// What a Thread\Lock or Thread\Atomic object shares, and a container value
/// holds, across threads: upstream's `ThreadResource` subclasses.
#[derive(Debug)]
pub enum Resource {
    Atomic(AtomicU32),
    Long(AtomicI64),
    Lock(ThreadLock),
}

impl Resource {
    /// The word an atomic object operates on, or the lock's storage.
    fn word(&self) -> *mut c_void {
        match self {
            Self::Atomic(word) => word.as_ptr().cast(),
            Self::Long(word) => word.as_ptr().cast(),
            Self::Lock(lock) => lock.storage,
        }
    }
}

/// A thread-private lock (`new Mutex(false)` and its `RWLock`, `SpinLock`
/// twins), destroyed with its last reference.
#[derive(Debug)]
pub struct ThreadLock {
    storage: *mut c_void,
    kind: i32,
}

// SAFETY: a pthread lock is made to be used from any thread; the storage is
// owned by the lock and freed once, by `Drop`.
unsafe impl Send for ThreadLock {}
// SAFETY: as above; every operation goes through the pthread functions.
unsafe impl Sync for ThreadLock {}

/// The storage a lock of `kind` takes. Upstream throws a C++ exception, which
/// terminates the process, when a lock cannot be made.
fn layout(kind: i32) -> Layout {
    let size = swr_v1_locks_size(kind);
    match Layout::from_size_align(size, swr_v1_locks_align(kind)) {
        Ok(layout) if size != 0 => layout,
        _ => std::process::abort(),
    }
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

impl ThreadLock {
    fn new(kind: i32) -> Self {
        let layout = layout(kind);
        // SAFETY: a non-zero size.
        let storage = unsafe { alloc_zeroed(layout) }.cast::<c_void>();
        // SAFETY: `layout` bytes, aligned for `kind`, owned by this lock.
        if storage.is_null() || unsafe { swr_v1_locks_init(storage, kind, 0) } != 0 {
            std::process::abort();
        }
        Self { storage, kind }
    }
}

impl Drop for ThreadLock {
    fn drop(&mut self) {
        // SAFETY: the last reference: nothing else uses the lock.
        unsafe { swr_v1_locks_destroy(self.storage, self.kind) };
        // SAFETY: allocated by `new` with this layout.
        unsafe { dealloc(self.storage.cast(), layout(self.kind)) };
    }
}

/// The classes whose objects share a [`Resource`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shareable {
    Lock,
    Atomic,
    Long,
}

impl Shareable {
    fn class(self) -> &'static Class {
        match self {
            Self::Lock => &LOCK,
            Self::Atomic => &ATOMIC,
            Self::Long => &LONG,
        }
    }
}

/// An object's resource (one reference, from `Arc::into_raw`) and its word or
/// lock storage; both null until `__construct`.
#[repr(C)]
struct Shared {
    resource: *const Resource,
    word: *mut c_void,
}

type ThreadObject = Object<Shared>;

unsafe extern "C" fn create_atomic(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: Zend calls `create_object` with this class; null pointers are
    // valid zeroed data.
    unsafe { ThreadObject::create(&ATOMIC, ce) }
}

unsafe extern "C" fn create_long(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: as in `create_atomic`.
    unsafe { ThreadObject::create(&LONG, ce) }
}

unsafe extern "C" fn create_lock(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: as in `create_atomic`.
    unsafe { ThreadObject::create(&LOCK, ce) }
}

unsafe extern "C" fn free(std: *mut zend_object) {
    // SAFETY: Zend frees an object one of these classes created.
    let shared = unsafe { ThreadObject::data(std) };
    let resource = std::mem::replace(&mut shared.resource, ptr::null());
    shared.word = ptr::null_mut();
    if !resource.is_null() {
        // SAFETY: the reference the object held.
        drop(unsafe { Arc::from_raw(resource) });
    }
    // SAFETY: as above.
    unsafe { ThreadObject::dtor(std) };
}

/// A Thread\Lock, Thread\Atomic or Thread\Atomic\Long object's class and a new
/// reference to its resource (`None` while unconstructed); `None` for any
/// other object.
/// # Safety
/// `std` is a live object.
#[must_use]
pub unsafe fn shared(std: *mut zend_object) -> Option<(Shareable, Option<Arc<Resource>>)> {
    // SAFETY: per the caller.
    let ce = unsafe { (*std).ce };
    let class = [Shareable::Lock, Shareable::Atomic, Shareable::Long]
        .into_iter()
        .find(|class| class.class().entry() == ce)?;
    // SAFETY: one of our objects (the classes are final).
    let resource = unsafe { ThreadObject::data(std) }.resource;
    if resource.is_null() {
        return Some((class, None));
    }
    // SAFETY: the object's live reference.
    unsafe { Arc::increment_strong_count(resource) };
    // SAFETY: the reference just added.
    Some((class, Some(unsafe { Arc::from_raw(resource) })))
}

/// A new object of `class` over `resource` (unconstructed for `None`), as
/// `php_swoole_thread_lock_create` and its Atomic twins made one.
/// # Safety
/// In a request, after MINIT; `out` is writable and holds nothing.
pub unsafe fn shared_object(out: *mut zval, class: Shareable, resource: Option<&Arc<Resource>>) {
    let class = class.class();
    // SAFETY: after MINIT; the class's own entry.
    let std = unsafe { ThreadObject::create(class, class.entry()) };
    if let Some(resource) = resource {
        // SAFETY: the object just created.
        unsafe {
            *ThreadObject::data(std) = Shared {
                resource: Arc::into_raw(Arc::clone(resource)),
                word: resource.word(),
            };
        }
    }
    // SAFETY: per the caller; `std` is a new object.
    unsafe { set_object(out, std) };
}

/// Final and not serializable, clone and unset denied, custom object.
fn configure(
    entry: &mut zend_class_entry,
    handlers: &mut zend_object_handlers,
    create: unsafe extern "C" fn(*mut zend_class_entry) -> *mut zend_object,
) {
    entry.ce_flags |= ZEND_ACC_FINAL | ZEND_ACC_NOT_SERIALIZABLE;
    handlers.clone_obj = None;
    handlers.unset_property = Some(unset_property_deny);
    ThreadObject::install(entry, handlers, create, free);
}

// Thread\Atomic and Thread\Atomic\Long.

trait Kind: Word {
    const IS_LONG: bool;
    fn resource() -> Resource;
}

impl Kind for u32 {
    const IS_LONG: bool = false;
    fn resource() -> Resource {
        Resource::Atomic(AtomicU32::new(0))
    }
}

impl Kind for i64 {
    const IS_LONG: bool = true;
    fn resource() -> Resource {
        Resource::Long(AtomicI64::new(0))
    }
}

fn this_word<W>(call: &Call) -> *mut W {
    // SAFETY: Thread\Atomic methods are non-static and the classes final, so
    // `$this` is one of our objects.
    unsafe { ThreadObject::data(call.this()) }.word.cast()
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

/// MINIT (`php_swoole_thread_atomic_minit`): register both classes.
/// # Safety
/// Called once, at MINIT.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phplocks_thread_atomic_minit(module_number: c_int) {
    // SAFETY: per the caller: Zend's MINIT of this module, on this thread.
    let minit = unsafe { Minit::new(module_number) };
    minit.register(
        &ATOMIC,
        c"Swoole\\Thread\\Atomic",
        &ATOMIC_METHODS.0,
        |e, h| {
            configure(e, h, create_atomic);
        },
    );
    minit.register(
        &LONG,
        c"Swoole\\Thread\\Atomic\\Long",
        &LONG_METHODS.0,
        |e, h| {
            configure(e, h, create_long);
        },
    );
}

/// `__construct(int $value = 0)`: make the resource, once.
fn construct<W: Kind>(call: &Call, _word: *mut W) {
    let Ok(value) = call.arity(0, 1).and_then(|()| call.long(1, 0)) else {
        call.set_bool(false);
        return;
    };
    // SAFETY: as in `this_word`.
    let shared = unsafe { ThreadObject::data(call.this()) };
    if !shared.resource.is_null() {
        call.throw_constructed_twice();
        // Upstream's Long constructor returns false here, Atomic's nothing.
        if W::IS_LONG {
            call.set_bool(false);
        }
        return;
    }
    let resource = Arc::new(W::resource());
    let word = resource.word();
    // SAFETY: the fresh resource's word, shared only through `Arc`s; the
    // value is cast to the word as upstream's constructors do.
    unsafe { W::apply(word.cast(), word::STORE, value, 0) };
    *shared = Shared {
        resource: Arc::into_raw(resource),
        word,
    };
}

methods! {
    this_word;
    zim_swoole_thread_atomic___construct: u32 => construct::<u32>;
    zim_swoole_thread_atomic_add: u32 => word::add;
    zim_swoole_thread_atomic_sub: u32 => word::sub;
    zim_swoole_thread_atomic_get: u32 => word::get;
    zim_swoole_thread_atomic_set: u32 => word::set;
    zim_swoole_thread_atomic_cmpset: u32 => word::cmpset;
    zim_swoole_thread_atomic_wait: u32 => word::wait;
    zim_swoole_thread_atomic_wakeup: u32 => word::wakeup;
    zim_swoole_thread_atomic_long___construct: i64 => construct::<i64>;
    zim_swoole_thread_atomic_long_add: i64 => word::add;
    zim_swoole_thread_atomic_long_sub: i64 => word::sub;
    zim_swoole_thread_atomic_long_get: i64 => word::get;
    zim_swoole_thread_atomic_long_set: i64 => word::set;
    zim_swoole_thread_atomic_long_cmpset: i64 => word::cmpset;
}

// Thread\Lock.

method_table! {
    static LOCK_METHODS = [
        c"__construct" => zim_swoole_thread_lock___construct, &thread_lock::arginfo_class_Swoole_Thread_Lock___construct, ZEND_ACC_PUBLIC;
        c"lock" => zim_swoole_thread_lock_lock, &thread_lock::arginfo_class_Swoole_Thread_Lock_lock, ZEND_ACC_PUBLIC;
        c"unlock" => zim_swoole_thread_lock_unlock, &thread_lock::arginfo_class_Swoole_Thread_Lock_unlock, ZEND_ACC_PUBLIC;
    ];
}

/// MINIT (`php_swoole_thread_lock_minit`), sharing `Swoole\Lock`'s
/// lock types and capabilities: register the class, its
/// constants and `errCode`.
/// # Safety
/// Called once, at MINIT, with C++'s fatal-error reporter.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phplocks_thread_lock_minit(
    module_number: c_int,
    unconstructed: Option<unsafe extern "C" fn(*const c_char)>,
) {
    if let Some(unconstructed) = unconstructed {
        UNCONSTRUCTED.get_or_init(|| unconstructed);
    }
    // SAFETY: per the caller: Zend's MINIT of this module, on this thread.
    let minit = unsafe { Minit::new(module_number) };
    minit.register(&LOCK, c"Swoole\\Thread\\Lock", &LOCK_METHODS.0, |e, h| {
        configure(e, h, create_lock);
    });
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
}

/// `$this`'s lock.
fn this_lock(call: &Call) -> &'static mut Shared {
    // SAFETY: Thread\Lock methods are non-static and the class final.
    unsafe { ThreadObject::data(call.this()) }
}

/// The constructed lock's resource, or the fatal error upstream intends for a
/// method called before the constructor.
fn constructed(call: &Call) -> Option<&'static ThreadLock> {
    let resource = this_lock(call).resource;
    if resource.is_null() {
        if let Some(report) = UNCONSTRUCTED.get() {
            // SAFETY: C++'s reporter, trapped; the handler raises the bailout.
            bailout::defer(unsafe { bailout::report(*report, c"must call constructor first") });
        }
        return None;
    }
    // SAFETY: the object holds a reference for as long as it lives.
    match unsafe { &*resource } {
        Resource::Lock(lock) => Some(lock),
        _ => None,
    }
}

/// `__construct(int $type = Lock::MUTEX)`.
fn lock_construct(call: &Call) {
    let shared = this_lock(call);
    if !shared.resource.is_null() {
        call.throw_constructed_twice();
        call.set_bool(false);
        return;
    }
    let Ok(requested) = call.arity(0, 1).and_then(|()| call.long(1, MUTEX)) else {
        return;
    };
    let resource = Arc::new(Resource::Lock(ThreadLock::new(lock_kind(requested, KINDS))));
    *shared = Shared {
        word: resource.word(),
        resource: Arc::into_raw(resource),
    };
}

/// `lock(int $operation = LOCK_EX, float $timeout = -1): bool`.
fn lock(call: &Call) {
    let parsed = call
        .arity(0, 2)
        .and_then(|()| Ok((call.long(1, LOCK_EX)?, call.double(2, -1.0)?)));
    let Ok((operation, timeout)) = parsed else {
        call.set_bool(false);
        return;
    };
    let Some(lock) = constructed(call) else {
        return;
    };
    #[expect(clippy::cast_possible_truncation, reason = "upstream passes an int")]
    // SAFETY: a lock `ThreadLock::new` initialised.
    let result = unsafe {
        swr_v1_locks_lock(
            lock.storage,
            lock.kind,
            operation as i32,
            milliseconds(timeout),
            CAPABILITIES,
        )
    };
    check(call, result);
}

/// `unlock(): bool`.
fn unlock(call: &Call) {
    if let Some(lock) = constructed(call) {
        // SAFETY: as in `lock`.
        check(call, unsafe {
            swr_v1_locks_unlock(lock.storage, lock.kind)
        });
    }
}

swoole_rs_zend::handlers! {
    zim_swoole_thread_lock___construct => lock_construct;
    zim_swoole_thread_lock_lock => lock;
    zim_swoole_thread_lock_unlock => unlock;
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
        let resource = Arc::new(Resource::Lock(ThreadLock::new(3)));
        let Resource::Lock(lock) = &*resource else {
            unreachable!("made as a lock");
        };
        // SAFETY: an initialised mutex.
        assert_eq!(
            unsafe { swr_v1_locks_lock(lock.storage, 3, 2, -1, 0) },
            0,
            "locks"
        );
        // SAFETY: held by this thread.
        assert_eq!(
            unsafe { swr_v1_locks_unlock(lock.storage, 3) },
            0,
            "unlocks"
        );
        let shared = Arc::clone(&resource);
        drop(resource);
        assert!(matches!(*shared, Resource::Lock(_)), "alive while shared");
    }

    #[test]
    fn atomic_words_are_the_resource_words() {
        let resource = Resource::Atomic(AtomicU32::new(7));
        // SAFETY: the resource's live word.
        assert_eq!(
            unsafe { *resource.word().cast::<u32>() },
            7,
            "the same word"
        );
    }
}
