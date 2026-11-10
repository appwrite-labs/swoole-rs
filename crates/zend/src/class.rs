//! Internal classes registered at MINIT, with their object handlers.
use crate::sys::{
    CONST_PERSISTENT, ZEND_ACC_PUBLIC, std_object_handlers, swr_zend_register_class,
    zend_class_entry, zend_declare_class_constant_long, zend_declare_property_bool,
    zend_declare_property_long, zend_declare_property_null, zend_declare_property_string,
    zend_function_entry, zend_hash_find, zend_long, zend_object, zend_object_handlers,
    zend_register_double_constant, zend_register_long_constant, zend_string, zend_throw_error,
};
use std::{
    ffi::{CStr, c_char, c_void},
    mem::offset_of,
    ptr,
    sync::{
        OnceLock,
        atomic::{AtomicPtr, Ordering},
    },
};

/// A class entry and the object handlers its objects share: process-global,
/// written once at MINIT and read-only afterwards.
pub struct Class {
    entry: AtomicPtr<zend_class_entry>,
    handlers: OnceLock<zend_object_handlers>,
}

impl Default for Class {
    fn default() -> Self {
        Self::new()
    }
}

impl Class {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entry: AtomicPtr::new(ptr::null_mut()),
            handlers: OnceLock::new(),
        }
    }

    /// Register `name` with `methods`, then let `configure` adjust the entry and a
    /// copy of the standard handlers (Swoole's `SW_INIT_CLASS_ENTRY` followed by
    /// its `SW_SET_CLASS_*` macros).
    /// # Safety
    /// Called once, at MINIT. `methods` is a static, `PHP_FE_END`-terminated
    /// table, such as one built from a stub's generated arginfo.
    pub unsafe fn register(
        &self,
        name: &CStr,
        methods: *const zend_function_entry,
        configure: impl FnOnce(&mut zend_class_entry, &mut zend_object_handlers),
    ) {
        // SAFETY: per the caller.
        unsafe { self.register_child(name, methods, ptr::null_mut(), configure) };
    }

    /// `register`, extending `parent` (null for none).
    /// # Safety
    /// As `register`; `parent` is null or a registered class. A
    /// [`CustomClass`](crate::CustomClass) registers only through
    /// [`Minit::register_object`](crate::Minit::register_object).
    pub unsafe fn register_child(
        &self,
        name: &CStr,
        methods: *const zend_function_entry,
        parent: *mut zend_class_entry,
        configure: impl FnOnce(&mut zend_class_entry, &mut zend_object_handlers),
    ) {
        // SAFETY: MINIT, with a NUL-terminated name and the caller's static table.
        let entry =
            unsafe { swr_zend_register_class(name.as_ptr(), name.count_bytes(), methods, parent) };
        // SAFETY: the standard handlers are initialised before any extension's MINIT.
        let mut handlers = unsafe { std_object_handlers };
        // SAFETY: Zend returned the registered entry, which nothing else uses yet.
        configure(unsafe { &mut *entry }, &mut handlers);
        self.handlers.get_or_init(|| handlers);
        self.entry.store(entry, Ordering::Release);
    }

    /// The registered class entry (null before MINIT).
    #[must_use]
    pub fn entry(&self) -> *mut zend_class_entry {
        self.entry.load(Ordering::Acquire)
    }

    /// `zend_declare_class_constant_long`.
    /// # Safety
    /// At MINIT, after `register`.
    pub unsafe fn constant_long(&self, name: &CStr, value: zend_long) {
        // SAFETY: per the caller; Zend copies the name.
        unsafe {
            zend_declare_class_constant_long(
                self.entry(),
                name.as_ptr(),
                name.count_bytes(),
                value,
            );
        }
    }

    /// `zend_declare_property_long(..., ZEND_ACC_PUBLIC)`.
    /// # Safety
    /// At MINIT, after `register`.
    pub unsafe fn property_long(&self, name: &CStr, value: zend_long) {
        #[expect(clippy::cast_possible_wrap, reason = "a small flag")]
        // SAFETY: per the caller; Zend copies the name.
        unsafe {
            zend_declare_property_long(
                self.entry(),
                name.as_ptr(),
                name.count_bytes(),
                value,
                ZEND_ACC_PUBLIC as i32,
            );
        }
    }

    /// `zend_declare_property_null(..., ZEND_ACC_PUBLIC)`.
    /// # Safety
    /// At MINIT, after `register`.
    pub unsafe fn property_null(&self, name: &CStr) {
        #[expect(clippy::cast_possible_wrap, reason = "a small flag")]
        // SAFETY: per the caller; Zend copies the name.
        unsafe {
            zend_declare_property_null(
                self.entry(),
                name.as_ptr(),
                name.count_bytes(),
                ZEND_ACC_PUBLIC as i32,
            );
        }
    }

    /// Swoole's `SW_CLASS_ALIAS`: `name` (no leading backslash) names this class too.
    /// # Safety
    /// At MINIT, after `register`.
    /// # Errors
    /// Returns a contained PHP allocation or alias registration bailout.
    pub unsafe fn alias(&self, name: &CStr) -> Result<(), crate::bailout::Bailout> {
        // SAFETY: per the caller; the Rust registry owns the full alias construction.
        unsafe {
            crate::registration::class_alias(name.as_ptr(), name.count_bytes(), self.entry(), false)
        }
        .map(|_| ())
    }

    /// `object_init_ex` of this class, then its `__construct(args)` (Swoole's
    /// `sw_zend_call_method_with_N_params`, at most two): the new object.
    /// # Errors
    /// The constructor bailed out.
    /// # Panics
    /// The class is not registered.
    pub fn instantiate(
        &self,
        args: &[&crate::Value],
    ) -> Result<crate::Owned, crate::bailout::Bailout> {
        let entry = self.entry();
        assert!(!entry.is_null(), "instantiate a registered class");
        let object = crate::Owned::default();
        // SAFETY: a slot this owns and a registered class; Zend refuses one it
        // cannot instantiate, leaving no object.
        unsafe { crate::init_object(object.as_ptr(), entry) };
        let Some(std) = object.object() else {
            return Ok(object);
        };
        let args: Vec<_> = args.iter().map(|arg| arg.as_ptr()).collect();
        // SAFETY: the new object, of this class, and live arguments.
        unsafe { crate::bailout::call_method(std, entry, b"__construct", ptr::null_mut(), &args) }?;
        Ok(object)
    }

    /// `zend_throw_exception(this class, message, code)`, from a handler.
    /// # Panics
    /// The class is not registered.
    pub fn throw(&self, message: &CStr, code: zend_long) {
        let entry = self.entry();
        assert!(!entry.is_null(), "throw a registered class");
        // SAFETY: a registered exception class; Zend copies the message.
        unsafe { crate::throw_exception(entry, message, code) };
    }

    /// The handlers this class's objects use (null before MINIT).
    #[must_use]
    #[inline]
    pub fn handlers(&self) -> *const zend_object_handlers {
        self.handlers.get().map_or(ptr::null(), ptr::from_ref)
    }
}

/// `ZEND_ACC_*` flags as the `int` the declare functions take.
fn flags(value: u32) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

/// `zend_declare_property_long` with `access` flags.
/// # Safety
/// MINIT, with a registered class.
pub unsafe fn declare_long(ce: *mut zend_class_entry, name: &CStr, value: zend_long, access: u32) {
    // SAFETY: per the caller.
    unsafe {
        zend_declare_property_long(ce, name.as_ptr(), name.count_bytes(), value, flags(access));
    }
}

/// `zend_declare_property_string` with `access` flags.
/// # Safety
/// MINIT, with a registered class.
pub unsafe fn declare_string(ce: *mut zend_class_entry, name: &CStr, value: &CStr, access: u32) {
    // SAFETY: per the caller; Zend copies the value.
    unsafe {
        zend_declare_property_string(
            ce,
            name.as_ptr(),
            name.count_bytes(),
            value.as_ptr(),
            flags(access),
        );
    }
}

/// `zend_declare_property_bool` with `access` flags.
/// # Safety
/// MINIT, with a registered class.
pub unsafe fn declare_bool(ce: *mut zend_class_entry, name: &CStr, value: bool, access: u32) {
    // SAFETY: per the caller.
    unsafe {
        zend_declare_property_bool(
            ce,
            name.as_ptr(),
            name.count_bytes(),
            zend_long::from(value),
            flags(access),
        );
    }
}

/// `SW_REGISTER_LONG_CONSTANT`: a persistent global constant.
/// # Safety
/// At MINIT, with the extension's module number.
pub unsafe fn register_long_constant(name: &CStr, value: zend_long, module: i32) {
    #[expect(clippy::cast_possible_wrap, reason = "a small flag")]
    // SAFETY: per the caller; Zend copies the name.
    unsafe {
        zend_register_long_constant(
            name.as_ptr(),
            name.count_bytes(),
            value,
            CONST_PERSISTENT as i32,
            module,
        );
    }
}

/// `SW_REGISTER_DOUBLE_CONSTANT`: a persistent global constant.
/// # Safety
/// At MINIT, with the extension's module number.
pub unsafe fn register_double_constant(name: &CStr, value: f64, module: i32) {
    #[expect(clippy::cast_possible_wrap, reason = "a small flag")]
    // SAFETY: per the caller; Zend copies the name.
    unsafe {
        zend_register_double_constant(
            name.as_ptr(),
            name.count_bytes(),
            value,
            CONST_PERSISTENT as i32,
            module,
        );
    }
}

/// Swoole's `sw_zend_class_unset_property_deny`: declared properties of an
/// internal class cannot be unset; anything else unsets as usual.
/// # Safety
/// Installed as `unset_property`, so Zend passes a live object and member name.
pub unsafe extern "C" fn unset_property_deny(
    object: *mut zend_object,
    member: *mut zend_string,
    cache_slot: *mut *mut c_void,
) {
    // SAFETY: a live object.
    let class = unsafe { (*object).ce };
    // SAFETY: class entries live for the process.
    let mut root = unsafe { &*class };
    loop {
        // SAFETY: an internal class's union holds its parent entry.
        let parent = unsafe { root.__bindgen_anon_1.parent };
        if parent.is_null() {
            break;
        }
        // SAFETY: as above.
        root = unsafe { &*parent };
    }
    // SAFETY: a live hash table and member name.
    if !unsafe { zend_hash_find(&raw const root.properties_info, member) }.is_null() {
        // SAFETY: the class name of a live object.
        let name = unsafe { (*class).name };
        // SAFETY: a printf format whose two `%s` take the NUL-terminated member
        // and class names.
        unsafe {
            zend_throw_error(
                ptr::null_mut(),
                c"Property %s of class %s cannot be unset".as_ptr(),
                text(member),
                text(name),
            );
        }
        return;
    }
    // SAFETY: the standard handlers are initialised and immutable.
    if let Some(unset) = unsafe { std_object_handlers.unset_property } {
        // SAFETY: forwarding Zend's own arguments.
        unsafe { unset(object, member, cache_slot) };
    }
}

/// `ZSTR_VAL(Z_OBJCE_P(object)->name)`.
/// # Safety
/// `object` is live; the name lives as long as its class.
#[must_use]
pub unsafe fn class_name<'a>(object: *const zend_object) -> &'a CStr {
    // SAFETY: a live object.
    let class = unsafe { (*object).ce };
    // SAFETY: class entries live for the process.
    let name = unsafe { (*class).name };
    // SAFETY: a class name is a NUL-terminated zend_string.
    unsafe { CStr::from_ptr(text(name)) }
}

/// `ZSTR_VAL`: a `zend_string`'s NUL-terminated bytes.
fn text(string: *const zend_string) -> *const c_char {
    string
        .wrapping_byte_add(offset_of!(zend_string, val))
        .cast()
}
