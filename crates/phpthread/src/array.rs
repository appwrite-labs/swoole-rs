//! Upstream's `ZendArray`: a table of items behind a reader-writer lock,
//! shared by every Map or `ArrayList` object (in any thread) that refers to it.
use crate::{
    host::finish,
    item::{Item, set},
    table::{Key, Table},
};
use std::{
    ffi::c_void,
    sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard},
};
use swoole_rs_zend::{
    string::bytes,
    sys::{
        IS_DOUBLE, IS_LONG, IS_NULL, swr_zend_array_init, swr_zend_hash_foreach_kv,
        swr_zval_get_long, swr_zval_stringl, zend_array, zend_hash_index_add,
        zend_hash_next_index_insert_new, zend_hash_str_add, zend_sort, zend_string, zend_ulong,
        zend_value, zval,
    },
    type_of, value,
};

#[derive(Debug, Default)]
pub(crate) struct Array {
    table: RwLock<Table<Item>>,
}

impl Array {
    fn read(&self) -> RwLockReadGuard<'_, Table<Item>> {
        self.table.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> RwLockWriteGuard<'_, Table<Item>> {
        self.table.write().unwrap_or_else(PoisonError::into_inner)
    }

    /// `ZendArray::from`: every element of a PHP array, references followed.
    /// # Safety
    /// In a request, with a live table.
    pub(crate) unsafe fn from(source: *const zend_array) -> Arc<Self> {
        unsafe extern "C" fn each(
            key: *mut zend_string,
            index: zend_ulong,
            value: *mut zval,
            context: *mut c_void,
        ) {
            // SAFETY: the table `from` passed, live for the loop.
            let table = unsafe { &mut *context.cast::<Table<Item>>() };
            let key = if key.is_null() {
                #[expect(clippy::cast_possible_wrap, reason = "Zend's int keys are zend_long")]
                Key::Int(index as i64)
            } else {
                // SAFETY: a live key.
                Key::Str(unsafe { bytes(key) }.into())
            };
            // SAFETY: a live element.
            table.add(key, unsafe { Item::store(value) });
        }
        let mut table = Table::default();
        // SAFETY: per the caller; `table` outlives the loop.
        unsafe { swr_zend_hash_foreach_kv(source, Some(each), (&raw mut table).cast()) };
        Arc::new(Self {
            table: RwLock::new(table),
        })
    }

    /// `append` (upstream's unlocked builder for thread arguments).
    /// # Safety
    /// In a request, with a live zval.
    pub(crate) unsafe fn push(&self, value: *mut zval) {
        // SAFETY: per the caller.
        let item = unsafe { Item::store(value) };
        self.write().push(item);
    }

    pub(crate) fn clean(&self) {
        let removed = std::mem::take(&mut *self.write());
        drop(removed);
    }

    pub(crate) fn count(&self) -> usize {
        self.read().len()
    }

    /// `strkey_offsetGet` / `intkey_offsetGet`.
    /// # Safety
    /// In a request; `out` is the return slot.
    pub(crate) unsafe fn get(&self, key: &Key, out: *mut zval) {
        if let Some(item) = self.read().get(key) {
            // SAFETY: per the caller.
            unsafe { item.fetch(out) };
        }
    }

    pub(crate) fn exists(&self, key: &Key) -> bool {
        self.read().contains(key)
    }

    pub(crate) fn unset(&self, key: &Key) {
        let removed = self.write().remove(key);
        drop(removed);
    }

    /// `offsetSet`: the item is made before the lock is taken.
    /// # Safety
    /// In a request, with a live zval.
    pub(crate) unsafe fn set(&self, key: Key, value: *mut zval) {
        // SAFETY: per the caller.
        let item = unsafe { Item::store(value) };
        self.write().update(key, item);
    }

    /// `strkey_add` / `intkey_add`: false if the key exists.
    /// # Safety
    /// As `set`.
    pub(crate) unsafe fn add(&self, key: Key, value: *mut zval) -> bool {
        let mut table = self.write();
        if table.contains(&key) {
            return false;
        }
        // SAFETY: per the caller.
        table.add(key, unsafe { Item::store(value) })
    }

    /// `strkey_update` / `intkey_update`: false unless the key exists.
    /// # Safety
    /// As `set`.
    pub(crate) unsafe fn update(&self, key: Key, value: *mut zval) -> bool {
        let mut table = self.write();
        if !table.contains(&key) {
            return false;
        }
        // SAFETY: per the caller.
        table.update(key, unsafe { Item::store(value) });
        true
    }

    /// `strkey_incr` / `intkey_incr`. An absent int key stores `delta` itself,
    /// as upstream does; an absent string key stores the counter.
    /// # Safety
    /// In a request; `delta` is live and `out` the return slot.
    pub(crate) unsafe fn incr(&self, key: Key, delta: *mut zval, out: *mut zval) {
        let mut table = self.write();
        if let Some(item) = table.get_mut(&key) {
            // SAFETY: per the caller.
            unsafe { item.add(delta, out) };
            return;
        }
        // SAFETY: per the caller.
        let counter = unsafe { Item::counter(delta, out) };
        let item = match key {
            // SAFETY: per the caller.
            Key::Int(_) => unsafe { Item::store(delta) },
            Key::Str(_) => counter,
        };
        table.update(key, item);
    }

    /// `index_offsetGet`: false when `index` is not below the count.
    /// # Safety
    /// As `get`.
    pub(crate) unsafe fn index_get(&self, index: i64, out: *mut zval) -> bool {
        let table = self.read();
        if !below(index, table.len()) {
            return false;
        }
        if let Some(item) = table.get_int(index) {
            // SAFETY: per the caller.
            unsafe { item.fetch(out) };
        }
        true
    }

    pub(crate) fn index_exists(&self, index: i64) -> bool {
        below(index, self.read().len())
    }

    /// `index_offsetSet`: -1 or the count appends; past the count fails.
    /// # Safety
    /// As `set`.
    pub(crate) unsafe fn index_set(&self, index: i64, value: *mut zval) -> bool {
        // SAFETY: per the caller.
        let item = unsafe { Item::store(value) };
        let mut table = self.write();
        let count = length(&table);
        if index > count {
            drop(table);
            drop(item);
            return false;
        }
        if index == -1 || index == count {
            table.push(item);
        } else {
            table.update(Key::Int(index), item);
        }
        true
    }

    /// `index_incr`: like `index_offsetSet`, adding to the existing value.
    /// # Safety
    /// As `incr`.
    pub(crate) unsafe fn index_incr(&self, index: i64, delta: *mut zval, out: *mut zval) -> bool {
        let mut table = self.write();
        let count = length(&table);
        if index > count {
            return false;
        }
        if index == -1 || index == count {
            // SAFETY: per the caller.
            table.push(unsafe { Item::counter(delta, out) });
        } else if let Some(item) = table.get_mut(&Key::Int(index)) {
            // SAFETY: per the caller.
            unsafe { item.add(delta, out) };
        } else {
            // Upstream dereferences a missing item here (a negative index).
            // SAFETY: per the caller.
            table.update(Key::Int(index), unsafe { Item::counter(delta, out) });
        }
        true
    }

    /// `index_offsetUnset`.
    pub(crate) fn index_unset(&self, index: i64) {
        self.write().unset_shift(index);
    }

    /// `keys`.
    /// # Safety
    /// `out` is the return slot.
    pub(crate) unsafe fn keys(&self, out: *mut zval) {
        let table = self.read();
        // SAFETY: per the caller.
        unsafe { swr_zend_array_init(out) };
        // SAFETY: an array zval.
        let array = unsafe { value::table(out) };
        for (key, _) in table.iter() {
            let mut value = zval_null();
            match key {
                Key::Int(index) => set(&raw mut value, IS_LONG, zend_value { lval: *index }),
                // SAFETY: Zend copies the bytes.
                Key::Str(name) => unsafe {
                    swr_zval_stringl(&raw mut value, name.as_ptr().cast(), name.len());
                },
            }
            // SAFETY: an array zval; the array takes the value.
            unsafe { zend_hash_next_index_insert_new(array, &raw mut value) };
        }
    }

    /// `values`.
    /// # Safety
    /// In a request; `out` is the return slot.
    pub(crate) unsafe fn values(&self, out: *mut zval) {
        let table = self.read();
        // SAFETY: per the caller.
        unsafe { swr_zend_array_init(out) };
        // SAFETY: an array zval.
        let array = unsafe { value::table(out) };
        for (_, item) in table.iter() {
            let mut value = zval_null();
            // SAFETY: per the caller.
            unsafe { item.fetch_or_null(&raw mut value) };
            // SAFETY: an array zval; the array takes the value.
            unsafe { zend_hash_next_index_insert_new(array, &raw mut value) };
        }
    }

    /// `to_array`.
    /// # Safety
    /// In a request; `out` is writable and holds nothing.
    pub(crate) unsafe fn to_array(&self, out: *mut zval) {
        let table = self.read();
        // SAFETY: per the caller.
        unsafe { swr_zend_array_init(out) };
        // SAFETY: an array zval.
        let array = unsafe { value::table(out) };
        for (key, item) in table.iter() {
            let mut value = zval_null();
            // SAFETY: per the caller.
            unsafe { item.fetch_or_null(&raw mut value) };
            match key {
                #[expect(clippy::cast_sign_loss, reason = "Zend's int keys are zend_ulong")]
                // SAFETY: a live array; it takes the value.
                Key::Int(index) => unsafe {
                    zend_hash_index_add(array, *index as zend_ulong, &raw mut value)
                },
                // SAFETY: as above; Zend copies the key.
                Key::Str(name) => unsafe {
                    zend_hash_str_add(array, name.as_ptr().cast(), name.len(), &raw mut value)
                },
            };
        }
    }

    /// `find`: the key of the first item equal to `search`, or nothing.
    /// # Safety
    /// `search` is live; `out` is the return slot.
    pub(crate) unsafe fn find(&self, search: *const zval, out: *mut zval) {
        let table = self.read();
        // SAFETY: per the caller.
        let found = table
            .iter()
            .find(|(_, item)| unsafe { item.equals(search) });
        match found {
            Some((Key::Int(index), _)) => set(out, IS_LONG, zend_value { lval: *index }),
            // SAFETY: per the caller; Zend copies the bytes.
            Some((Key::Str(name), _)) => unsafe {
                swr_zval_stringl(out, name.as_ptr().cast(), name.len());
            },
            None => {}
        }
    }

    /// `sort`: `zend_hash_sort` with `ArrayItem::compare` (Zend's own hybrid
    /// insertion sort, so ties land where upstream's do).
    pub(crate) fn sort(&self, renumber: bool) {
        type Entry = (Key, Item);
        unsafe extern "C" fn compare(a: *const c_void, b: *const c_void) -> i32 {
            // SAFETY: zend_sort passes two elements of the slice.
            let a = unsafe { &*a.cast::<Entry>() };
            // SAFETY: as above.
            let b = unsafe { &*b.cast::<Entry>() };
            a.1.compare(&b.1)
        }
        unsafe extern "C" fn swap(a: *mut c_void, b: *mut c_void) {
            // SAFETY: zend_sort passes two distinct elements of the slice.
            unsafe { std::ptr::swap(a.cast::<Entry>(), b.cast::<Entry>()) };
        }
        self.write().sort(renumber, |entries| {
            // SAFETY: a live slice of `Entry`s; the callbacks match its layout.
            unsafe {
                zend_sort(
                    entries.as_mut_ptr().cast(),
                    entries.len(),
                    size_of::<Entry>(),
                    Some(compare),
                    Some(swap),
                );
            }
        });
    }
}

/// `index < (zend_long) zend_hash_num_elements(&ht)`.
fn below(index: i64, count: usize) -> bool {
    index < i64::try_from(count).unwrap_or(i64::MAX)
}

/// `zend_hash_num_elements` as the signed long upstream compares it with.
fn length(table: &Table<Item>) -> i64 {
    i64::try_from(table.len()).unwrap_or(i64::MAX)
}

fn zval_null() -> zval {
    // SAFETY: a zval is plain data; all-zero bytes are IS_UNDEF.
    let mut value: zval = unsafe { std::mem::zeroed() };
    set(&raw mut value, IS_NULL, zend_value { lval: 0 });
    value
}

/// `zval_get_long`, or -1 for null (`ZVAL_IS_NULL(zkey) ? -1 : zval_get_long(zkey)`).
/// # Safety
/// `key` is a live zval.
pub(crate) unsafe fn list_index(key: *const zval) -> i64 {
    // SAFETY: per the caller.
    if unsafe { type_of(key) } == IS_NULL {
        -1
    } else {
        // SAFETY: per the caller.
        unsafe { swr_zval_get_long(key) }
    }
}

/// `INIT_DECR_VALUE`: `delta` negated as a float, or as an int.
/// # Safety
/// `delta` is a live zval.
pub(crate) unsafe fn negate(delta: *const zval) -> zval {
    // SAFETY: per the caller; a copy of the zval's bits, as upstream's `*v`.
    let mut value = unsafe { *delta };
    // SAFETY: per the caller.
    if unsafe { type_of(delta) } == IS_DOUBLE {
        // SAFETY: a float zval.
        let negated = -unsafe { value.value.dval };
        set(&raw mut value, IS_DOUBLE, zend_value { dval: negated });
    } else {
        // SAFETY: per the caller.
        let negated = unsafe { swr_zval_get_long(delta) }.wrapping_neg();
        set(&raw mut value, IS_LONG, zend_value { lval: negated });
    }
    value
}

// Shared tables for Swoole\Thread's arguments (C++ holds them as `ZendArray *`).

pub(crate) fn handle(array: Arc<Array>) -> *mut c_void {
    Arc::into_raw(array).cast_mut().cast()
}

/// # Safety
/// `array` came from `handle` and is still held.
unsafe fn borrow<'a>(array: *mut c_void) -> &'a Array {
    // SAFETY: per the caller.
    unsafe { &*array.cast::<Array>() }
}

/// `new ZendArray()`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_phpthread_array_new() -> *mut c_void {
    handle(Arc::default())
}

/// `ZendArray::from(array)`.
/// # Safety
/// In a request, with a live array.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpthread_array_from(array: *mut zend_array) -> *mut c_void {
    // SAFETY: per the caller.
    let shared = handle(unsafe { Array::from(array) });
    finish();
    shared
}

/// `argv->append(value)`.
/// # Safety
/// `array` is a live handle; in a request, with a live zval.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpthread_array_append(array: *mut c_void, value: *mut zval) {
    // SAFETY: per the caller.
    let array = unsafe { borrow(array) };
    // SAFETY: per the caller.
    unsafe { array.push(value) };
    finish();
}

/// `argv->to_array(return_value)`.
/// # Safety
/// `array` is a live handle; in a request; `out` is writable and holds nothing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpthread_array_to_array(
    array: *mut c_void,
    return_value: *mut zval,
) {
    // SAFETY: per the caller.
    let array = unsafe { borrow(array) };
    // SAFETY: per the caller.
    unsafe { array.to_array(return_value) };
    finish();
}

/// `argv->add_ref()`: another holder of the handle.
/// # Safety
/// `array` is a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpthread_array_add_ref(array: *mut c_void) {
    // SAFETY: per the caller, a reference `handle` gave out.
    unsafe { Arc::increment_strong_count(array.cast_const().cast::<Array>()) };
}

/// `argv->del_ref()`.
/// # Safety
/// `array` is a live handle, released here.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpthread_array_release(array: *mut c_void) {
    // SAFETY: per the caller: the reference `handle` gave out.
    drop(unsafe { Arc::from_raw(array.cast_const().cast::<Array>()) });
}
