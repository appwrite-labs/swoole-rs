//! Upstream's `ZendArray`: a table of items behind a reader-writer lock,
//! shared by every Map or `ArrayList` object (in any thread) that refers to it.
use crate::{
    item::Item,
    table::{Key, Table},
};
use std::{
    ffi::c_void,
    sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard},
};
use swoole_rs_corebase::CCast;
use swoole_rs_zend::{
    Array as ZArray, Call, Key as ZKey, Owned, OwnedArray, Value as ZValue, ZString, bailout,
    sys::{zend_sort, zend_ulong},
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
                ZKey::Index(index) => Key::Int(index.c_cast::<i64>()),
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
    pub(crate) fn get(&self, key: &Key) -> Option<Owned> {
        self.read().get(key).and_then(Item::fetch)
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

    /// `index_offsetGet`, the item in the return slot of `call`: false when
    /// `index` is not below the count.
    pub(crate) fn index_get(&self, index: i64, call: &Call) -> bool {
        let table = self.read();
        if !below(index, table.len()) {
            return false;
        }
        if let Some(value) = table.get_int(index).and_then(Item::fetch) {
            call.set_owned(value);
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
    pub(crate) fn keys(&self) -> Owned {
        let table = self.read();
        let mut array = OwnedArray::default();
        for (key, _) in table.iter() {
            match key {
                Key::Int(index) => array.push_long(*index),
                Key::Str(name) => array.push_bytes(name),
            }
        }
        array.into_owned()
    }

    /// `values`.
    pub(crate) fn values(&self) -> Owned {
        let table = self.read();
        let mut array = OwnedArray::default();
        for (_, item) in table.iter() {
            array.push(item.fetch_or_null());
        }
        array.into_owned()
    }

    /// `to_array`.
    pub(crate) fn to_array(&self) -> Owned {
        let table = self.read();
        let mut array = OwnedArray::default();
        for (key, item) in table.iter() {
            let value = item.fetch_or_null();
            match key {
                Key::Int(index) => array.insert_index((*index).c_cast::<zend_ulong>(), value),
                // A table's string keys are never numeric, so this is `zend_hash_str_add`.
                Key::Str(name) => array.insert(name, value),
            }
        }
        array.into_owned()
    }

    /// `find`: the key of the first item equal to `search`, or nothing.
    pub(crate) fn find(&self, search: &ZValue) -> Option<Owned> {
        let table = self.read();
        let found = table.iter().find(|(_, item)| item.equals(search));
        match found? {
            (Key::Int(index), _) => Some(Owned::long(*index)),
            (Key::Str(name), _) => Some(Owned::string(ZString::new(name))),
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

/// `zval_get_long`, or -1 for null (`ZVAL_IS_NULL(zkey) ? -1 : zval_get_long(zkey)`).
pub(crate) fn list_index(key: &ZValue) -> i64 {
    if key.is_null() { -1 } else { key.long() }
}

/// `INIT_DECR_VALUE`: `delta` negated as a float, or as an int.
pub(crate) fn negate(delta: &ZValue) -> Owned {
    match delta.as_double() {
        Some(double) => Owned::double(-double),
        None => Owned::long(delta.long().wrapping_neg()),
    }
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
