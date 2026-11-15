//! The PHP classes: `Swoole\Thread\Map`, `Swoole\Thread\ArrayList` and
//! `Swoole\Thread\Queue`. Each object holds one reference to a shared table or
//! queue (null until `__construct`, or when made by reading a value).
use crate::{
    array::{Array, list_index, negate},
    host::{self, handlers},
    item::{Item, Kind, Value},
    queue::Queue,
    table::Key,
};
use std::{cell::OnceCell, sync::Arc};
use swoole_rs_corebase::CCast;
use swoole_rs_phpstubs::thread_arraylist;
use swoole_rs_phpstubs::thread_map;
use swoole_rs_phpstubs::thread_queue;
use swoole_rs_zend::{
    Call, Custom, CustomClass, Invalid, Minit, Owned, Value as ZValue, bailout, new_object,
    sys::{
        IS_DOUBLE, IS_FALSE, IS_LONG, IS_NULL, IS_RESOURCE, IS_STRING, IS_TRUE, ZEND_ACC_FINAL,
        ZEND_ACC_PUBLIC, ZEND_ACC_READONLY, swr_zend_dval_to_lval_safe, zend_long, zend_ulong,
        zend_use_resource_as_offset,
    },
};

unsafe extern "C" {
    /// `_zend_handle_numeric_str`: whether a live string is a canonical integer,
    /// which `index` then holds. It reads the string only.
    safe fn swr_zend_handle_numeric_str(key: &swoole_rs_zend::ZStr, index: &mut zend_ulong)
    -> bool;
    /// `Z_RES_HANDLE_P`: the handle of the resource a live zval holds, read
    /// only; 0 for any other value.
    safe fn swr_zend_resource_handle(value: &ZValue) -> zend_long;
}
use swoole_rs_zend::{method_table, sys as z};

/// An object's reference to its shared table or queue, unset until
/// `__construct` (or when made by reading a value back).
trait Shared: Custom {
    type Target;
    fn cell(&self) -> &OnceCell<Arc<Self::Target>>;
    fn around(shared: &Arc<Self::Target>) -> Self;
}

macro_rules! shared {
    ($($name:ident($target:ty): $class:ident;)*) => {$(
        pub(crate) struct $name(OnceCell<Arc<$target>>);
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
            type Target = $target;
            fn cell(&self) -> &OnceCell<Arc<$target>> {
                &self.0
            }
            fn around(shared: &Arc<$target>) -> Self {
                Self(OnceCell::from(Arc::clone(shared)))
            }
        }
    )*};
}

shared! {
    Map(Array): MAP;
    List(Array): LIST;
    QueueObject(Queue): QUEUE;
}

method_table! {
    static MAP_METHODS = [
        c"__construct" => zim_swoole_thread_map___construct, &thread_map::arginfo_class_Swoole_Thread_Map___construct, z::ZEND_ACC_PUBLIC;
        c"offsetGet" => zim_swoole_thread_map_offsetGet, &thread_map::arginfo_class_Swoole_Thread_Map_offsetGet, z::ZEND_ACC_PUBLIC;
        c"offsetExists" => zim_swoole_thread_map_offsetExists, &thread_map::arginfo_class_Swoole_Thread_Map_offsetExists, z::ZEND_ACC_PUBLIC;
        c"offsetSet" => zim_swoole_thread_map_offsetSet, &thread_map::arginfo_class_Swoole_Thread_Map_offsetSet, z::ZEND_ACC_PUBLIC;
        c"offsetUnset" => zim_swoole_thread_map_offsetUnset, &thread_map::arginfo_class_Swoole_Thread_Map_offsetUnset, z::ZEND_ACC_PUBLIC;
        c"find" => zim_swoole_thread_map_find, &thread_map::arginfo_class_Swoole_Thread_Map_find, z::ZEND_ACC_PUBLIC;
        c"count" => zim_swoole_thread_map_count, &thread_map::arginfo_class_Swoole_Thread_Map_count, z::ZEND_ACC_PUBLIC;
        c"incr" => zim_swoole_thread_map_incr, &thread_map::arginfo_class_Swoole_Thread_Map_incr, z::ZEND_ACC_PUBLIC;
        c"decr" => zim_swoole_thread_map_decr, &thread_map::arginfo_class_Swoole_Thread_Map_decr, z::ZEND_ACC_PUBLIC;
        c"add" => zim_swoole_thread_map_add, &thread_map::arginfo_class_Swoole_Thread_Map_add, z::ZEND_ACC_PUBLIC;
        c"update" => zim_swoole_thread_map_update, &thread_map::arginfo_class_Swoole_Thread_Map_update, z::ZEND_ACC_PUBLIC;
        c"clean" => zim_swoole_thread_map_clean, &thread_map::arginfo_class_Swoole_Thread_Map_clean, z::ZEND_ACC_PUBLIC;
        c"keys" => zim_swoole_thread_map_keys, &thread_map::arginfo_class_Swoole_Thread_Map_keys, z::ZEND_ACC_PUBLIC;
        c"values" => zim_swoole_thread_map_values, &thread_map::arginfo_class_Swoole_Thread_Map_values, z::ZEND_ACC_PUBLIC;
        c"toArray" => zim_swoole_thread_map_toArray, &thread_map::arginfo_class_Swoole_Thread_Map_toArray, z::ZEND_ACC_PUBLIC;
        c"sort" => zim_swoole_thread_map_sort, &thread_map::arginfo_class_Swoole_Thread_Map_sort, z::ZEND_ACC_PUBLIC;
    ];
}

/// `php_swoole_thread_map_minit`: final, `ArrayAccess` and `Countable`.
pub fn map_minit(minit: &Minit) {
    minit.register_object::<Map>(c"Swoole\\Thread\\Map", MAP_METHODS.table(), ZEND_ACC_FINAL);
    minit.implements(&MAP, minit.array_access());
    minit.implements(&MAP, minit.countable());
}

method_table! {
    static ARRAYLIST_METHODS = [
        c"__construct" => zim_swoole_thread_arraylist___construct, &thread_arraylist::arginfo_class_Swoole_Thread_ArrayList___construct, z::ZEND_ACC_PUBLIC;
        c"offsetGet" => zim_swoole_thread_arraylist_offsetGet, &thread_arraylist::arginfo_class_Swoole_Thread_ArrayList_offsetGet, z::ZEND_ACC_PUBLIC;
        c"offsetExists" => zim_swoole_thread_arraylist_offsetExists, &thread_arraylist::arginfo_class_Swoole_Thread_ArrayList_offsetExists, z::ZEND_ACC_PUBLIC;
        c"offsetSet" => zim_swoole_thread_arraylist_offsetSet, &thread_arraylist::arginfo_class_Swoole_Thread_ArrayList_offsetSet, z::ZEND_ACC_PUBLIC;
        c"offsetUnset" => zim_swoole_thread_arraylist_offsetUnset, &thread_arraylist::arginfo_class_Swoole_Thread_ArrayList_offsetUnset, z::ZEND_ACC_PUBLIC;
        c"find" => zim_swoole_thread_arraylist_find, &thread_arraylist::arginfo_class_Swoole_Thread_ArrayList_find, z::ZEND_ACC_PUBLIC;
        c"incr" => zim_swoole_thread_arraylist_incr, &thread_arraylist::arginfo_class_Swoole_Thread_ArrayList_incr, z::ZEND_ACC_PUBLIC;
        c"decr" => zim_swoole_thread_arraylist_decr, &thread_arraylist::arginfo_class_Swoole_Thread_ArrayList_decr, z::ZEND_ACC_PUBLIC;
        c"clean" => zim_swoole_thread_arraylist_clean, &thread_arraylist::arginfo_class_Swoole_Thread_ArrayList_clean, z::ZEND_ACC_PUBLIC;
        c"count" => zim_swoole_thread_arraylist_count, &thread_arraylist::arginfo_class_Swoole_Thread_ArrayList_count, z::ZEND_ACC_PUBLIC;
        c"toArray" => zim_swoole_thread_arraylist_toArray, &thread_arraylist::arginfo_class_Swoole_Thread_ArrayList_toArray, z::ZEND_ACC_PUBLIC;
        c"sort" => zim_swoole_thread_arraylist_sort, &thread_arraylist::arginfo_class_Swoole_Thread_ArrayList_sort, z::ZEND_ACC_PUBLIC;
    ];
}

/// `php_swoole_thread_arraylist_minit`.
pub fn arraylist_minit(minit: &Minit) {
    minit.register_object::<List>(
        c"Swoole\\Thread\\ArrayList",
        ARRAYLIST_METHODS.table(),
        ZEND_ACC_FINAL,
    );
    minit.implements(&LIST, minit.array_access());
    minit.implements(&LIST, minit.countable());
    minit.property_long(&LIST, c"id", 0, ZEND_ACC_PUBLIC | ZEND_ACC_READONLY);
}

method_table! {
    static QUEUE_METHODS = [
        c"__construct" => zim_swoole_thread_queue___construct, &thread_queue::arginfo_class_Swoole_Thread_Queue___construct, z::ZEND_ACC_PUBLIC;
        c"push" => zim_swoole_thread_queue_push, &thread_queue::arginfo_class_Swoole_Thread_Queue_push, z::ZEND_ACC_PUBLIC;
        c"pop" => zim_swoole_thread_queue_pop, &thread_queue::arginfo_class_Swoole_Thread_Queue_pop, z::ZEND_ACC_PUBLIC;
        c"clean" => zim_swoole_thread_queue_clean, &thread_queue::arginfo_class_Swoole_Thread_Queue_clean, z::ZEND_ACC_PUBLIC;
        c"count" => zim_swoole_thread_queue_count, &thread_queue::arginfo_class_Swoole_Thread_Queue_count, z::ZEND_ACC_PUBLIC;
    ];
}

/// `php_swoole_thread_queue_minit`.
pub fn queue_minit(minit: &Minit) {
    minit.register_object::<QueueObject>(
        c"Swoole\\Thread\\Queue",
        QUEUE_METHODS.table(),
        ZEND_ACC_FINAL,
    );
    minit.implements(&QUEUE, minit.countable());
    minit.class_constant_long(&QUEUE, c"NOTIFY_ONE", NOTIFY_ONE);
    minit.class_constant_long(&QUEUE, c"NOTIFY_ALL", NOTIFY_ALL);
}

const NOTIFY_ONE: i64 = 1;
const NOTIFY_ALL: i64 = 2;

/// Store a Map, `ArrayList` or Queue by reference (`CAST_OBJ_TO_RESOURCE`).
/// An unconstructed one ends the request with upstream's fatal error.
pub(crate) fn shared(value: &ZValue) -> Option<Item> {
    let (kind, list, shared) = if let Some(map) = value.custom::<Map>() {
        (
            Kind::Map,
            false,
            map.0.get().map(|array| Value::Array(Arc::clone(array))),
        )
    } else if let Some(list) = value.custom::<List>() {
        (
            Kind::ArrayList,
            true,
            list.0.get().map(|array| Value::Array(Arc::clone(array))),
        )
    } else {
        let queue = value.custom::<QueueObject>()?;
        (
            Kind::Queue,
            false,
            queue.0.get().map(|queue| Value::Queue(Arc::clone(queue))),
        )
    };
    let Some(value) = shared else {
        crate::host::defer(crate::host::Pending::Unconstructed { list });
        return Some(Item {
            kind: Kind::Undef,
            value: Value::Bare(0),
        });
    };
    Some(Item { kind, value })
}

/// `php_swoole_thread_{arraylist,map}_create`: an object over `array`.
pub(crate) fn array_object(kind: Kind, array: &Arc<Array>) -> Owned {
    if kind == Kind::ArrayList {
        new_object(List::around(array))
    } else {
        new_object(Map::around(array))
    }
}

/// `php_swoole_thread_queue_create`.
pub(crate) fn queue_object(queue: &Arc<Queue>) -> Owned {
    new_object(QueueObject::around(queue))
}

/// The object's reference cell: the methods are non-static and the classes
/// final, so `$this` is one of `T`'s objects.
fn cell<T: Shared>(call: &Call) -> Option<&OnceCell<Arc<T::Target>>> {
    call.custom::<T>().map(Shared::cell)
}

/// The object's table or queue, or the fatal error upstream raises
/// (`*_fetch_object_check`); nothing is held when it bails out.
fn constructed<T: Shared>(call: &Call, list: bool) -> Option<&T::Target> {
    let shared = cell::<T>(call)?.get();
    if shared.is_none() {
        // The fatal error; this frame owns nothing.
        host::unconstructed(list);
    }
    shared.map(|shared| &**shared)
}

/// `Z_PARAM_ARRAY_HT_OR_NULL` of optional argument 1.
fn array_or_null(call: &Call) -> Result<Option<Owned>, Invalid> {
    if call.value(1).is_none() {
        return Ok(None);
    }
    call.array_or_null(1)
}

/// `__construct(?array $array = null)` of Map and `ArrayList`.
fn construct_array<T: Shared<Target = Array>>(call: &Call, list: bool) {
    let Ok(array) = call.arity(0, 1).and_then(|()| array_or_null(call)) else {
        return;
    };
    let Some(cell) = cell::<T>(call) else {
        return;
    };
    if cell.get().is_some() {
        call.throw_constructed_twice();
        return;
    }
    let shared = match array.as_ref().and_then(|array| array.array()) {
        Some(array) if list && !array.is_list() => {
            call.throw_error(c"the parameter $array must be an array of type list");
            return;
        }
        Some(array) => Array::from(array),
        None => Arc::default(),
    };
    if bailout::pending() {
        return;
    }
    _ = cell.set(shared);
}

// Map.

/// `handle_array_key` and `ZEND_ARRAY_CALL_METHOD`'s dispatch: the key, or
/// `None` after Zend's errors.
fn map_key(call: &Call, key: &swoole_rs_zend::Value) -> Option<Key> {
    match key.kind() {
        IS_STRING => {
            let text = key.str()?;
            let mut index: zend_ulong = 0;
            if swr_zend_handle_numeric_str(text, &mut index) {
                Some(Key::Int(index.c_cast::<i64>()))
            } else {
                Some(Key::Str(text.bytes().into()))
            }
        }
        IS_LONG => key.as_long().map(Key::Int),
        IS_NULL => Some(Key::Str(Box::default())),
        IS_DOUBLE => {
            let number = key.as_double()?;
            // SAFETY: a conversion on a PHP thread, which may raise `E_DEPRECATED`
            // and so run the user's error handler.
            Some(Key::Int(unsafe { swr_zend_dval_to_lval_safe(number) }))
        }
        IS_FALSE => Some(Key::Int(0)),
        IS_TRUE => Some(Key::Int(1)),
        IS_RESOURCE => {
            // SAFETY: a resource zval; Zend's warning.
            unsafe { zend_use_resource_as_offset(key.as_ptr()) };
            Some(Key::Int(swr_zend_resource_handle(key)))
        }
        _ => {
            // Both of upstream's errors, in its order.
            call.argument_type_error(1, c"Illegal offset type");
            swoole_rs_zend::type_error(c"Illegal offset type");
            None
        }
    }
}

/// Parse `(mixed $key[, ...])`, check the object, and resolve the key.
fn map_call(call: &Call, min: u32, max: u32) -> Option<(&Array, Key)> {
    call.arity(min, max).ok()?;
    let map = constructed::<Map>(call, false)?;
    let key = map_key(call, call.value(1)?)?;
    Some((map, key))
}

/// `INIT_ARRAY_INCR_PARAMS`' `$value`, 1 by default, negated
/// (`INIT_DECR_VALUE`) for a decrement, for `step`.
fn delta(call: &Call, decrement: bool, step: impl FnOnce(&ZValue)) {
    let one = swoole_rs_zend::Owned::long(1);
    let given = call.value(2).unwrap_or(&one);
    if !decrement {
        return step(given);
    }
    step(&negate(given));
}

fn map_construct(call: &Call) {
    construct_array::<Map>(call, false);
}

fn map_get(call: &Call) {
    if let Some(value) = map_call(call, 1, 1).and_then(|(map, key)| map.get(&key)) {
        call.set_owned(value);
    }
}

fn map_exists(call: &Call) {
    if let Some((map, key)) = map_call(call, 1, 1) {
        call.set_bool(map.exists(&key));
    }
}

fn map_set(call: &Call) {
    if let Some(((map, key), value)) = map_call(call, 2, 2).zip(call.value(2)) {
        map.set(key, value);
    }
}

fn map_step(call: &Call, decrement: bool) {
    if let Some((map, key)) = map_call(call, 1, 2) {
        delta(call, decrement, |delta| {
            map.incr(key, delta, call);
        });
    }
}

fn map_incr(call: &Call) {
    map_step(call, false);
}

fn map_decr(call: &Call) {
    map_step(call, true);
}

fn map_add(call: &Call) {
    if let Some(((map, key), value)) = map_call(call, 2, 2).zip(call.value(2)) {
        call.set_bool(map.add(key, value));
    }
}

fn map_update(call: &Call) {
    if let Some(((map, key), value)) = map_call(call, 2, 2).zip(call.value(2)) {
        call.set_bool(map.update(key, value));
    }
}

fn map_unset(call: &Call) {
    if let Some((map, key)) = map_call(call, 1, 1) {
        map.unset(&key);
    }
}

fn map_find(call: &Call) {
    if call.arity(1, 1).is_err() {
        return;
    }
    let found = constructed::<Map>(call, false)
        .zip(call.value(1))
        .and_then(|(map, search)| map.find(search));
    if let Some(found) = found {
        call.set_owned(found);
    }
}

fn map_count(call: &Call) {
    if let Some(map) = constructed::<Map>(call, false) {
        call.set_long(i64::try_from(map.count()).unwrap_or(i64::MAX));
    }
}

fn map_keys(call: &Call) {
    if let Some(map) = constructed::<Map>(call, false) {
        call.set_owned(map.keys());
    }
}

fn map_values(call: &Call) {
    if let Some(map) = constructed::<Map>(call, false) {
        call.set_owned(map.values());
    }
}

fn map_to_array(call: &Call) {
    if let Some(map) = constructed::<Map>(call, false) {
        call.set_owned(map.to_array());
    }
}

fn map_clean(call: &Call) {
    if let Some(map) = constructed::<Map>(call, false) {
        map.clean();
    }
}

fn map_sort(call: &Call) {
    if let Some(map) = constructed::<Map>(call, false) {
        map.sort(false);
    }
}

handlers! {
    zim_swoole_thread_map___construct => map_construct;
    zim_swoole_thread_map_offsetGet => map_get;
    zim_swoole_thread_map_offsetExists => map_exists;
    zim_swoole_thread_map_offsetSet => map_set;
    zim_swoole_thread_map_offsetUnset => map_unset;
    zim_swoole_thread_map_find => map_find;
    zim_swoole_thread_map_count => map_count;
    zim_swoole_thread_map_keys => map_keys;
    zim_swoole_thread_map_values => map_values;
    zim_swoole_thread_map_incr => map_incr;
    zim_swoole_thread_map_decr => map_decr;
    zim_swoole_thread_map_add => map_add;
    zim_swoole_thread_map_update => map_update;
    zim_swoole_thread_map_clean => map_clean;
    zim_swoole_thread_map_toArray => map_to_array;
    zim_swoole_thread_map_sort => map_sort;
}

// ArrayList.

/// `throw new Swoole\Exception("out of range", -1)`.
fn out_of_range(call: &Call) {
    call.throw(swoole_rs_phpswoole::exception(), c"out of range", -1);
}

/// Parse `(int $index)` and check the object.
fn list_long(call: &Call) -> Option<(&Array, i64)> {
    let index = call.arity(1, 1).and_then(|()| call.long(1, 0)).ok()?;
    Some((constructed::<List>(call, true)?, index))
}

fn list_construct(call: &Call) {
    construct_array::<List>(call, true);
}

fn list_get(call: &Call) {
    if let Some((list, index)) = list_long(call)
        && !list.index_get(index, call)
    {
        out_of_range(call);
    }
}

fn list_exists(call: &Call) {
    if let Some((list, index)) = list_long(call) {
        call.set_bool(list.index_exists(index));
    }
}

fn list_set(call: &Call) {
    if call.arity(2, 2).is_err() {
        return;
    }
    let Some(list) = constructed::<List>(call, true) else {
        return;
    };
    let Some((index, value)) = call.value(1).zip(call.value(2)) else {
        return;
    };
    if !list.index_set(list_index(index), value) {
        out_of_range(call);
    }
}

fn list_step(call: &Call, decrement: bool) {
    if call.arity(1, 2).is_err() {
        return;
    }
    let Some((list, key)) = constructed::<List>(call, true).zip(call.value(1)) else {
        return;
    };
    delta(call, decrement, |delta| {
        if !list.index_incr(list_index(key), delta, call) {
            out_of_range(call);
        }
    });
}

fn list_incr(call: &Call) {
    list_step(call, false);
}

fn list_decr(call: &Call) {
    list_step(call, true);
}

fn list_unset(call: &Call) {
    if let Some((list, index)) = list_long(call) {
        list.index_unset(index);
    }
}

fn list_find(call: &Call) {
    if call.arity(1, 1).is_err() {
        return;
    }
    let found = constructed::<List>(call, true)
        .zip(call.value(1))
        .and_then(|(list, search)| list.find(search));
    if let Some(found) = found {
        call.set_owned(found);
    }
}

fn list_count(call: &Call) {
    if let Some(list) = constructed::<List>(call, true) {
        call.set_long(i64::try_from(list.count()).unwrap_or(i64::MAX));
    }
}

fn list_clean(call: &Call) {
    if let Some(list) = constructed::<List>(call, true) {
        list.clean();
    }
}

fn list_to_array(call: &Call) {
    if let Some(list) = constructed::<List>(call, true) {
        call.set_owned(list.to_array());
    }
}

fn list_sort(call: &Call) {
    if let Some(list) = constructed::<List>(call, true) {
        list.sort(true);
    }
}

handlers! {
    zim_swoole_thread_arraylist___construct => list_construct;
    zim_swoole_thread_arraylist_offsetGet => list_get;
    zim_swoole_thread_arraylist_offsetExists => list_exists;
    zim_swoole_thread_arraylist_offsetSet => list_set;
    zim_swoole_thread_arraylist_offsetUnset => list_unset;
    zim_swoole_thread_arraylist_find => list_find;
    zim_swoole_thread_arraylist_count => list_count;
    zim_swoole_thread_arraylist_incr => list_incr;
    zim_swoole_thread_arraylist_decr => list_decr;
    zim_swoole_thread_arraylist_clean => list_clean;
    zim_swoole_thread_arraylist_toArray => list_to_array;
    zim_swoole_thread_arraylist_sort => list_sort;
}

// Queue.

/// `SW_ERROR_NO_PAYLOAD`.
const NO_PAYLOAD: i32 = swoole_rs_error::ErrorCode::SW_ERROR_NO_PAYLOAD as i32;

fn queue_construct(call: &Call) {
    let Some(cell) = cell::<QueueObject>(call) else {
        return;
    };
    if cell.set(Arc::default()).is_err() {
        call.throw_constructed_twice();
    }
}

fn queue_push(call: &Call) {
    let Ok(notify) = call.arity(1, 2).and_then(|()| call.long(2, 0)) else {
        return;
    };
    let Some(queue) = constructed::<QueueObject>(call, false) else {
        return;
    };
    let Some(value) = call.value(1) else {
        return;
    };
    let item = Item::store(value);
    if bailout::pending() {
        return;
    }
    if notify > 0 {
        queue.push_notify(item, notify == NOTIFY_ALL);
    } else {
        queue.push(item);
    }
}

fn queue_pop(call: &Call) {
    let Ok(timeout) = call.arity(0, 1).and_then(|()| call.double(1, 0.0)) else {
        return;
    };
    let Some(queue) = constructed::<QueueObject>(call, false) else {
        return;
    };
    let item = if timeout == 0.0 {
        queue.pop()
    } else {
        let item = queue.pop_wait(timeout);
        if item.is_none() {
            // All woken, and another thread took the item: null.
            call.set_null();
            // Swoole's last error.
            swoole_rs_corebase::log::set_last_error(NO_PAYLOAD);
        }
        item
    };
    if let Some(value) = item.as_ref().and_then(Item::fetch) {
        call.set_owned(value);
    }
}

fn queue_count(call: &Call) {
    if let Some(queue) = constructed::<QueueObject>(call, false) {
        call.set_long(i64::try_from(queue.count()).unwrap_or(i64::MAX));
    }
}

fn queue_clean(call: &Call) {
    if let Some(queue) = constructed::<QueueObject>(call, false) {
        queue.clean();
    }
}

handlers! {
    zim_swoole_thread_queue___construct => queue_construct;
    zim_swoole_thread_queue_push => queue_push;
    zim_swoole_thread_queue_pop => queue_pop;
    zim_swoole_thread_queue_count => queue_count;
    zim_swoole_thread_queue_clean => queue_clean;
}
