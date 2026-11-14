//! MINIT's registrations, safe behind the token Zend's MINIT hands a family:
//! constants, classes and their function aliases.
use crate::{
    Array, Class, ClassEntry, Function, Owned, Value,
    arginfo::ArgInfo,
    bailout::Bailout,
    class::Layout,
    function::{arg_info, deny},
    helpers::CText,
    registration::{SUCCESS, leaf},
    sys::{
        HashTable, IS_STRING, ZEND_ACC_NOT_SERIALIZABLE, swr_zend_http_intern,
        swr_zend_try_facade_register_functions, zend_class_entry, zend_class_implements,
        zend_function, zend_function_entry, zend_internal_arg_info, zend_long,
        zend_object_handlers,
    },
};
use std::{
    ffi::{CStr, c_int, c_void},
    marker::PhantomData,
    mem::offset_of,
    ptr,
};

// MINIT's registrations (shim_minit.h), each over NUL-terminated names, a
// registered class or plain values, and total: it registers, or does nothing
// and says so where Zend would raise a fatal error. A global constant that is
// defined already gets Zend's warning, which at MINIT reaches no PHP error
// handler. None runs PHP code.
// Private, so only a [`Minit`]'s methods call them: at MINIT, on its thread.
unsafe extern "C" {
    safe fn swr_zend_minit_long_constant(name: CText<'_>, value: zend_long, module: c_int);
    safe fn swr_zend_minit_double_constant(name: CText<'_>, value: f64, module: c_int);
    safe fn swr_zend_minit_bool_constant(name: CText<'_>, value: bool, module: c_int);
    safe fn swr_zend_minit_string_constant(name: CText<'_>, value: CText<'_>, module: c_int);
    safe fn swr_zend_minit_class_constant_long(
        class: ClassEntry<'static>,
        name: CText<'_>,
        value: zend_long,
    ) -> bool;
    safe fn swr_zend_minit_class_constant_string(
        class: ClassEntry<'static>,
        name: CText<'_>,
        value: CText<'_>,
    ) -> bool;
    /// It copies `value` unless that is refcounted.
    safe fn swr_zend_minit_property(
        class: ClassEntry<'static>,
        name: CText<'_>,
        value: &Value,
        access: c_int,
    ) -> bool;
    safe fn swr_zend_minit_property_string(
        class: ClassEntry<'static>,
        name: CText<'_>,
        value: CText<'_>,
        access: c_int,
    );
    safe fn swr_zend_minit_sensitive_parameter(
        class: ClassEntry<'static>,
        method: CText<'_>,
        offset: u32,
    ) -> bool;
    /// `CG(function_table)`'s function `name`, or null: one lookup in the
    /// compiler's table.
    safe fn swr_zend_compiled_function_find(name: CText<'_>) -> *mut zend_function;
    /// The php.ini entry `name`'s string, or none for an unset or non-string entry:
    /// one lookup in the configuration, filled before MINIT and freed at module
    /// shutdown. Only [`Minit::ini_string`] calls it, which ties the string to MINIT.
    safe fn swr_zend_minit_ini_string<'m>(name: CText<'_>) -> Option<CText<'m>>;
    /// The native scopes' hooks (`native/corebase/swoole_rs_zend_guard.h`): two
    /// process-wide function pointers are stored, for scopes entered later,
    /// which call them with no arguments.
    safe fn swr_zend_guard_register(
        context: extern "C" fn() -> *mut c_void,
        resume: extern "C" fn(),
    );
}

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

    /// `SW_REGISTER_LONG_CONSTANT`: Zend warns when a constant has the name already.
    pub fn long_constant(&self, name: &CStr, value: zend_long) {
        swr_zend_minit_long_constant(name.into(), value, self.module);
    }

    /// `SW_REGISTER_DOUBLE_CONSTANT`, as [`Self::long_constant`].
    pub fn double_constant(&self, name: &CStr, value: f64) {
        swr_zend_minit_double_constant(name.into(), value, self.module);
    }

    /// `REGISTER_BOOL_CONSTANT`, persistent, as [`Self::long_constant`].
    pub fn bool_constant(&self, name: &CStr, value: bool) {
        swr_zend_minit_bool_constant(name.into(), value, self.module);
    }

    /// `REGISTER_STRING_CONSTANT`, persistent, as [`Self::long_constant`].
    pub fn string_constant(&self, name: &CStr, value: &CStr) {
        swr_zend_minit_string_constant(name.into(), value.into(), self.module);
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
            .cast::<Array>();
        // SAFETY: MINIT; a registered class, which lives for the process, has its
        // table of internal methods. `Array` is a transparent table Zend may change.
        let source = unsafe { &*source };
        for (method, alias) in aliases {
            let info = arg_info(methods, method).map_or(Info::NONE, Info::row);
            // Lookup lowercases the method's ASCII bytes.
            let lower = method.to_bytes().to_ascii_lowercase();
            let found = source.find(&lower).and_then(Value::pointer);
            // The class's table owns this live function, which remains live
            // and unchanged throughout the registration.
            let function = found.map(|function| Function(function.cast(), PhantomData));
            self.alias_from(function, alias, info)?;
        }
        Ok(())
    }

    /// `SW_FUNCTION_ALIAS(CG(function_table), origin, CG(function_table), alias)`
    /// with `info`, `origin`'s arginfo: whether Zend registered it, false when the
    /// compiler knows no internal `origin` (lower case) whose parameters `info`
    /// covers.
    /// # Errors
    /// A contained registration bailout.
    pub fn function_alias<const N: usize>(
        &self,
        origin: &CStr,
        alias: &CStr,
        info: &'static ArgInfo<N>,
    ) -> Result<bool, Bailout> {
        let found = swr_zend_compiled_function_find(origin.into());
        // The compiler's table owns this live function, which remains live and
        // unchanged throughout the registration.
        let function = (!found.is_null()).then_some(Function(found, PhantomData));
        let info = Info {
            start: info.0.as_ptr(),
            rows: N,
        };
        self.alias_from(function, alias, info)
    }

    /// Register `alias` in the compiler's table using the internal `function`:
    /// its real handler and argument count, with `info`. Whether Zend
    /// registered it: false without an internal function, when `info` misses
    /// one of its parameters, or when the registration failed.
    fn alias_from(
        &self,
        function: Option<Function<'_>>,
        alias: &CStr,
        info: Info,
    ) -> Result<bool, Bailout> {
        let Some(function) = function else {
            return Ok(false);
        };
        let (kind, args) = (function.kind(), function.num_args());
        let Some(handler) = function.internal_handler() else {
            return Ok(false);
        };
        // Zend reads the return row and the `args` rows after it.
        if !info.start.is_null() && info.rows <= args as usize {
            return Ok(false);
        }
        let mut entries = crate::zeroed::<[zend_function_entry; 2]>();
        entries[0].fname = alias.as_ptr();
        entries[0].handler = handler;
        entries[0].arg_info = info.start;
        entries[0].num_args = args;
        let mut result = -1;
        // SAFETY: Zend borrows this complete terminated table, its NUL-terminated
        // name and `info`'s static rows while it copies the registered metadata,
        // inside the contained leaf, into the compiler's live table (MINIT).
        leaf(unsafe {
            swr_zend_try_facade_register_functions(
                entries.as_ptr(),
                self.compiler_functions(),
                c_int::from(kind),
                &raw mut result,
            )
        })?;
        Ok(result == SUCCESS)
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
        // SAFETY: MINIT (the token) and a registered class.
        unsafe { crate::registration::class_alias(name.to_bytes(), registered(class), false) }
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
    /// `class` is not registered, has the constant already, or the name is
    /// `class`: Zend's fatal errors.
    pub fn class_constant_long(&self, class: &Class, name: &CStr, value: zend_long) {
        let declared = swr_zend_minit_class_constant_long(declared(class), name.into(), value);
        assert!(declared, "class constant {name:?} declared twice");
    }

    /// `zend_declare_class_constant_string`, as [`Self::class_constant_long`].
    /// # Panics
    /// `class` is not registered.
    pub fn class_constant_string(&self, class: &Class, name: &CStr, value: &CStr) {
        let declared =
            swr_zend_minit_class_constant_string(declared(class), name.into(), value.into());
        assert!(declared, "class constant {name:?} declared twice");
    }

    /// `zend_add_parameter_attribute(method, offset, SensitiveParameter)`: PHP
    /// hides argument `offset` (from 0) of `class`'s `method` (lower case) in
    /// backtraces. Nothing when the class has no such method or parameter.
    /// # Panics
    /// `class` is not registered.
    pub fn sensitive_parameter(&self, class: &Class, method: &CStr, offset: u32) {
        swr_zend_minit_sensitive_parameter(declared(class), method.into(), offset);
    }

    /// `zend::get_function(CG(function_table), name)`: the function the
    /// compiler knows as `name` (lower case), or null.
    #[must_use]
    pub fn compiled_function(&self, name: &CStr) -> *mut zend_function {
        swr_zend_compiled_function_find(name.into())
    }

    /// `cfg_get_string(name)`: the php.ini entry `name`, or None when it is unset
    /// or no string.
    #[must_use]
    pub fn ini_string(&self, name: &CStr) -> Option<&CStr> {
        swr_zend_minit_ini_string(name.into()).map(CText::get)
    }

    /// `swr_zend_guard_register`: what names the running native scope
    /// (`context`), and what a whole-thread exit runs before it bails out
    /// (`resume`), from now on.
    pub fn guard_register(&self, context: extern "C" fn() -> *mut c_void, resume: extern "C" fn()) {
        swr_zend_guard_register(context, resume);
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
        swr_zend_minit_property(declared(class), name.into(), value, flags(access));
    }

    /// `zend_declare_property_string` with `access` flags.
    /// # Panics
    /// `class` is not registered.
    pub fn property_string(&self, class: &Class, name: &CStr, value: &CStr, access: u32) {
        swr_zend_minit_property_string(declared(class), name.into(), value.into(), flags(access));
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

/// An alias's arginfo: `rows` rows of a static table from `start`, its
/// return row first; none when `start` is null. Made only from static tables.
#[derive(Clone, Copy)]
struct Info {
    start: *const zend_internal_arg_info,
    rows: usize,
}

impl Info {
    const NONE: Self = Self {
        start: ptr::null(),
        rows: 0,
    };

    /// A static method table's row: its return row, then `num_args` more.
    fn row(entry: &'static zend_function_entry) -> Self {
        Self {
            start: entry.arg_info,
            rows: entry.num_args as usize + 1,
        }
    }
}

/// `class`'s entry, checking it is registered.
fn registered(class: &Class) -> *mut zend_class_entry {
    let entry = class.entry();
    assert!(!entry.is_null(), "a registered class");
    entry
}

/// `class`, checking it is registered: an internal class, which lives for the
/// process.
fn declared(class: &Class) -> ClassEntry<'static> {
    ClassEntry(registered(class), PhantomData)
}

/// `methods` as a table pointer, checking its `PHP_FE_END`.
fn terminated(methods: &'static [zend_function_entry]) -> *const zend_function_entry {
    assert!(
        methods.last().is_some_and(|end| end.fname.is_null()),
        "a method table ends with PHP_FE_END"
    );
    methods.as_ptr()
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
