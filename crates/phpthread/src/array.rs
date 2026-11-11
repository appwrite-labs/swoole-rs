//! Upstream's `ZendArray`: a table of items behind a reader-writer lock,
//! shared by every Map or `ArrayList` object (in any thread) that refers to it.
use crate::{
    item::{Item, set},
    table::{Key, Table},
};
use std::{
    ffi::c_void,
    sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard},
};
use swoole_rs_zend::{
    Array as ZArray, Call, Key as ZKey, OwnedArray, Value as ZValue, bailout,
    sys::{
        IS_DOUBLE, IS_LONG, IS_NULL, swr_zend_array_init, swr_zval_stringl, zend_hash_index_add,
        zend_hash_next_index_insert_new, zend_hash_str_add, zend_sort, zend_ulong, zend_value,
        zval,
    },
    value,
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
    pub(crate) fn from(source: &ZArray) -> Arc<Self> {
        let mut table = Table::default();
        source.for_each(|key, value| {
            if bailout::pending() {
                return;
            }
            let key = match key {
                #[expect(clippy::cast_possible_wrap, reason = "Zend's int keys are zend_long")]
                ZKey::Index(index) => Key::Int(index as i64),
                ZKey::Name(name) => Key::Str(name.into()),
            };
            let item = Item::store(value);
            if !bailout::pending() {
                table.add(key, item);
            }
        });
        Arc::new(Self {
            table: RwLock::new(table),
        })
    }

    /// `append` (upstream's unlocked builder for thread arguments).
    pub(crate) fn push(&self, value: &ZValue) {
        let item = Item::store(value);
        if bailout::pending() {
            return;
        }
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
    pub(crate) fn set(&self, key: Key, value: &ZValue) {
        let item = Item::store(value);
        if bailout::pending() {
            return;
        }
        self.write().update(key, item);
    }

    /// `strkey_add` / `intkey_add`: false if the key exists.
    pub(crate) fn add(&self, key: Key, value: &ZValue) -> bool {
        let mut table = self.write();
        if table.contains(&key) {
            return false;
        }
        let item = Item::store(value);
        if bailout::pending() {
            return false;
        }
        table.add(key, item)
    }

    /// `strkey_update` / `intkey_update`: false unless the key exists.
    pub(crate) fn update(&self, key: Key, value: &ZValue) -> bool {
        let mut table = self.write();
        if !table.contains(&key) {
            return false;
        }
        let item = Item::store(value);
        if bailout::pending() {
            return false;
        }
        table.update(key, item);
        true
    }

    /// `strkey_incr` / `intkey_incr`, the result in the return slot of `call`. An
    /// absent int key stores `delta` itself, as upstream does; an absent string
    /// key stores the counter.
    pub(crate) fn incr(&self, key: Key, delta: &ZValue, call: &Call) {
        let mut table = self.write();
        if let Some(item) = table.get_mut(&key) {
            item.add(delta, call);
            return;
        }
        let counter = Item::counter(delta, call);
        if bailout::pending() {
            return;
        }
        let item = match key {
            Key::Int(_) => Item::store(delta),
            Key::Str(_) => counter,
        };
        if !bailout::pending() {
            table.update(key, item);
        }
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
    pub(crate) fn index_set(&self, index: i64, value: &ZValue) -> bool {
        let item = Item::store(value);
        if bailout::pending() {
            return false;
        }
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
    pub(crate) fn index_incr(&self, index: i64, delta: &ZValue, call: &Call) -> bool {
        let mut table = self.write();
        let count = length(&table);
        if index > count {
            return false;
        }
        if index == -1 || index == count {
            let item = Item::counter(delta, call);
            if bailout::pending() {
                return false;
            }
            table.push(item);
        } else if let Some(item) = table.get_mut(&Key::Int(index)) {
            item.add(delta, call);
        } else {
            // Upstream dereferences a missing item here (a negative index).
            let item = Item::counter(delta, call);
            if bailout::pending() {
                return false;
            }
            table.update(Key::Int(index), item);
        }
        true
    }

    /// `index_offsetUnset`.
    pub(crate) fn index_unset(&self, index: i64) {
        self.write().unset_shift(index);
    }

    /// `keys`.
    pub(crate) fn keys(&self) -> OwnedArray {
        let mut array = OwnedArray::default();
        for (key, _) in self.read().iter() {
            match key {
                Key::Int(index) => array.push_long(*index),
                Key::Str(name) => array.push_bytes(name),
            }
        }
        array
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
    /// `out` is the return slot.
    pub(crate) unsafe fn find(&self, search: &ZValue, out: *mut zval) {
        let table = self.read();
        let found = table.iter().find(|(_, item)| item.equals(search));
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
    let mut value = swoole_rs_zend::UNDEF;
    set(&raw mut value, IS_NULL, zend_value { lval: 0 });
    value
}

/// `zval_get_long`, or -1 for null (`ZVAL_IS_NULL(zkey) ? -1 : zval_get_long(zkey)`).
pub(crate) fn list_index(key: &ZValue) -> i64 {
    if key.is_null() { -1 } else { key.long() }
}

/// `INIT_DECR_VALUE`: `delta` negated as a float, or as an int.
pub(crate) fn negate(delta: &ZValue) -> zval {
    let mut value = zval_null();
    if let Some(double) = delta.as_double() {
        set(&raw mut value, IS_DOUBLE, zend_value { dval: -double });
    } else {
        let lval = delta.long().wrapping_neg();
        set(&raw mut value, IS_LONG, zend_value { lval });
    }
    value
}

/// A thread's arguments: a shared table each new thread copies into its
/// `Thread::getArguments()` (upstream's `ZendArray *`). Cloning takes another
/// reference.
#[derive(Clone, Debug)]
pub struct Arguments(pub(crate) Arc<Array>);

impl Arguments {
    /// `ZendArray::from(array)`.
    /// # Errors
    /// A bailout PHP code raised while copying, pending for the caller to raise.
    pub fn copy(array: &ZArray) -> Result<Self, bailout::Bailout> {
        let shared = Array::from(array);
        if bailout::pending() {
            return Err(bailout::Bailout);
        }
        Ok(Self(shared))
    }
}
