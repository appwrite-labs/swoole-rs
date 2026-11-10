//! Static-method classes such as `Swoole\Timer`: global function aliases of
//! their methods, a `create_object` that refuses instances, and array results.
use crate::{
    Call, Class,
    sys::{
        _zval_struct__bindgen_ty_1, IS_UNDEF, swr_zend_class_set_create_object, zend_class_entry,
        zend_function_entry, zend_internal_arg_info, zend_object, zend_object_handlers, zval,
    },
};
use std::{ffi::CStr, mem::offset_of, ptr};

impl Class {
    /// Register a class of static methods that cannot be instantiated
    /// (`SW_INIT_CLASS_ENTRY` then `SW_SET_CLASS_CREATE(.., sw_zend_create_object_deny)`).
    /// # Safety
    /// As `register`.
    pub unsafe fn register_static(&self, name: &CStr, methods: *const zend_function_entry) {
        // SAFETY: per the caller.
        unsafe { self.register(name, methods, deny) };
    }

    /// `SW_FUNCTION_ALIAS` of each `(method, alias)` in order, with the arginfo
    /// the method has in `methods`.
    /// # Safety
    /// At MINIT, after `register` with `methods`.
    /// # Errors
    /// Stops at a contained allocation or registration bailout.
    pub unsafe fn function_aliases(
        &self,
        methods: *const zend_function_entry,
        aliases: &[(&CStr, &CStr)],
    ) -> Result<(), crate::bailout::Bailout> {
        for (method, alias) in aliases {
            // SAFETY: per the caller.
            unsafe { self.function_alias(method, alias, arg_info(methods, method)) }?;
        }
        Ok(())
    }

    /// Swoole's `SW_FUNCTION_ALIAS(&ce->function_table, method, CG(function_table),
    /// alias, arg_info)`: the global function `alias` runs static `method`.
    /// # Safety
    /// At MINIT, after `register`; `arg_info` is the method's static arginfo.
    /// # Errors
    /// Returns a contained PHP allocation or registration bailout.
    pub unsafe fn function_alias(
        &self,
        method: &CStr,
        alias: &CStr,
        arg_info: *const zend_internal_arg_info,
    ) -> Result<(), crate::bailout::Bailout> {
        let entry = self.entry();
        // SAFETY: the class entry is process-lived. Project the raw table pointer
        // without holding a mutable Rust reference across Zend registration.
        let source = unsafe { &raw mut (*entry).function_table };
        // SAFETY: Zend initializes the compiler's table before module startup.
        let target = unsafe { crate::sys::swr_zend_facade_compiler_functions() };
        // SAFETY: the names and arginfo remain live through the guarded registration.
        unsafe {
            crate::registration::function_alias(
                source,
                method.as_ptr(),
                method.count_bytes(),
                target,
                alias.as_ptr(),
                alias.count_bytes(),
                arg_info,
                None,
            )
        }
        .map(|_| ())
    }
}

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
fn deny(entry: &mut zend_class_entry, _: &mut zend_object_handlers) {
    // SAFETY: a registered entry, written at MINIT.
    unsafe { swr_zend_class_set_create_object(entry, Some(create_object_deny)) };
}

/// The arginfo of `method` in a `PHP_FE_END`-terminated table (null if absent).
fn arg_info(methods: *const zend_function_entry, method: &CStr) -> *const zend_internal_arg_info {
    let mut entry = methods;
    loop {
        // SAFETY: a terminated table; `entry` has not passed the end.
        let current = unsafe { &*entry };
        if current.fname.is_null() {
            return ptr::null();
        }
        // SAFETY: entry names are NUL-terminated literals.
        if unsafe { CStr::from_ptr(current.fname) } == method {
            return current.arg_info;
        }
        entry = entry.wrapping_add(1);
    }
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
        assert_eq!(
            arg_info(table.as_ptr(), c"after").addr(),
            32,
            "after's arginfo"
        );
        assert_eq!(
            arg_info(table.as_ptr(), c"clearAll").addr(),
            48,
            "clearAll's"
        );
        assert!(
            arg_info(table.as_ptr(), c"list").is_null(),
            "a missing method"
        );
    }
}
