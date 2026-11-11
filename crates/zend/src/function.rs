//! Static-method classes such as `Swoole\Timer`: global function aliases of
//! their methods, a `create_object` that refuses instances, and array results.
use crate::{
    Call,
    sys::{
        _zval_struct__bindgen_ty_1, IS_UNDEF, swr_zend_class_set_create_object, zend_class_entry,
        zend_function_entry, zend_internal_arg_info, zend_object, zend_object_handlers, zval,
    },
};
use std::{ffi::CStr, mem::offset_of, ptr};

impl Call {
    /// `add_assoc_bool_ex(return_value, key, value)`, after `set_array`
    /// (nothing unless the return slot holds an array).
    pub fn add_assoc_bool(&self, key: &CStr, value: bool) {
        // SAFETY: as in `add_assoc_long`.
        let slot = unsafe { &mut *self.return_value().cast::<crate::Owned>() };
        if let Some(array) = slot.array_mut() {
            array.insert_bool(key.to_bytes(), value);
        }
    }
}

/// Swoole's `sw_zend_create_object_deny`: the `create_object` of a class that
/// cannot be instantiated. It throws and returns the bare object Zend expects
/// (declared properties undefined), which Zend then releases.
/// # Safety
/// Installed as `create_object`, so Zend passes a registered class.
pub unsafe extern "C" fn create_object_deny(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: a registered class.
    let mut object = ptr::null_mut();
    // SAFETY: Zend's allocation failure is contained in the C leaf.
    if unsafe { crate::sys::swr_zend_try_facade_objects_new(ce, &raw mut object) } != 0 {
        crate::bailout::Bailout.defer();
    }
    crate::bailout::finish();
    // SAFETY: class entries live for the process.
    let class = unsafe { &*ce };
    let count = usize::try_from(class.default_properties_count).unwrap_or(0);
    // `zend_objects_new` allocated the class's property slots from here.
    let slots = object
        .wrapping_byte_add(offset_of!(zend_object, properties_table))
        .cast::<zval>();
    for index in 0..count {
        // SAFETY: as above; `ZVAL_UNDEF`.
        unsafe {
            (*slots.wrapping_add(index)).u1 = _zval_struct__bindgen_ty_1 {
                type_info: IS_UNDEF,
            };
        }
    }
    // SAFETY: Zend's error construction is trapped before this callback returns.
    if unsafe { crate::sys::swr_zend_try_facade_creation_error(ce) } != 0 {
        crate::bailout::Bailout.defer();
    }
    crate::bailout::finish();
    object
}

/// `SW_SET_CLASS_CREATE(.., sw_zend_create_object_deny)`.
pub(crate) fn deny(entry: &mut zend_class_entry, _: &mut zend_object_handlers) {
    // SAFETY: a registered entry, written at MINIT.
    unsafe { swr_zend_class_set_create_object(entry, Some(create_object_deny)) };
}

/// The arginfo of `method` in a method table (null if absent).
pub(crate) fn arg_info(
    methods: &[zend_function_entry],
    method: &CStr,
) -> *const zend_internal_arg_info {
    methods
        .iter()
        .take_while(|entry| !entry.fname.is_null())
        // SAFETY: entry names are NUL-terminated literals.
        .find(|entry| unsafe { CStr::from_ptr(entry.fname) } == method)
        .map_or(ptr::null(), |entry| entry.arg_info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_take_their_methods_arginfo() {
        let names = [c"tick", c"after", c"clearAll"];
        // SAFETY: all-zero entries are valid, and the last one ends the table.
        let mut table: [zend_function_entry; 4] = unsafe { std::mem::zeroed() };
        for (index, name) in names.iter().enumerate() {
            table[index].fname = name.as_ptr();
            table[index].arg_info = ptr::without_provenance(16 * (index + 1));
        }
        assert_eq!(arg_info(&table, c"after").addr(), 32, "after's arginfo");
        assert_eq!(arg_info(&table, c"clearAll").addr(), 48, "clearAll's");
        assert!(arg_info(&table, c"list").is_null(), "a missing method");
    }
}
