//! MINIT's registrations, safe behind the token Zend's MINIT hands a family
//! ([`minit!`](crate::minit)): constants, classes and their function aliases.
use crate::{
    Class,
    bailout::Bailout,
    sys::{
        CONST_PERSISTENT, ZEND_ACC_NOT_SERIALIZABLE, zend_ce_arrayaccess, zend_ce_countable,
        zend_ce_error, zend_ce_exception, zend_ce_iterator, zend_class_entry,
        zend_class_implements, zend_function_entry, zend_long, zend_object_handlers,
        zend_register_bool_constant, zend_register_string_constant,
    },
};
use std::{
    ffi::{CStr, c_int},
    marker::PhantomData,
    ptr,
};

unsafe extern "C" {
    static spl_ce_ArrayIterator: *mut zend_class_entry;
    static spl_ce_ArrayObject: *mut zend_class_entry;
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
        unsafe { crate::class::register_long_constant(name, value, self.module) };
    }

    /// `SW_REGISTER_DOUBLE_CONSTANT`.
    pub fn double_constant(&self, name: &CStr, value: f64) {
        // SAFETY: MINIT, with the module's number.
        unsafe { crate::class::register_double_constant(name, value, self.module) };
    }

    /// `REGISTER_BOOL_CONSTANT`, persistent.
    pub fn bool_constant(&self, name: &CStr, value: bool) {
        // SAFETY: MINIT, with the module's number; Zend copies the name.
        unsafe {
            zend_register_bool_constant(
                name.as_ptr(),
                name.count_bytes(),
                value,
                persistent(),
                self.module,
            );
        }
    }

    /// `REGISTER_STRING_CONSTANT`, persistent.
    pub fn string_constant(&self, name: &CStr, value: &CStr) {
        // SAFETY: MINIT, with the module's number; Zend copies the name and value.
        unsafe {
            zend_register_string_constant(
                name.as_ptr(),
                name.count_bytes(),
                value.as_ptr(),
                persistent(),
                self.module,
            );
        }
    }

    /// The module number Zend gave this module.
    #[must_use]
    pub fn module(&self) -> c_int {
        self.module
    }

    /// [`Class::register_static`] of `name` with `methods`.
    /// # Panics
    /// `class` is a [`CustomClass`](crate::CustomClass)'s, or `methods` is not
    /// `PHP_FE_END`-terminated.
    pub fn register_static(
        &self,
        class: &Class,
        name: &CStr,
        methods: &'static [zend_function_entry],
    ) {
        // SAFETY: MINIT, with a plain class and a static, terminated table.
        unsafe { plain(class).register_static(name, terminated(methods)) };
    }

    /// `SW_FUNCTION_ALIAS` of each `(method, alias)` of `class`, registered with
    /// `methods`, with that method's arginfo.
    /// # Errors
    /// Stops at a contained allocation or registration bailout.
    /// # Panics
    /// `class` is not registered, or `methods` is not `PHP_FE_END`-terminated.
    pub fn function_aliases(
        &self,
        class: &Class,
        methods: &'static [zend_function_entry],
        aliases: &[(&CStr, &CStr)],
    ) -> Result<(), Bailout> {
        registered(class);
        // SAFETY: MINIT, after registering the class; the table is static and terminated.
        unsafe { class.function_aliases(terminated(methods), aliases) }
    }

    /// Register `name` with `methods` (none when empty), then let `configure`
    /// adjust the entry and a copy of the standard handlers (Swoole's
    /// `SW_INIT_CLASS_ENTRY` followed by its `SW_SET_CLASS_*` macros).
    /// # Panics
    /// `class` is a [`CustomClass`](crate::CustomClass)'s, or `methods` is
    /// neither empty nor `PHP_FE_END`-terminated.
    pub fn register(
        &self,
        class: &Class,
        name: &CStr,
        methods: &'static [zend_function_entry],
        configure: impl FnOnce(&mut zend_class_entry, &mut zend_object_handlers),
    ) {
        let class = plain(class);
        // SAFETY: MINIT, with a plain class and a static, terminated table or none.
        unsafe { class.register_child(name, table(methods), ptr::null_mut(), configure) };
    }

    /// [`Self::register`], extending `parent`.
    /// # Panics
    /// As [`Self::register`].
    pub fn register_child(
        &self,
        class: &Class,
        name: &CStr,
        methods: &'static [zend_function_entry],
        parent: Parent,
        configure: impl FnOnce(&mut zend_class_entry, &mut zend_object_handlers),
    ) {
        let class = plain(class);
        // SAFETY: MINIT, with a plain class, a static, terminated table or none,
        // and a registered parent.
        unsafe { class.register_child(name, table(methods), parent.0, configure) };
    }

    /// Swoole's `SW_CLASS_ALIAS`: `name` (no leading backslash) names `class` too.
    /// # Errors
    /// A contained PHP allocation or alias registration bailout.
    /// # Panics
    /// `class` is not registered.
    pub fn alias(&self, class: &Class, name: &CStr) -> Result<(), Bailout> {
        registered(class);
        // SAFETY: MINIT, after registration.
        unsafe { class.alias(name) }
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

    /// `zend_declare_class_constant_long` on a registered class.
    /// # Panics
    /// `class` is not registered.
    pub fn class_constant_long(&self, class: &Class, name: &CStr, value: zend_long) {
        registered(class);
        // SAFETY: MINIT, after registration.
        unsafe { class.constant_long(name, value) };
    }

    /// `zend_declare_property_long` with `access` (`ZEND_ACC_*`) flags.
    /// # Panics
    /// `class` is not registered.
    pub fn property_long(&self, class: &Class, name: &CStr, value: zend_long, access: u32) {
        // SAFETY: MINIT, with a registered class.
        unsafe { crate::class::declare_long(registered(class), name, value, access) };
    }

    /// `zend_declare_property_double` with `access` flags.
    /// # Panics
    /// `class` is not registered.
    pub fn property_double(&self, class: &Class, name: &CStr, value: f64, access: u32) {
        // SAFETY: MINIT, with a registered class.
        unsafe { crate::class::declare_double(registered(class), name, value, access) };
    }

    /// `zend_declare_property_bool` with `access` flags.
    /// # Panics
    /// `class` is not registered.
    pub fn property_bool(&self, class: &Class, name: &CStr, value: bool, access: u32) {
        // SAFETY: MINIT, with a registered class.
        unsafe { crate::class::declare_bool(registered(class), name, value, access) };
    }

    /// `zend_declare_property_string` with `access` flags.
    /// # Panics
    /// `class` is not registered.
    pub fn property_string(&self, class: &Class, name: &CStr, value: &CStr, access: u32) {
        // SAFETY: MINIT, with a registered class.
        unsafe { crate::class::declare_string(registered(class), name, value, access) };
    }

    /// `zend_declare_property_null` with `access` flags.
    /// # Panics
    /// `class` is not registered.
    pub fn property_null(&self, class: &Class, name: &CStr, access: u32) {
        // SAFETY: MINIT, with a registered class.
        unsafe { crate::class::declare_null(registered(class), name, access) };
    }

    /// A registered class, to extend.
    /// # Panics
    /// `class` is not registered.
    #[must_use]
    pub fn parent(&self, class: &Class) -> Parent {
        Parent(registered(class))
    }

    /// Zend's `Exception`.
    #[must_use]
    pub fn exception(&self) -> Parent {
        // SAFETY: Zend registers its classes before any extension's MINIT.
        Parent(unsafe { zend_ce_exception })
    }

    /// Zend's `Error`.
    #[must_use]
    pub fn error(&self) -> Parent {
        // SAFETY: as above.
        Parent(unsafe { zend_ce_error })
    }

    /// SPL's `ArrayObject`.
    #[must_use]
    pub fn array_object(&self) -> Parent {
        // SAFETY: SPL registers it at its MINIT, before any extension that depends on it.
        Parent(unsafe { spl_ce_ArrayObject })
    }

    /// Zend's `Iterator`.
    #[must_use]
    pub fn iterator(&self) -> Parent {
        // SAFETY: Zend registers its interfaces before any extension's MINIT.
        Parent(unsafe { zend_ce_iterator })
    }

    /// Zend's `ArrayAccess`.
    #[must_use]
    pub fn array_access(&self) -> Parent {
        // SAFETY: Zend registers its interfaces before any extension's MINIT.
        Parent(unsafe { zend_ce_arrayaccess })
    }

    /// Zend's `Countable`.
    #[must_use]
    pub fn countable(&self) -> Parent {
        // SAFETY: as above.
        Parent(unsafe { zend_ce_countable })
    }

    /// `zend_class_implements(class, 1, interface)`.
    /// # Panics
    /// `class` is not registered.
    pub fn implements(&self, class: &Class, interface: Parent) {
        // SAFETY: MINIT, a registered class and a registered interface.
        unsafe { zend_class_implements(registered(class), 1, interface.0) };
    }

    /// SPL's `ArrayIterator`.
    #[must_use]
    pub fn array_iterator(&self) -> Parent {
        // SAFETY: SPL registers it at its MINIT, before any extension that depends on it.
        Parent(unsafe { spl_ce_ArrayIterator })
    }
}

/// `CONST_PERSISTENT` as the `int` the register functions take.
fn persistent() -> c_int {
    c_int::try_from(CONST_PERSISTENT).unwrap_or(c_int::MAX)
}

/// `class`, checking it is no [`CustomClass`](crate::CustomClass)'s: registered
/// here, its objects would get `T`'s handlers ([`crate::new_object`]) without
/// the layout and free handler [`Minit::register_object`] gives them.
fn plain(class: &Class) -> &Class {
    assert!(
        !class.custom,
        "a CustomClass registers through register_object"
    );
    class
}

/// `class`'s entry, checking it is registered.
fn registered(class: &Class) -> *mut zend_class_entry {
    let entry = class.entry();
    assert!(!entry.is_null(), "a registered class");
    entry
}

/// `methods` as a table pointer: null when empty, else checking its `PHP_FE_END`.
fn table(methods: &'static [zend_function_entry]) -> *const zend_function_entry {
    if methods.is_empty() {
        ptr::null()
    } else {
        terminated(methods)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_class_registers_through_register() {
        let class = Class::new();
        assert!(ptr::eq(plain(&class), &raw const class));
    }

    #[test]
    #[should_panic(expected = "a CustomClass registers through register_object")]
    fn a_custom_class_registers_only_through_register_object() {
        let _ = plain(&crate::CustomClass::<u64>::new());
    }
}
