//! MINIT's registrations, safe behind the token Zend's MINIT hands a family
//! ([`minit!`](crate::minit)): constants, classes and their function aliases.
use crate::{
    Class,
    bailout::Bailout,
    sys::{ZEND_ACC_NOT_SERIALIZABLE, zend_class_entry, zend_function_entry, zend_long},
};
use std::{
    ffi::{CStr, c_int},
    marker::PhantomData,
    ptr,
};

unsafe extern "C" {
    static spl_ce_ArrayIterator: *mut zend_class_entry;
}

/// This module's MINIT, running on this thread: what makes registration safe.
/// It is neither `Send` nor `Clone`, and a family only borrows it.
pub struct Minit {
    module: c_int,
    _thread: PhantomData<*mut ()>,
}

/// A registered class another may extend.
#[derive(Clone, Copy)]
pub struct Parent(*mut zend_class_entry);

impl Minit {
    /// # Safety
    /// Zend is running the MINIT of the module numbered `module` on this thread,
    /// and the token is dropped before that MINIT returns.
    #[must_use]
    pub unsafe fn new(module: c_int) -> Self {
        Self {
            module,
            _thread: PhantomData,
        }
    }

    /// `SW_REGISTER_LONG_CONSTANT`.
    pub fn long_constant(&self, name: &CStr, value: zend_long) {
        // SAFETY: MINIT, with the module's number.
        unsafe { crate::register_long_constant(name, value, self.module) };
    }

    /// `SW_REGISTER_DOUBLE_CONSTANT`.
    pub fn double_constant(&self, name: &CStr, value: f64) {
        // SAFETY: MINIT, with the module's number.
        unsafe { crate::register_double_constant(name, value, self.module) };
    }

    /// [`Class::register_static`] of `name` with `methods`, then `SW_FUNCTION_ALIAS`
    /// of each `(method, alias)` with that method's arginfo.
    /// # Errors
    /// Stops at a contained allocation or registration bailout.
    /// # Panics
    /// `methods` is not `PHP_FE_END`-terminated.
    pub fn register_static(
        &self,
        class: &Class,
        name: &CStr,
        methods: &'static [zend_function_entry],
        aliases: &[(&CStr, &CStr)],
    ) -> Result<(), Bailout> {
        let table = terminated(methods);
        // SAFETY: MINIT, with a static, terminated table.
        unsafe { class.register_static(name, table) };
        // SAFETY: MINIT, after registering the class with the same table.
        unsafe { class.function_aliases(table, aliases) }
    }

    /// [`Class::register_child`] of `name`, extending `parent`, with `methods`
    /// (none when empty) and `flags` added to its `ce_flags`.
    /// # Panics
    /// `methods` is neither empty nor `PHP_FE_END`-terminated.
    pub fn register_child(
        &self,
        class: &Class,
        name: &CStr,
        methods: &'static [zend_function_entry],
        parent: Parent,
        flags: u32,
    ) {
        let table = if methods.is_empty() {
            ptr::null()
        } else {
            terminated(methods)
        };
        // SAFETY: MINIT, with a static, terminated table or none, and a registered parent.
        unsafe { class.register_child(name, table, parent.0, |entry, _| entry.ce_flags |= flags) };
    }

    /// Register `T`'s class as `name` with `methods` and `flags` added to its
    /// `ce_flags`: not serializable and, as Swoole's custom objects are, not
    /// cloneable, with `unset` denied, its objects embedding a `T`
    /// ([`Custom`](crate::Custom)).
    /// # Panics
    /// The class is already registered, or `methods` is not `PHP_FE_END`-terminated.
    pub fn register_object<T: crate::Custom>(
        &self,
        name: &CStr,
        methods: &'static [zend_function_entry],
        flags: u32,
    ) {
        let class = T::class();
        let table = terminated(methods);
        assert!(class.entry().is_null(), "a class registers once");
        let configure = |entry: &mut zend_class_entry, handlers: &mut _| {
            entry.ce_flags |= flags | ZEND_ACC_NOT_SERIALIZABLE;
            crate::object::install_custom::<T>(entry, handlers);
        };
        // SAFETY: MINIT, with a static, terminated table.
        unsafe { class.register(name, table, configure) };
    }

    /// `zend_declare_property_long(..., ZEND_ACC_PUBLIC)` on a registered class.
    /// # Panics
    /// `class` is not registered.
    pub fn property_long(&self, class: &Class, name: &CStr, value: zend_long) {
        assert!(!class.entry().is_null(), "a registered class");
        // SAFETY: MINIT, after registration.
        unsafe { class.property_long(name, value) };
    }

    /// `zend_declare_class_constant_long` on a registered class.
    /// # Panics
    /// `class` is not registered.
    pub fn class_constant_long(&self, class: &Class, name: &CStr, value: zend_long) {
        assert!(!class.entry().is_null(), "a registered class");
        // SAFETY: MINIT, after registration.
        unsafe { class.constant_long(name, value) };
    }

    /// A registered class, to extend.
    /// # Panics
    /// `class` is not registered.
    #[must_use]
    pub fn parent(&self, class: &Class) -> Parent {
        let entry = class.entry();
        assert!(!entry.is_null(), "a registered class");
        Parent(entry)
    }

    /// SPL's `ArrayIterator`.
    #[must_use]
    pub fn array_iterator(&self) -> Parent {
        // SAFETY: SPL registers it at its MINIT, before any extension that depends on it.
        Parent(unsafe { spl_ce_ArrayIterator })
    }
}

/// `methods` as a table pointer, checking its `PHP_FE_END`.
fn terminated(methods: &'static [zend_function_entry]) -> *const zend_function_entry {
    assert!(
        methods.last().is_some_and(|end| end.fname.is_null()),
        "a method table ends with PHP_FE_END"
    );
    methods.as_ptr()
}

/// A family's MINIT hook (`unsafe extern "C" fn(int)`, which Zend runs inside
/// `zend_try`) that runs `body(&Minit)` and raises the bailout it returns.
#[macro_export]
macro_rules! minit {
    ($name:ident => $body:path) => {
        /// A family's MINIT hook.
        /// # Safety
        /// Called by Zend at this module's MINIT, with its number.
        pub unsafe extern "C" fn $name(module_number: ::std::ffi::c_int) {
            // SAFETY: per Zend's MINIT contract; the token lives for this call.
            let minit = unsafe { $crate::Minit::new(module_number) };
            $crate::bailout::raise($body(&minit));
        }
    };
}
