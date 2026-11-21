//! `Swoole\Thread\Barrier`: a reusable barrier every thread holding the object
//! (or a container value referring to it) waits on.
use crate::{
    host::{self, handlers},
    item::{IS_BARRIER, Item, Value},
};
use std::{
    cell::OnceCell,
    sync::{Arc, Barrier},
};
use swoole_rs_phpstubs::thread_barrier;
use swoole_rs_zend::{
    Call, Custom, CustomClass, Minit, Owned, Value as ZValue, new_object, sys::ZEND_ACC_FINAL,
};
use swoole_rs_zend::{method_table, sys as z};

/// A Barrier object's barrier, unset until `__construct` (or when read back
/// from an unconstructed one).
pub(crate) struct BarrierObject(OnceCell<Arc<Barrier>>);

static BARRIER: CustomClass<BarrierObject> = CustomClass::new();

impl Custom for BarrierObject {
    fn class() -> &'static CustomClass<Self> {
        &BARRIER
    }
    fn new() -> Self {
        Self(OnceCell::new())
    }
}

/// `SW_ERROR_INVALID_PARAMS`.
const INVALID_PARAMS: i64 = 504;

method_table! {
    static METHODS = [
        c"__construct" => zim_swoole_thread_barrier___construct, &thread_barrier::arginfo_class_Swoole_Thread_Barrier___construct, z::ZEND_ACC_PUBLIC;
        c"wait" => zim_swoole_thread_barrier_wait, &thread_barrier::arginfo_class_Swoole_Thread_Barrier_wait, z::ZEND_ACC_PUBLIC;
    ];
}

/// `php_swoole_thread_barrier_minit`.
pub fn barrier_minit(minit: &Minit) {
    minit.register_object::<BarrierObject>(c"Swoole\\Thread\\Barrier", &METHODS.0, ZEND_ACC_FINAL);
}

/// A Barrier stored in a container, by reference; `None` for other objects.
/// Upstream shares an unconstructed one's null resource; reading it back
/// gives an unconstructed Barrier.
pub(crate) fn shared(value: &ZValue) -> Option<Item> {
    let barrier = value.custom::<BarrierObject>()?;
    Some(Item {
        kind: IS_BARRIER,
        value: Value::Barrier(barrier.0.get().cloned()),
    })
}

/// `php_swoole_thread_barrier_create`: an object over `barrier`.
pub(crate) fn object(barrier: Option<&Arc<Barrier>>) -> Owned {
    new_object(BarrierObject(
        barrier.cloned().map(OnceCell::from).unwrap_or_default(),
    ))
}

/// `__construct(int $count)`: the constructed check comes before the
/// arguments, as upstream's does.
fn construct(call: &Call) {
    // The methods are non-static and the class final.
    let Some(BarrierObject(cell)) = call.custom() else {
        return;
    };
    if cell.get().is_some() {
        call.throw_constructed_twice();
        return;
    }
    let Ok(count) = call.arity(1, 1).and_then(|()| call.long(1, 0)) else {
        return;
    };
    if count < 2 {
        call.throw(
            swoole_rs_phpswoole::exception(),
            c"The parameter $count must be greater than 1",
            INVALID_PARAMS,
        );
        return;
    }
    let count = usize::try_from(count).unwrap_or(usize::MAX);
    _ = cell.set(Arc::new(Barrier::new(count)));
}

fn wait(call: &Call) {
    let Some(BarrierObject(cell)) = call.custom() else {
        return;
    };
    let Some(barrier) = cell.get() else {
        // Swoole's fatal error, which exits; this frame owns nothing.
        host::unconstructed(true);
        return;
    };
    barrier.wait();
}

handlers! {
    zim_swoole_thread_barrier___construct => construct;
    zim_swoole_thread_barrier_wait => wait;
}
