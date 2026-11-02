//! `Swoole\Thread\Atomic` and `Swoole\Thread\Atomic\Long` (ZTS builds): the word
//! lives in a C++ `ThreadResource`, which `Thread\Map`, `ArrayList` and `Queue`
//! reference-count across threads, so C++ makes and releases it.
use crate::word::{self, Word, methods};
use std::{ffi::c_void, ptr, sync::OnceLock};
use swoole_rs_phpstubs::thread_atomic;
use swoole_rs_zend::{
    Call, Class, Object, method_table, set_object,
    sys::{
        ZEND_ACC_FINAL, ZEND_ACC_NOT_SERIALIZABLE, ZEND_ACC_PUBLIC, zend_class_entry, zend_object,
        zend_object_handlers, zval,
    },
    throw_constructed_twice, unset_property_deny,
};

/// Makes a resource holding `value` and stores the address of its word.
type CreateResource =
    unsafe extern "C" fn(is_long: i32, value: i64, word: *mut *mut c_void) -> *mut c_void;
type DelRef = unsafe extern "C" fn(resource: *mut c_void);

struct Resources {
    create: CreateResource,
    del_ref: DelRef,
}

static RESOURCES: OnceLock<Resources> = OnceLock::new();
static ATOMIC: Class = Class::new();
static LONG: Class = Class::new();

/// The `ThreadResource` and its word; both null until `__construct`.
#[repr(C)]
struct Shared<W> {
    resource: *mut c_void,
    word: *mut W,
}

type ThreadAtomic<W> = Object<Shared<W>>;

trait Kind: Word {
    const IS_LONG: i32;
    fn class() -> &'static Class;
}

impl Kind for u32 {
    const IS_LONG: i32 = 0;
    fn class() -> &'static Class {
        &ATOMIC
    }
}

impl Kind for i64 {
    const IS_LONG: i32 = 1;
    fn class() -> &'static Class {
        &LONG
    }
}

unsafe extern "C" fn create<W: Kind>(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: Zend calls `create_object` with this class; null pointers are
    // valid zeroed data.
    unsafe { ThreadAtomic::<W>::create(W::class(), ce) }
}

unsafe extern "C" fn free<W: Kind>(std: *mut zend_object) {
    // SAFETY: Zend frees an object this class created.
    let shared = unsafe { ThreadAtomic::<W>::data(std) };
    if !shared.resource.is_null()
        && let Some(resources) = RESOURCES.get()
    {
        // SAFETY: the object holds one reference to the resource.
        unsafe { (resources.del_ref)(shared.resource) };
    }
    *shared = Shared {
        resource: ptr::null_mut(),
        word: ptr::null_mut(),
    };
    // SAFETY: as above.
    unsafe { ThreadAtomic::<W>::dtor(std) };
}

fn this_word<W>(call: &Call) -> *mut W {
    // SAFETY: Thread\Atomic methods are non-static and the classes final, so
    // `$this` is one of our objects.
    unsafe { ThreadAtomic::<W>::data(call.this()).word }
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

/// MINIT: register both classes as `php_swoole_thread_atomic_minit` did and
/// publish their entries to C++ (Thread\Map and friends test against them).
/// # Safety
/// Called once, at MINIT, with the resource functions and writable class-entry
/// slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phplocks_thread_atomic_minit(
    create: Option<CreateResource>,
    del_ref: Option<DelRef>,
    atomic_class: *mut *mut zend_class_entry,
    long_class: *mut *mut zend_class_entry,
) {
    if let (Some(create), Some(del_ref)) = (create, del_ref) {
        RESOURCES.get_or_init(|| Resources { create, del_ref });
    }
    // SAFETY: MINIT, with static tables.
    unsafe {
        ATOMIC.register(
            c"Swoole\\Thread\\Atomic",
            ATOMIC_METHODS.as_ptr(),
            configure::<u32>,
        );
    }
    // SAFETY: as above.
    unsafe {
        LONG.register(
            c"Swoole\\Thread\\Atomic\\Long",
            LONG_METHODS.as_ptr(),
            configure::<i64>,
        );
    }
    // SAFETY: the caller's writable slots.
    unsafe { *atomic_class = ATOMIC.entry() };
    // SAFETY: as above.
    unsafe { *long_class = LONG.entry() };
}

/// Final and not serializable, clone and unset denied, custom object.
fn configure<W: Kind>(entry: &mut zend_class_entry, handlers: &mut zend_object_handlers) {
    entry.ce_flags |= ZEND_ACC_FINAL | ZEND_ACC_NOT_SERIALIZABLE;
    handlers.clone_obj = None;
    handlers.unset_property = Some(unset_property_deny);
    ThreadAtomic::<W>::install(entry, handlers, create::<W>, free::<W>);
}

/// `php_swoole_thread_atomic_cast`: the resource an object holds (either class;
/// both put it first).
/// # Safety
/// `object` is a Thread\Atomic or Thread\Atomic\Long.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phplocks_thread_atomic_resource(
    object: *const zval,
) -> *mut c_void {
    // SAFETY: an object zval.
    let value = unsafe { (*object).value };
    // SAFETY: as above, so the value holds `obj`.
    let std = unsafe { value.obj };
    // SAFETY: one of our objects, whose resource is its first field.
    unsafe { ThreadAtomic::<u32>::data(std).resource }
}

/// `php_swoole_thread_atomic_create`: an object over a resource another thread
/// shared, taking over the reference the caller holds.
/// # Safety
/// `return_value` is writable; `word` is the resource's live word or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phplocks_thread_atomic_create(
    return_value: *mut zval,
    is_long: i32,
    resource: *mut c_void,
    word: *mut c_void,
) {
    let (class, std) = if is_long == 0 {
        (&ATOMIC, adopt::<u32>(resource, word))
    } else {
        (&LONG, adopt::<i64>(resource, word))
    };
    debug_assert!(!class.entry().is_null(), "created after MINIT");
    // SAFETY: per the caller; `std` is a new object.
    unsafe { set_object(return_value, std) };
}

fn adopt<W: Kind>(resource: *mut c_void, word: *mut c_void) -> *mut zend_object {
    // SAFETY: after MINIT; the class's own entry.
    let std = unsafe { create::<W>(W::class().entry()) };
    // SAFETY: the object just created.
    unsafe {
        *ThreadAtomic::<W>::data(std) = Shared {
            resource,
            word: word.cast(),
        };
    }
    std
}

/// `__construct(int $value = 0)`: make the resource, once.
fn construct<W: Kind>(call: &Call, _word: *mut W) {
    let Ok(value) = call.arity(0, 1).and_then(|()| call.long(1, 0)) else {
        call.set_bool(false);
        return;
    };
    // SAFETY: as in `this_word`.
    let shared = unsafe { ThreadAtomic::<W>::data(call.this()) };
    if !shared.resource.is_null() {
        // SAFETY: in a handler, with `$this` live.
        unsafe { throw_constructed_twice(call.this()) };
        // Upstream's Long constructor returns false here, Atomic's nothing.
        if W::IS_LONG != 0 {
            call.set_bool(false);
        }
        return;
    }
    let Some(resources) = RESOURCES.get() else {
        return;
    };
    let mut word = ptr::null_mut();
    // SAFETY: C++ makes a resource holding `value` and writes its word's address.
    let resource = unsafe { (resources.create)(W::IS_LONG, value, &raw mut word) };
    *shared = Shared {
        resource,
        word: word.cast(),
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
