//! `Swoole\Atomic` and `Swoole\Atomic\Long`: a word in Swoole's global memory
//! pool, so processes forked after construction share it.
use crate::word::{self, Holder};
use std::sync::atomic::{AtomicI64, AtomicU32};
use swoole_rs_memory::members::pool;
use swoole_rs_phpstubs::atomic;
use swoole_rs_zend::{Call, Custom, CustomClass, Minit, method_table, sys::ZEND_ACC_PUBLIC};

/// `SW_ERROR_MALLOC_FAIL`.
const MALLOC_FAIL: i64 = 501;

static ATOMIC: CustomClass<Atomic> = CustomClass::new();
static LONG: CustomClass<Long> = CustomClass::new();

/// `struct AtomicObject { sw_atomic_t *ptr; zend_object std; }`.
pub(crate) struct Atomic(Option<&'static AtomicU32>);
/// `struct AtomicLongObject { sw_atomic_long_t *ptr; zend_object std; }`.
pub(crate) struct Long(Option<&'static AtomicI64>);

/// The pool's word, or Swoole's exception when it has none.
fn alloc<W>(word: Option<&'static W>) -> Option<&'static W> {
    if word.is_none() {
        swoole_rs_phpswoole::exception().throw(c"global memory allocation failure", MALLOC_FAIL);
    }
    word
}

impl Custom for Atomic {
    fn class() -> &'static CustomClass<Self> {
        &ATOMIC
    }
    fn new() -> Self {
        Self(alloc(pool::words(1).and_then(|words| words.first())))
    }
}

impl Custom for Long {
    fn class() -> &'static CustomClass<Self> {
        &LONG
    }
    fn new() -> Self {
        Self(alloc(pool::words(1).and_then(|words| words.first())))
    }
}

impl Holder for Atomic {
    type Word = AtomicU32;
    fn word(&self) -> Option<&AtomicU32> {
        self.0
    }
}

impl Holder for Long {
    type Word = AtomicI64;
    fn word(&self) -> Option<&AtomicI64> {
        self.0
    }
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
/// # Errors
/// A registration bailed out.
pub fn atomic_minit(minit: &Minit) -> Result<(), swoole_rs_zend::bailout::Bailout> {
    minit.register_object::<Atomic>(c"Swoole\\Atomic", ATOMIC_METHODS.rows(), 0);
    minit.register_object::<Long>(c"Swoole\\Atomic\\Long", LONG_METHODS.rows(), 0);
    Ok(())
}

fn construct(call: &Call) {
    word::construct::<Atomic>(call);
}

/// `Atomic\Long::__construct` returns true.
fn construct_long(call: &Call) {
    if word::construct::<Long>(call) {
        call.set_bool(true);
    }
}

swoole_rs_zend::handlers! {
    leaf:
    zim_swoole_atomic___construct => construct;
    zim_swoole_atomic_add => word::add::<Atomic>;
    zim_swoole_atomic_sub => word::sub::<Atomic>;
    zim_swoole_atomic_get => word::get::<Atomic>;
    zim_swoole_atomic_set => word::set::<Atomic>;
    zim_swoole_atomic_cmpset => word::cmpset::<Atomic>;
    zim_swoole_atomic_wait => word::wait::<Atomic>;
    zim_swoole_atomic_wakeup => word::wakeup::<Atomic>;
    zim_swoole_atomic_long___construct => construct_long;
    zim_swoole_atomic_long_add => word::add::<Long>;
    zim_swoole_atomic_long_sub => word::sub::<Long>;
    zim_swoole_atomic_long_get => word::get::<Long>;
    zim_swoole_atomic_long_set => word::set::<Long>;
    zim_swoole_atomic_long_cmpset => word::cmpset::<Long>;
}
