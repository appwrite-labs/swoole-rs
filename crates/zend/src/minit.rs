//! MINIT's registrations, safe behind the token Zend's MINIT hands a family
//! ([`minit!`](crate::minit)): constants, classes and their function aliases.
use crate::{
    Class, Owned,
    bailout::Bailout,
    class::Layout,
    function::{arg_info, deny},
    sys::{
        CONST_PERSISTENT, HashTable, IS_STRING, ZEND_ACC_NOT_SERIALIZABLE, swr_zend_http_intern,
        zend_class_entry, zend_class_implements, zend_declare_class_constant_long,
        zend_declare_property, zend_declare_property_string, zend_function_entry, zend_long,
        zend_object_handlers, zend_register_bool_constant, zend_register_double_constant,
        zend_register_long_constant, zend_register_string_constant,
    },
};
use std::{
    ffi::{CStr, c_int},
    marker::PhantomData,
    mem::offset_of,
};

unsafe extern "C" {
    /// SPL's classes: null until its MINIT, which runs before that of any
    /// extension depending on it, registers them; never written again.
    safe static spl_ce_ArrayIterator: *mut zend_class_entry;
    safe static spl_ce_ArrayObject: *mut zend_class_entry;
    /// Zend's own classes and interfaces, likewise: it registers them at
    /// startup, before any extension's MINIT, and never writes them again.
    safe static zend_ce_exception: *mut zend_class_entry;
    safe static zend_ce_error: *mut zend_class_entry;
    safe static zend_ce_iterator: *mut zend_class_entry;
    safe static zend_ce_stringable: *mut zend_class_entry;
    safe static zend_ce_arrayaccess: *mut zend_class_entry;
    safe static zend_ce_countable: *mut zend_class_entry;
    /// `CG(function_table)`: one pointer of the compiler's globals is read.
    safe fn swr_zend_facade_compiler_functions() -> *mut HashTable;
}

/// This module's MINIT, running on this thread: what makes registration safe.
/// It is neither `Send` nor `Clone`, and a family only borrows it.
pub struct Minit {
    module: c_int,
    _thread: PhantomData<*mut ()>,
}

/// This module's MSHUTDOWN, running on the main thread after every PHP thread
/// has ended: what makes undoing MINIT's process-wide registrations safe.
/// Neither `Send` nor `Clone`, as [`Minit`].
pub struct Mshutdown {
    _thread: PhantomData<*mut ()>,
}

impl Mshutdown {
    /// # Safety
    /// Zend is running this module's MSHUTDOWN on this thread, and the token
    /// is dropped before it returns.
    #[must_use]
    pub unsafe fn new() -> Self {
        Self {
            _thread: PhantomData,
        }
    }
}

/// A registered class another may extend.
#[derive(Clone, Copy)]
pub struct Parent(*mut zend_class_entry);

impl Parent {
    /// The class's entry.
    pub(crate) fn entry(self) -> *mut zend_class_entry {
        self.0
    }
}

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
        // SAFETY: MINIT, with the module's number; Zend copies the name.
        unsafe {
            zend_register_long_constant(
                name.as_ptr(),
                name.count_bytes(),
                value,
                persistent(),
                self.module,
            );
        }
    }

    /// `SW_REGISTER_DOUBLE_CONSTANT`.
    pub fn double_constant(&self, name: &CStr, value: f64) {
        // SAFETY: MINIT, with the module's number; Zend copies the name.
        unsafe {
            zend_register_double_constant(
                name.as_ptr(),
                name.count_bytes(),
                value,
                persistent(),
                self.module,
            );
        }
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

    /// `zend_string_init_interned(text, len, 1)` into `slot`: MINIT's strings
    /// are interned permanently.
    pub fn intern(&self, slot: &crate::Interned, text: &[u8]) {
        // SAFETY: MINIT; Zend copies the bytes.
        let string = unsafe { swr_zend_http_intern(text.as_ptr().cast(), text.len()) };
        slot.0.store(string, std::sync::atomic::Ordering::Relaxed);
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
        plain(class);
        terminated(methods);
        self.register_plain(class, name, methods, None, deny);
    }

    /// The compiler's function table (`CG(function_table)`), where MINIT
    /// registers global functions.
    #[must_use]
    pub fn compiler_functions(&self) -> *mut HashTable {
        swr_zend_facade_compiler_functions()
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
        let entry = registered(class);
        terminated(methods);
        // The class's own table, projected without a Rust reference to the entry.
        let source = entry
            .wrapping_byte_add(offset_of!(zend_class_entry, function_table))
            .cast::<HashTable>();
        let target = self.compiler_functions();
        for (method, alias) in aliases {
            // SAFETY: MINIT; a registered class, which lives for the process, has
            // its table of internal methods, the compiler's is live, and the
            // arginfo comes from the method's row of the static table.
            unsafe {
                crate::registration::function_alias(
                    source,
                    method,
                    target,
                    alias,
                    arg_info(methods, method),
                )
            }?;
        }
        Ok(())
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
        self.register_plain(class, name, methods, None, configure);
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
        self.register_plain(class, name, methods, Some(parent), configure);
    }

    /// [`Class::register_child`] of a class that is no
    /// [`CustomClass`](crate::CustomClass)'s, with `methods` (none when empty).
    fn register_plain(
        &self,
        class: &Class,
        name: &CStr,
        methods: &'static [zend_function_entry],
        parent: Option<Parent>,
        configure: impl FnOnce(&mut zend_class_entry, &mut zend_object_handlers),
    ) {
        plain(class).register_child(self, name, methods, parent, None, configure);
    }

    /// Swoole's `SW_CLASS_ALIAS`: `name` (no leading backslash) names `class` too.
    /// # Errors
    /// A contained PHP allocation or alias registration bailout.
    /// # Panics
    /// `class` is not registered.
    pub fn alias(&self, class: &Class, name: &CStr) -> Result<(), Bailout> {
        // SAFETY: MINIT; a NUL-terminated name and a registered class.
        unsafe {
            crate::registration::class_alias(
                name.as_ptr(),
                name.count_bytes(),
                registered(class),
                false,
            )
        }
        .map(|_| ())
    }

    /// Register `T`'s class as `name` with `methods` and `flags` added to its
    /// `ce_flags`: not serializable (unless [`Custom::serializable`](crate::Custom::serializable))
    /// and, as Swoole's custom objects are, not cloneable, with `unset` denied, its objects embedding a `T`
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
        terminated(methods);
        assert!(class.entry().is_null(), "a class registers once");
        let configure = |entry: &mut zend_class_entry, _: &mut _| {
            entry.ce_flags |= flags;
            if !T::serializable() {
                entry.ce_flags |= ZEND_ACC_NOT_SERIALIZABLE;
            }
        };
        // `T`'s layout gives the class `T`'s handlers.
        class.register_child(
            self,
            name,
            methods,
            None,
            Some(Layout::of::<T>()),
            configure,
        );
    }

    /// `zend_declare_class_constant_long` on a registered class.
    /// # Panics
    /// `class` is not registered.
    pub fn class_constant_long(&self, class: &Class, name: &CStr, value: zend_long) {
        // SAFETY: MINIT, with a registered class; Zend copies the name.
        unsafe {
            zend_declare_class_constant_long(
                registered(class),
                name.as_ptr(),
                name.count_bytes(),
                value,
            );
        }
    }

    /// `zend_declare_property_long` with `access` (`ZEND_ACC_*`) flags.
    /// # Panics
    /// `class` is not registered.
    pub fn property_long(&self, class: &Class, name: &CStr, value: zend_long, access: u32) {
        self.property(class, name, &Owned::long(value), access);
    }

    /// `zend_declare_property_double` with `access` flags.
    /// # Panics
    /// `class` is not registered.
    pub fn property_double(&self, class: &Class, name: &CStr, value: f64, access: u32) {
        self.property(class, name, &Owned::double(value), access);
    }

    /// `zend_declare_property_bool` with `access` flags.
    /// # Panics
    /// `class` is not registered.
    pub fn property_bool(&self, class: &Class, name: &CStr, value: bool, access: u32) {
        self.property(class, name, &Owned::bool(value), access);
    }

    /// `zend_declare_property_null` with `access` flags.
    /// # Panics
    /// `class` is not registered.
    pub fn property_null(&self, class: &Class, name: &CStr, access: u32) {
        self.property(class, name, &Owned::null(), access);
    }

    /// `zend_declare_property` of a scalar default, as Zend's typed
    /// `zend_declare_property_{long,double,bool,null}` each call it.
    /// # Panics
    /// `class` is not registered, or `value` is no scalar: an internal class's
    /// default must not be a request allocation.
    #[expect(
        clippy::unused_self,
        reason = "the token is what proves this runs at MINIT"
    )]
    fn property(&self, class: &Class, name: &CStr, value: &Owned, access: u32) {
        assert!(value.kind() < IS_STRING, "a scalar default");
        // SAFETY: MINIT, with a registered class; Zend copies the name and
        // the scalar zval.
        unsafe {
            zend_declare_property(
                registered(class),
                name.as_ptr(),
                name.count_bytes(),
                value.as_ptr(),
                flags(access),
            );
        }
    }

    /// `zend_declare_property_string` with `access` flags.
    /// # Panics
    /// `class` is not registered.
    pub fn property_string(&self, class: &Class, name: &CStr, value: &CStr, access: u32) {
        // SAFETY: MINIT, with a registered class; Zend copies the name and value.
        unsafe {
            zend_declare_property_string(
                registered(class),
                name.as_ptr(),
                name.count_bytes(),
                value.as_ptr(),
                flags(access),
            );
        }
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
        Parent(zend_ce_exception)
    }

    /// Zend's `Error`.
    #[must_use]
    pub fn error(&self) -> Parent {
        Parent(zend_ce_error)
    }

    /// SPL's `ArrayObject`.
    #[must_use]
    pub fn array_object(&self) -> Parent {
        Parent(spl_ce_ArrayObject)
    }

    /// Zend's `Iterator`.
    #[must_use]
    pub fn iterator(&self) -> Parent {
        Parent(zend_ce_iterator)
    }

    /// Zend's `Stringable`.
    #[must_use]
    pub fn stringable(&self) -> Parent {
        Parent(zend_ce_stringable)
    }

    /// Zend's `ArrayAccess`.
    #[must_use]
    pub fn array_access(&self) -> Parent {
        Parent(zend_ce_arrayaccess)
    }

    /// Zend's `Countable`.
    #[must_use]
    pub fn countable(&self) -> Parent {
        Parent(zend_ce_countable)
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
        Parent(spl_ce_ArrayIterator)
    }
}

/// `CONST_PERSISTENT` as the `int` the register functions take.
fn persistent() -> c_int {
    c_int::try_from(CONST_PERSISTENT).unwrap_or(c_int::MAX)
}

/// `ZEND_ACC_*` flags as the `int` the declare functions take.
fn flags(value: u32) -> c_int {
    c_int::try_from(value).unwrap_or(c_int::MAX)
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
    use std::ptr;

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
