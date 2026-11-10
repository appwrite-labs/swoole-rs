//! `Swoole\Atomic` and `Swoole\Atomic\Long`: a word in Swoole's global memory
//! pool, so processes forked after construction share it.
use crate::word::{self, Word, methods};
use std::ffi::c_int;
use swoole_rs_memory::members::pool;
use swoole_rs_phpstubs::atomic;
use swoole_rs_zend::{
    Call, Class, Minit, Object, method_table,
    sys::{ZEND_ACC_NOT_SERIALIZABLE, ZEND_ACC_PUBLIC, zend_class_entry, zend_object},
    unset_property_deny,
};

/// `SW_ERROR_MALLOC_FAIL`.
const MALLOC_FAIL: i64 = 501;

static ATOMIC: Class = Class::new();
static LONG: Class = Class::new();

/// `struct AtomicObject { sw_atomic_t *ptr; zend_object std; }` and its Long twin.
type Atomic<W> = Object<*mut W>;

trait Kind: Word {
    fn class() -> &'static Class;
}

impl Kind for u32 {
    fn class() -> &'static Class {
        &ATOMIC
    }
}

impl Kind for i64 {
    fn class() -> &'static Class {
        &LONG
    }
}

unsafe extern "C" fn create<W: Kind>(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: Zend calls `create_object` with this class or a subclass of it;
    // a null pointer is valid zeroed data.
    let std = unsafe { Atomic::<W>::create(W::class(), ce) };
    #[expect(clippy::cast_possible_truncation, reason = "a word is 4 or 8 bytes")]
    let word = pool::alloc(size_of::<W>() as u32);
    if word.is_null() {
        swoole_rs_phpswoole::exception().throw(c"global memory allocation failure", MALLOC_FAIL);
    }
    // SAFETY: `std` is the object just created.
    unsafe { *Atomic::<W>::data(std) = word.cast() };
    std
}

unsafe extern "C" fn free<W: Kind>(std: *mut zend_object) {
    // The word stays: the global pool frees nothing.
    // SAFETY: Zend frees an object this class created.
    unsafe { Atomic::<W>::dtor(std) };
}

fn this_word<W>(call: &Call) -> *mut W {
    // SAFETY: Atomic methods are non-static, so `$this` is one of our objects.
    unsafe { *Atomic::<W>::data(call.this()) }
}

method_table! {
    static ATOMIC_METHODS = [
        c"__construct" => zim_swoole_atomic___construct, &atomic::arginfo_class_Swoole_Atomic___construct, ZEND_ACC_PUBLIC;
        c"add" => zim_swoole_atomic_add, &atomic::arginfo_class_Swoole_Atomic_add, ZEND_ACC_PUBLIC;
        c"sub" => zim_swoole_atomic_sub, &atomic::arginfo_class_Swoole_Atomic_sub, ZEND_ACC_PUBLIC;
        c"get" => zim_swoole_atomic_get, &atomic::arginfo_class_Swoole_Atomic_get, ZEND_ACC_PUBLIC;
        c"set" => zim_swoole_atomic_set, &atomic::arginfo_class_Swoole_Atomic_set, ZEND_ACC_PUBLIC;
        c"wait" => zim_swoole_atomic_wait, &atomic::arginfo_class_Swoole_Atomic_wait, ZEND_ACC_PUBLIC;
        c"wakeup" => zim_swoole_atomic_wakeup, &atomic::arginfo_class_Swoole_Atomic_wakeup, ZEND_ACC_PUBLIC;
        c"cmpset" => zim_swoole_atomic_cmpset, &atomic::arginfo_class_Swoole_Atomic_cmpset, ZEND_ACC_PUBLIC;
    ];
}

method_table! {
    static LONG_METHODS = [
        c"__construct" => zim_swoole_atomic_long___construct, &atomic::arginfo_class_Swoole_Atomic_Long___construct, ZEND_ACC_PUBLIC;
        c"add" => zim_swoole_atomic_long_add, &atomic::arginfo_class_Swoole_Atomic_Long_add, ZEND_ACC_PUBLIC;
        c"sub" => zim_swoole_atomic_long_sub, &atomic::arginfo_class_Swoole_Atomic_Long_sub, ZEND_ACC_PUBLIC;
        c"get" => zim_swoole_atomic_long_get, &atomic::arginfo_class_Swoole_Atomic_Long_get, ZEND_ACC_PUBLIC;
        c"set" => zim_swoole_atomic_long_set, &atomic::arginfo_class_Swoole_Atomic_Long_set, ZEND_ACC_PUBLIC;
        c"cmpset" => zim_swoole_atomic_long_cmpset, &atomic::arginfo_class_Swoole_Atomic_Long_cmpset, ZEND_ACC_PUBLIC;
    ];
}

/// `php_swoole_atomic_minit`: register both classes.
/// # Safety
/// Called once, at MINIT.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn php_swoole_atomic_minit(module_number: c_int) {
    // SAFETY: Zend's MINIT of this module, on this thread.
    let minit = unsafe { Minit::new(module_number) };
    minit.register(
        &ATOMIC,
        c"Swoole\\Atomic",
        &ATOMIC_METHODS.0,
        configure::<u32>,
    );
    minit.register(
        &LONG,
        c"Swoole\\Atomic\\Long",
        &LONG_METHODS.0,
        configure::<i64>,
    );
}

/// `SW_SET_CLASS_NOT_SERIALIZABLE`, clone and unset denied, custom object.
fn configure<W: Kind>(
    entry: &mut zend_class_entry,
    handlers: &mut swoole_rs_zend::sys::zend_object_handlers,
) {
    entry.ce_flags |= ZEND_ACC_NOT_SERIALIZABLE;
    handlers.clone_obj = None;
    handlers.unset_property = Some(unset_property_deny);
    Atomic::<W>::install(entry, handlers, create::<W>, free::<W>);
}

fn construct(call: &Call, word: *mut u32) {
    word::construct(call, word);
}

/// `Atomic\Long::__construct` returns true.
fn construct_long(call: &Call, word: *mut i64) {
    if word::construct(call, word) {
        call.set_bool(true);
    }
}

methods! {
    this_word;
    zim_swoole_atomic___construct: u32 => construct;
    zim_swoole_atomic_add: u32 => word::add;
    zim_swoole_atomic_sub: u32 => word::sub;
    zim_swoole_atomic_get: u32 => word::get;
    zim_swoole_atomic_set: u32 => word::set;
    zim_swoole_atomic_cmpset: u32 => word::cmpset;
    zim_swoole_atomic_wait: u32 => word::wait;
    zim_swoole_atomic_wakeup: u32 => word::wakeup;
    zim_swoole_atomic_long___construct: i64 => construct_long;
    zim_swoole_atomic_long_add: i64 => word::add;
    zim_swoole_atomic_long_sub: i64 => word::sub;
    zim_swoole_atomic_long_get: i64 => word::get;
    zim_swoole_atomic_long_set: i64 => word::set;
    zim_swoole_atomic_long_cmpset: i64 => word::cmpset;
}
