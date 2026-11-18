//! MINIT's registrations, safe behind the token Zend's MINIT hands a family:
//! constants, classes and their function aliases.
use crate::{
    Class, ClassEntry, Function, Owned, Value,
    arginfo::{ArgInfo, Handler, Methods, Table},
    bailout::Bailout,
    class::Layout,
    function::deny,
    helpers::CText,
    registration::{SUCCESS, leaf},
    sys::{
        IS_STRING, ZEND_ACC_NOT_SERIALIZABLE, zend_class_entry, zend_function, zend_function_entry,
        zend_internal_arg_info, zend_long, zend_module_entry, zend_object_handlers, zend_string,
        zif_handler,
    },
};
use std::{
    ffi::{CStr, CString, c_int, c_void},
    io::IoSlice,
    marker::PhantomData,
    ptr::{self, NonNull},
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
    /// The internal function `name`'s handler, replaced by `handler`, which
    /// Zend calls as it calls any: the original, or none for no such internal
    /// function. Only [`Minit::replace_handler`] calls it, before any request.
    safe fn swr_zend_minit_replace_handler(name: CText<'_>, handler: Handler) -> zif_handler;
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
    /// The class's method `lcname` (lower case), or null: one lookup in its
    /// function table.
    safe fn swr_zend_minit_method_find(
        class: ClassEntry<'static>,
        lcname: CText<'_>,
    ) -> *mut zend_function;
    /// `zend_class_implements(class, 1, interface)`; nothing for a null one.
    safe fn swr_zend_minit_implements(class: ClassEntry<'static>, interface: Parent);
    /// The lent bytes (an `IoSlice` is an iovec) interned as a new persistent
    /// string, inside `zend_try` (nonzero for a bailout): `out` then holds the
    /// interned string, which lives for the process when interned at MINIT.
    safe fn swr_lent_intern(bytes: IoSlice<'_>, out: &mut *mut zend_string) -> c_int;
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
    /// The table lives from before MINIT to after MSHUTDOWN; only
    /// [`Minit::compiler_functions`] calls this, for the MINIT it borrows.
    safe fn swr_zend_facade_compiler_functions<'a>() -> &'a crate::Array;
    /// `zend_register_functions(NULL, entries, table, kind)` inside `zend_try`
    /// (nonzero for a bailout), `result` its result: Zend copies the alias's
    /// row, its name and arginfo into the live table.
    safe fn swr_zend_try_facade_register_functions(
        entries: &Alias<'_>,
        table: &crate::Array,
        kind: c_int,
        result: &mut c_int,
    ) -> c_int;
    /// Zend runs `handler` for each instruction of `opcode` from now on. Only
    /// [`Minit::opcode_handler`] calls it, before any PHP code runs, with a
    /// handler written for that opcode.
    safe fn zend_set_user_opcode_handler(
        opcode: u8,
        handler: Option<crate::opcode::Entry>,
    ) -> crate::sys::zend_result;
}

/// This module's MINIT, running on this thread: what makes registration safe.
/// It is neither `Send` nor `Clone`, and a family only borrows it: only
/// [`Startup::with`] lends one, for the MINIT call Zend made.
pub struct Minit {
    module: c_int,
    _thread: PhantomData<*mut ()>,
}

/// This module's MSHUTDOWN, running on the main thread after every PHP thread
/// has ended: what makes undoing MINIT's process-wide registrations safe.
/// Neither `Send` nor `Clone`, as [`Minit`]: only [`Shutdown::with`] lends one.
pub struct Mshutdown {
    _thread: PhantomData<*mut ()>,
}

/// Zend's call of one module lifecycle phase, for `'a`: the module number
/// Zend passes the phase's function (`int module_number`), branded with the
/// call. Only Zend makes one, by calling a function [`Lifecycle::install`]
/// put in a module entry's slot for the phase; safe Rust can neither build
/// one nor keep it past the call, which the function takes for any `'a`.
macro_rules! phase {
    ($($(#[$doc:meta])* $name:ident;)*) => {$(
        $(#[$doc])*
        #[derive(Clone, Copy)]
        #[repr(transparent)]
        pub struct $name<'a>(c_int, PhantomData<&'a *mut ()>);

        const _: () = assert!(size_of::<$name<'static>>() == size_of::<c_int>());
    )*};
}

phase! {
    /// Zend's MINIT call of this module ([`Startup::with`] lends its [`Minit`]).
    Startup;
    /// Zend's MSHUTDOWN call ([`Shutdown::with`] lends its [`Mshutdown`]).
    Shutdown;
    /// Zend's RINIT or RSHUTDOWN call, on the thread starting or ending a request.
    Request;
}

impl Startup<'_> {
    /// `f` with this MINIT's token, which lives only as long as `f` runs.
    pub fn with<R>(self, f: impl FnOnce(&Minit) -> R) -> R {
        f(&Minit {
            module: self.0,
            _thread: PhantomData,
        })
    }
}

impl Shutdown<'_> {
    /// `f` with this MSHUTDOWN's token, which lives only as long as `f` runs.
    pub fn with<R>(self, f: impl FnOnce(&Mshutdown) -> R) -> R {
        f(&Mshutdown {
            _thread: PhantomData,
        })
    }
}

/// Zend's MINFO call of this module, for `'a`: the live entry Zend passes the
/// module's info function. Only Zend makes one, as [`Startup`].
#[repr(transparent)]
pub struct Minfo<'a>(
    NonNull<zend_module_entry>,
    PhantomData<&'a zend_module_entry>,
);

unsafe extern "C" {
    /// `display_ini_entries(module)` inside `zend_try` (nonzero for a
    /// bailout): MINFO's table of the module's registered INI entries.
    safe fn swr_zend_try_info_ini(module: Minfo<'_>) -> c_int;
}

impl Minfo<'_> {
    /// `DISPLAY_INI_ENTRIES()`: the module's INI entries.
    /// # Errors
    /// The output bailed out.
    pub fn ini_entries(self) -> Result<(), Bailout> {
        crate::bailout::trapped(swr_zend_try_info_ini(self))
    }
}

/// A native module phase, called by Zend inside its bailout scope.
type StartupHook = unsafe extern "C" fn(c_int, Startup<'_>) -> c_int;
/// A native module phase, called by Zend inside its bailout scope.
type ShutdownHook = unsafe extern "C" fn(c_int, Shutdown<'_>) -> c_int;
/// A native module phase, called by Zend inside its bailout scope.
type RequestHook = unsafe extern "C" fn(c_int, Request<'_>) -> c_int;
/// A native module phase, called by Zend inside its bailout scope.
type InfoHook = unsafe extern "C" fn(Minfo<'_>);

/// A module's phase functions, each taking the token of Zend's call of it.
pub struct Lifecycle {
    pub startup: StartupHook,
    pub shutdown: ShutdownHook,
    pub request_startup: RequestHook,
    pub request_shutdown: RequestHook,
    pub info: InfoHook,
}

unsafe extern "C" {
    /// Stores the phase functions in the entry's slots, which Zend calls only
    /// as their phase: with the module number, which a [`Startup`],
    /// [`Shutdown`] or [`Request`] is (a transparent `c_int`), or for MINFO
    /// with the live entry, which a [`Minfo`] is (a transparent non-null pointer).
    safe fn swr_slot_module(
        entry: &mut zend_module_entry,
        startup: StartupHook,
        shutdown: ShutdownHook,
        request_startup: RequestHook,
        request_shutdown: RequestHook,
        info: InfoHook,
    );
}

impl Lifecycle {
    /// Put each function in `entry`'s slot for its phase.
    pub fn install(self, entry: &mut zend_module_entry) {
        swr_slot_module(
            entry,
            self.startup,
            self.shutdown,
            self.request_startup,
            self.request_shutdown,
            self.info,
        );
    }
}

/// A registered class another may extend or implement, which lives for the
/// process: one this module registered, or Zend's or SPL's (null only before
/// their MINIT registers them, which runs first).
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct Parent(*mut zend_class_entry);

impl Parent {
    /// No parent: Zend's null.
    pub(crate) const NONE: Self = Self(std::ptr::null_mut());
}

impl Minit {
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
    /// are interned permanently. A bailout is deferred.
    pub fn intern(&self, slot: &crate::Interned, text: &[u8]) {
        match self.interned(text) {
            Ok(string) => slot.0.store(string, std::sync::atomic::Ordering::Relaxed),
            Err(bailout) => bailout.defer(),
        }
    }

    /// `text` interned for the process, as [`Minit::intern`] interns it.
    /// # Errors
    /// A contained interning bailout.
    #[expect(
        clippy::unused_self,
        reason = "the token is what proves this runs at MINIT"
    )]
    pub(crate) fn interned(&self, text: &[u8]) -> Result<*mut zend_string, Bailout> {
        let mut string = ptr::null_mut();
        leaf(swr_lent_intern(IoSlice::new(text), &mut string))?;
        Ok(string)
    }

    /// The module number Zend gave this module.
    #[must_use]
    pub fn module(&self) -> c_int {
        self.module
    }

    /// [`Class::register_static`] of `name` with `methods`.
    /// # Panics
    /// `class` is a [`CustomClass`](crate::CustomClass)'s.
    pub fn register_static(&self, class: &Class, name: &CStr, methods: Table) {
        plain(class);
        self.register_plain(class, name, methods, None, deny);
    }

    /// The compiler's function table (`CG(function_table)`), where MINIT
    /// registers global functions.
    #[must_use]
    pub fn compiler_functions(&self) -> &crate::Array {
        swr_zend_facade_compiler_functions()
    }

    /// `SW_FUNCTION_ALIAS` of each `(method, alias)` of `class`, registered with
    /// `methods`, with that method's arginfo.
    /// # Errors
    /// Stops at a contained allocation or registration bailout.
    /// # Panics
    /// `class` is not registered.
    pub fn function_aliases<const N: usize>(
        &self,
        class: &Class,
        methods: &'static Methods<N>,
        aliases: &[(&CStr, &CStr)],
    ) -> Result<(), Bailout> {
        let entry = registered(class);
        for (method, alias) in aliases {
            let info = methods.find(method).map_or(Info::NONE, Info::row);
            // Lookup lowercases the method's ASCII bytes.
            let lower = CString::new(method.to_bytes().to_ascii_lowercase()).unwrap_or_default();
            let found = swr_zend_minit_method_find(entry, lower.as_c_str().into());
            // The class's table owns this live function, which remains live
            // and unchanged throughout the registration.
            let function = (!found.is_null()).then_some(Function(found, PhantomData));
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
            start: info.as_ptr(),
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
        let Some((entries, kind)) = Alias::new(function, alias, info) else {
            return Ok(false);
        };
        let mut result = -1;
        leaf(swr_zend_try_facade_register_functions(
            &entries,
            self.compiler_functions(),
            c_int::from(kind),
            &mut result,
        ))?;
        Ok(result == SUCCESS)
    }

    /// Register `name` with `methods` (or [`Table::NONE`]), then let `configure`
    /// adjust the entry and a copy of the standard handlers (Swoole's
    /// `SW_INIT_CLASS_ENTRY` followed by its `SW_SET_CLASS_*` macros).
    /// # Panics
    /// `class` is a [`CustomClass`](crate::CustomClass)'s.
    pub fn register(
        &self,
        class: &Class,
        name: &CStr,
        methods: Table,
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
        methods: Table,
        parent: Parent,
        configure: impl FnOnce(&mut zend_class_entry, &mut zend_object_handlers),
    ) {
        self.register_plain(class, name, methods, Some(parent), configure);
    }

    /// [`Class::register_child`] of a class that is no
    /// [`CustomClass`](crate::CustomClass)'s, with `methods`.
    fn register_plain(
        &self,
        class: &Class,
        name: &CStr,
        methods: Table,
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
        crate::registration::class_alias(name.to_bytes(), registered(class), false).map(|_| ())
    }

    /// Register `T`'s class as `name` with `methods` and `flags` added to its
    /// `ce_flags`: not serializable (unless [`Custom::serializable`](crate::Custom::serializable))
    /// and, as Swoole's custom objects are, not cloneable, with `unset` denied, its objects embedding a `T`
    /// ([`Custom`](crate::Custom)).
    /// # Panics
    /// The class is already registered.
    pub fn register_object<T: crate::Custom>(&self, name: &CStr, methods: Table, flags: u32) {
        let class = T::class();
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
        let declared = swr_zend_minit_class_constant_long(registered(class), name.into(), value);
        assert!(declared, "class constant {name:?} declared twice");
    }

    /// `zend_declare_class_constant_string`, as [`Self::class_constant_long`].
    /// # Panics
    /// `class` is not registered.
    pub fn class_constant_string(&self, class: &Class, name: &CStr, value: &CStr) {
        let declared =
            swr_zend_minit_class_constant_string(registered(class), name.into(), value.into());
        assert!(declared, "class constant {name:?} declared twice");
    }

    /// `zend_add_parameter_attribute(method, offset, SensitiveParameter)`: PHP
    /// hides argument `offset` (from 0) of `class`'s `method` (lower case) in
    /// backtraces. Nothing when the class has no such method or parameter.
    /// # Panics
    /// `class` is not registered.
    pub fn sensitive_parameter(&self, class: &Class, method: &CStr, offset: u32) {
        swr_zend_minit_sensitive_parameter(registered(class), method.into(), offset);
    }

    /// `zend::get_function(CG(function_table), name)`: the function the
    /// compiler knows as `name` (lower case), or null.
    #[must_use]
    pub fn compiled_function(&self, name: &CStr) -> *mut zend_function {
        swr_zend_compiled_function_find(name.into())
    }

    /// A copy of the compiled function `name` (lower case) that no function table
    /// names, so only a frame its owner pushes runs it; null when there is no
    /// such function. It lives for the process.
    #[must_use]
    pub fn unnamed_copy(&self, name: &CStr) -> *mut zend_function {
        let named = self.compiled_function(name);
        if named.is_null() {
            return named;
        }
        // SAFETY: a registered zend_function, which nothing writes during MINIT.
        Box::into_raw(Box::new(unsafe { named.read() }))
    }

    /// The compiled internal function `name` (lower case) runs `handler` from
    /// now on: its original handler, or none when there is no such function.
    pub fn replace_handler(&self, name: &CStr, handler: Handler) -> zif_handler {
        swr_zend_minit_replace_handler(name.into(), handler)
    }

    /// `zend_set_user_opcode_handler(H::OPCODE, handler::<H>)`: Zend runs `H`
    /// for each instruction of its opcode from now on.
    pub fn opcode_handler<H: crate::opcode::Handler>(&self) {
        if let Ok(opcode) = u8::try_from(H::OPCODE) {
            zend_set_user_opcode_handler(opcode, Some(crate::opcode::handler::<H>));
        }
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
        swr_zend_minit_property(registered(class), name.into(), value, flags(access));
    }

    /// `zend_declare_property_string` with `access` flags.
    /// # Panics
    /// `class` is not registered.
    pub fn property_string(&self, class: &Class, name: &CStr, value: &CStr, access: u32) {
        swr_zend_minit_property_string(registered(class), name.into(), value.into(), flags(access));
    }

    /// A registered class, to extend.
    /// # Panics
    /// `class` is not registered.
    #[must_use]
    pub fn parent(&self, class: &Class) -> Parent {
        Parent(registered(class).as_ptr())
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
        swr_zend_minit_implements(registered(class), interface);
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

/// An alias's table: one row named `alias`, run by a live internal function's
/// handler with that function's argument count and arginfo ([`Info`]) holding a
/// row for each, then `PHP_FE_END`. Only [`Alias::new`] makes one.
#[repr(transparent)]
struct Alias<'a>([zend_function_entry; 2], PhantomData<&'a CStr>);

impl<'a> Alias<'a> {
    /// `alias` of the internal `function`, with its kind: None without an
    /// internal function, or when `info` misses one of its parameters.
    fn new(function: Option<Function<'_>>, alias: &'a CStr, info: Info) -> Option<(Self, u8)> {
        let function = function?;
        let (kind, args) = (function.kind(), function.num_args());
        let handler = function.internal_handler()?;
        // Zend reads the return row and the `args` rows after it.
        if !info.start.is_null() && info.rows <= args as usize {
            return None;
        }
        let mut entries = crate::zeroed::<[zend_function_entry; 2]>();
        entries[0].fname = alias.as_ptr();
        entries[0].handler = handler;
        entries[0].arg_info = info.start;
        entries[0].num_args = args;
        Some((Self(entries, PhantomData), kind))
    }
}

/// `class`, checking it is registered: an internal class, which lives for the
/// process.
fn registered(class: &Class) -> ClassEntry<'static> {
    let entry = class.handle();
    assert!(entry.is_some(), "a registered class");
    entry.unwrap_or_else(|| unreachable!("asserted registered"))
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
