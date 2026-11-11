//! The PHP classes: `Swoole\Thread\Map`, `Swoole\Thread\ArrayList` and
//! `Swoole\Thread\Queue`. Each object holds one reference to a shared table or
//! queue (null until `__construct`, or when made by reading a value).
use crate::{
    array::{Array, list_index, negate},
    host::{self, handlers},
    item::{IS_ARRAYLIST, IS_MAP, IS_QUEUE, Item, Value},
    queue::Queue,
    table::Key,
};
use std::{ffi::c_void, ptr, sync::Arc};
use swoole_rs_phpstubs::thread_arraylist;
use swoole_rs_phpstubs::thread_map;
use swoole_rs_phpstubs::thread_queue;
use swoole_rs_zend::{
    Call, Class, Invalid, Minit, Object, Value as ZValue, bailout, set_object,
    sys::{
        _zend_expected_type_Z_EXPECTED_ARRAY_OR_NULL, IS_DOUBLE, IS_FALSE, IS_LONG, IS_NULL,
        IS_RESOURCE, IS_STRING, IS_TRUE, ZEND_ACC_FINAL, ZEND_ACC_NOT_SERIALIZABLE,
        ZEND_ACC_PUBLIC, ZEND_ACC_READONLY, ZPP_ERROR_WRONG_ARG, swr_zend_dval_to_lval_safe,
        swr_zend_handle_numeric_str, swr_zend_resource_handle, zend_argument_type_error,
        zend_ce_arrayaccess, zend_ce_countable, zend_class_entry, zend_class_implements,
        zend_object, zend_object_handlers, zend_type_error, zend_ulong,
        zend_use_resource_as_offset, zend_wrong_parameter_error, zval,
    },
    unset_property_deny,
};
use swoole_rs_zend::{method_table, sys as z};

static MAP: Class = Class::new();
static LIST: Class = Class::new();
static QUEUE: Class = Class::new();

/// An object's reference (`Arc::into_raw`), or null.
type Shared<T> = Object<*const T>;

unsafe extern "C" fn create_map(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: Zend calls `create_object` with this class; null is valid zeroed data.
    unsafe { Shared::<Array>::create(&MAP, ce) }
}

unsafe extern "C" fn create_list(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: as `create_map`.
    unsafe { Shared::<Array>::create(&LIST, ce) }
}

unsafe extern "C" fn create_queue(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: as `create_map`.
    unsafe { Shared::<Queue>::create(&QUEUE, ce) }
}

unsafe extern "C" fn free_array(std: *mut zend_object) {
    // SAFETY: Zend frees a Map or ArrayList.
    unsafe { release::<Array>(std) };
}

unsafe extern "C" fn free_queue(std: *mut zend_object) {
    // SAFETY: Zend frees a Queue.
    unsafe { release::<Queue>(std) };
}

/// `del_ref` the object's reference, then the standard destructor.
unsafe fn release<T>(std: *mut zend_object) {
    // SAFETY: per the caller, one of our objects.
    let shared = std::mem::replace(unsafe { Shared::<T>::data(std) }, ptr::null());
    if !shared.is_null() {
        // SAFETY: the reference the object held.
        drop(unsafe { Arc::from_raw(shared) });
    }
    // SAFETY: per the caller.
    unsafe { Shared::<T>::dtor(std) };
}

/// Final and not serializable, clone and unset denied, custom object.
fn configure(
    create: unsafe extern "C" fn(*mut zend_class_entry) -> *mut zend_object,
    free: unsafe extern "C" fn(*mut zend_object),
) -> impl FnOnce(&mut zend_class_entry, &mut zend_object_handlers) {
    move |entry, handlers| {
        entry.ce_flags |= ZEND_ACC_FINAL | ZEND_ACC_NOT_SERIALIZABLE;
        handlers.clone_obj = None;
        handlers.unset_property = Some(unset_property_deny);
        Shared::<c_void>::install(entry, handlers, create, free);
    }
}

/// `zend_class_implements(ce, n, ...)`: `ArrayAccess` (unless only `Countable`)
/// and `Countable`.
/// # Safety
/// MINIT, after `register`.
unsafe fn implements(class: &Class, array_access: bool) {
    if array_access {
        // SAFETY: Zend's interface, set before any extension's MINIT.
        let interface = unsafe { zend_ce_arrayaccess };
        // SAFETY: per the caller.
        unsafe { zend_class_implements(class.entry(), 1, interface) };
    }
    // SAFETY: as above.
    let interface = unsafe { zend_ce_countable };
    // SAFETY: per the caller.
    unsafe { zend_class_implements(class.entry(), 1, interface) };
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

/// `php_swoole_thread_map_minit`.
pub fn map_minit(minit: &Minit) {
    minit.register(
        &MAP,
        c"Swoole\\Thread\\Map",
        &MAP_METHODS.0,
        configure(create_map, free_array),
    );
    // SAFETY: MINIT; Zend's own interfaces.
    unsafe { implements(&MAP, true) };
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
    minit.register(
        &LIST,
        c"Swoole\\Thread\\ArrayList",
        &ARRAYLIST_METHODS.0,
        configure(create_list, free_array),
    );
    // SAFETY: MINIT; Zend's own interfaces.
    unsafe { implements(&LIST, true) };
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
    minit.register(
        &QUEUE,
        c"Swoole\\Thread\\Queue",
        &QUEUE_METHODS.0,
        configure(create_queue, free_queue),
    );
    // SAFETY: MINIT; Zend's own interface.
    unsafe { implements(&QUEUE, false) };
    minit.class_constant_long(&QUEUE, c"NOTIFY_ONE", NOTIFY_ONE);
    minit.class_constant_long(&QUEUE, c"NOTIFY_ALL", NOTIFY_ALL);
}

const NOTIFY_ONE: i64 = 1;
const NOTIFY_ALL: i64 = 2;

/// Store a Map, `ArrayList` or Queue by reference (`CAST_OBJ_TO_RESOURCE`).
/// An unconstructed one ends the request with upstream's fatal error.
pub(crate) fn shared(value: &ZValue) -> Option<Item> {
    let (std, ce) = (value.object()?, value.class()?);
    let (kind, list) = if ce == MAP.entry() {
        (IS_MAP, false)
    } else if ce == LIST.entry() {
        (IS_ARRAYLIST, true)
    } else if ce == QUEUE.entry() {
        (IS_QUEUE, false)
    } else {
        return None;
    };
    let value = if kind == IS_QUEUE {
        // SAFETY: one of our Queues.
        let queue = unsafe { *Shared::<Queue>::data(std) };
        // SAFETY: the object's live reference, if any.
        (!queue.is_null()).then(|| Value::Queue(unsafe { clone(queue) }))
    } else {
        // SAFETY: one of our Maps or ArrayLists.
        let array = unsafe { *Shared::<Array>::data(std) };
        // SAFETY: as above.
        (!array.is_null()).then(|| Value::Array(unsafe { clone(array) }))
    };
    let Some(value) = value else {
        crate::host::defer(crate::host::Pending::Unconstructed { list });
        return Some(Item {
            kind: swoole_rs_zend::sys::IS_UNDEF,
            value: Value::Bare(0),
        });
    };
    Some(Item { kind, value })
}

/// A new reference from an object's.
/// # Safety
/// `shared` is a live reference.
unsafe fn clone<T>(shared: *const T) -> Arc<T> {
    // SAFETY: per the caller.
    unsafe { Arc::increment_strong_count(shared) };
    // SAFETY: the reference just added.
    unsafe { Arc::from_raw(shared) }
}

/// `php_swoole_thread_{arraylist,map}_create`: an object over `array`.
/// # Safety
/// In a request; `out` is writable and holds nothing.
pub(crate) unsafe fn array_object(out: *mut zval, kind: u32, array: &Arc<Array>) {
    let class = if kind == IS_ARRAYLIST { &LIST } else { &MAP };
    // SAFETY: after MINIT; the class's own entry.
    let std = unsafe { Shared::<Array>::create(class, class.entry()) };
    // SAFETY: the object just created.
    unsafe { *Shared::<Array>::data(std) = Arc::into_raw(Arc::clone(array)) };
    // SAFETY: per the caller.
    unsafe { set_object(out, std) };
}

/// `php_swoole_thread_queue_create`.
/// # Safety
/// As `array_object`.
pub(crate) unsafe fn queue_object(out: *mut zval, queue: &Arc<Queue>) {
    // SAFETY: after MINIT; the class's own entry.
    let std = unsafe { Shared::<Queue>::create(&QUEUE, QUEUE.entry()) };
    // SAFETY: the object just created.
    unsafe { *Shared::<Queue>::data(std) = Arc::into_raw(Arc::clone(queue)) };
    // SAFETY: per the caller.
    unsafe { set_object(out, std) };
}

/// The object's reference slot.
#[expect(
    clippy::mut_from_ref,
    reason = "the slot is in the Zend object, not the call"
)]
fn slot<T>(call: &Call) -> &mut *const T {
    // SAFETY: the methods are non-static and the classes final, so `$this` is
    // one of our objects of this kind.
    unsafe { Shared::<T>::data(call.this()) }
}

/// The object's table or queue, or the fatal error upstream raises
/// (`*_fetch_object_check`); nothing is held when it bails out.
fn constructed<T>(call: &Call, list: bool) -> Option<&'static T> {
    let shared = *slot::<T>(call);
    if shared.is_null() {
        // The fatal error; this frame owns nothing.
        host::unconstructed(list);
        return None;
    }
    // SAFETY: the object holds a reference for as long as it lives, which
    // outlasts the call.
    Some(unsafe { &*shared })
}

/// `Z_PARAM_ARRAY_HT_OR_NULL` of optional argument 1.
fn array_or_null(call: &Call) -> Result<Option<&swoole_rs_zend::Array>, Invalid> {
    let Some(arg) = call.value(1) else {
        return Ok(None);
    };
    match arg.array() {
        Some(array) => Ok(Some(array)),
        None if arg.is_null() => Ok(None),
        None => {
            #[expect(clippy::cast_possible_wrap, reason = "a small ZPP error code")]
            // SAFETY: in a handler, with the argument that failed.
            unsafe {
                zend_wrong_parameter_error(
                    ZPP_ERROR_WRONG_ARG as i32,
                    1,
                    ptr::null_mut(),
                    _zend_expected_type_Z_EXPECTED_ARRAY_OR_NULL,
                    arg.as_ptr(),
                );
            }
            Err(Invalid)
        }
    }
}

/// `__construct(?array $array = null)` of Map and `ArrayList`.
fn construct_array(call: &Call, list: bool) {
    let Ok(array) = call.arity(0, 1).and_then(|()| array_or_null(call)) else {
        return;
    };
    if !slot::<Array>(call).is_null() {
        call.throw_constructed_twice();
        return;
    }
    let shared = match array {
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
    *slot::<Array>(call) = Arc::into_raw(shared);
}

// Map.

/// `handle_array_key` and `ZEND_ARRAY_CALL_METHOD`'s dispatch: the key, or
/// `None` after Zend's errors.
fn map_key(key: &swoole_rs_zend::Value) -> Option<Key> {
    match key.kind() {
        IS_STRING => {
            let text = key.str()?.bytes();
            let mut index: zend_ulong = 0;
            // SAFETY: a live string and a writable local.
            if unsafe {
                swr_zend_handle_numeric_str(text.as_ptr().cast(), text.len(), &raw mut index)
            } {
                #[expect(
                    clippy::cast_possible_wrap,
                    reason = "Zend's numeric keys fit zend_long"
                )]
                Some(Key::Int(index as i64))
            } else {
                Some(Key::Str(text.into()))
            }
        }
        IS_LONG => key.as_long().map(Key::Int),
        IS_NULL => Some(Key::Str(Box::default())),
        IS_DOUBLE => {
            let number = key.as_double()?;
            // SAFETY: a pure conversion.
            Some(Key::Int(unsafe { swr_zend_dval_to_lval_safe(number) }))
        }
        IS_FALSE => Some(Key::Int(0)),
        IS_TRUE => Some(Key::Int(1)),
        IS_RESOURCE => {
            // SAFETY: a resource zval; Zend's warning.
            unsafe { zend_use_resource_as_offset(key.as_ptr()) };
            // SAFETY: as above.
            Some(Key::Int(unsafe { swr_zend_resource_handle(key.as_ptr()) }))
        }
        _ => {
            // SAFETY: in a handler; both of upstream's errors, in its order.
            unsafe { zend_argument_type_error(1, c"%s".as_ptr(), c"Illegal offset type".as_ptr()) };
            // SAFETY: as above.
            unsafe { zend_type_error(c"%s".as_ptr(), c"Illegal offset type".as_ptr()) };
            None
        }
    }
}

/// Parse `(mixed $key[, ...])`, check the object, and resolve the key.
fn map_call(call: &Call, min: u32, max: u32) -> Option<(&'static Array, Key)> {
    call.arity(min, max).ok()?;
    let map = constructed::<Array>(call, false)?;
    let key = map_key(call.value(1)?)?;
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
    let negated = negate(given);
    // SAFETY: a live local zval, borrowed only by `step`.
    if let Some(negated) = unsafe { ZValue::from_ptr(&raw const negated) } {
        step(negated);
    }
}

fn map_construct(call: &Call) {
    construct_array(call, false);
}

fn map_get(call: &Call) {
    if let Some((map, key)) = map_call(call, 1, 1) {
        // SAFETY: in a handler; the return slot.
        unsafe { map.get(&key, call.return_value()) };
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
    if let Some((map, search)) = constructed::<Array>(call, false).zip(call.value(1)) {
        // SAFETY: in a handler; the return slot.
        unsafe { map.find(search, call.return_value()) };
    }
}

fn map_count(call: &Call) {
    if let Some(map) = constructed::<Array>(call, false) {
        call.set_long(i64::try_from(map.count()).unwrap_or(i64::MAX));
    }
}

fn map_keys(call: &Call) {
    if let Some(map) = constructed::<Array>(call, false) {
        call.set_owned(map.keys().into_owned());
    }
}

fn map_values(call: &Call) {
    if let Some(map) = constructed::<Array>(call, false) {
        // SAFETY: in a handler; the return slot.
        unsafe { map.values(call.return_value()) };
    }
}

fn map_to_array(call: &Call) {
    if let Some(map) = constructed::<Array>(call, false) {
        // SAFETY: in a handler; the return slot.
        unsafe { map.to_array(call.return_value()) };
    }
}

fn map_clean(call: &Call) {
    if let Some(map) = constructed::<Array>(call, false) {
        map.clean();
    }
}

fn map_sort(call: &Call) {
    if let Some(map) = constructed::<Array>(call, false) {
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
fn list_long(call: &Call) -> Option<(&'static Array, i64)> {
    let index = call.arity(1, 1).and_then(|()| call.long(1, 0)).ok()?;
    Some((constructed::<Array>(call, true)?, index))
}

fn list_construct(call: &Call) {
    construct_array(call, true);
}

fn list_get(call: &Call) {
    if let Some((list, index)) = list_long(call) {
        // SAFETY: in a handler; the return slot.
        if !unsafe { list.index_get(index, call.return_value()) } {
            out_of_range(call);
        }
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
    let Some(list) = constructed::<Array>(call, true) else {
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
    let Some((list, key)) = constructed::<Array>(call, true).zip(call.value(1)) else {
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
    if let Some((list, search)) = constructed::<Array>(call, true).zip(call.value(1)) {
        // SAFETY: in a handler; the return slot.
        unsafe { list.find(search, call.return_value()) };
    }
}

fn list_count(call: &Call) {
    if let Some(list) = constructed::<Array>(call, true) {
        call.set_long(i64::try_from(list.count()).unwrap_or(i64::MAX));
    }
}

fn list_clean(call: &Call) {
    if let Some(list) = constructed::<Array>(call, true) {
        list.clean();
    }
}

fn list_to_array(call: &Call) {
    if let Some(list) = constructed::<Array>(call, true) {
        // SAFETY: in a handler; the return slot.
        unsafe { list.to_array(call.return_value()) };
    }
}

fn list_sort(call: &Call) {
    if let Some(list) = constructed::<Array>(call, true) {
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
const NO_PAYLOAD: i32 = 550;

fn queue_construct(call: &Call) {
    let shared = slot::<Queue>(call);
    if !shared.is_null() {
        call.throw_constructed_twice();
        return;
    }
    *shared = Arc::into_raw(Arc::default());
}

fn queue_push(call: &Call) {
    let Ok(notify) = call.arity(1, 2).and_then(|()| call.long(2, 0)) else {
        return;
    };
    let Some(queue) = constructed::<Queue>(call, false) else {
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
    let Some(queue) = constructed::<Queue>(call, false) else {
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
    if let Some(item) = item {
        // SAFETY: in a handler; the return slot.
        unsafe { item.fetch(call.return_value()) };
    }
}

fn queue_count(call: &Call) {
    if let Some(queue) = constructed::<Queue>(call, false) {
        call.set_long(i64::try_from(queue.count()).unwrap_or(i64::MAX));
    }
}

fn queue_clean(call: &Call) {
    if let Some(queue) = constructed::<Queue>(call, false) {
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
