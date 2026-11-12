//! Internal classes registered at MINIT, with their object handlers.
use crate::sys::{
    std_object_handlers, swr_zend_http_property_offset, swr_zend_register_class, zend_class_entry,
    zend_function_entry, zend_hash_find, zend_long, zend_object, zend_object_handlers, zend_string,
    zend_throw_error, zend_throw_exception,
};
use std::{
    ffi::{CStr, c_char, c_void},
    mem::offset_of,
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
};

/// A class entry and the object handlers its objects share: process-global,
/// written once at MINIT and read-only afterwards.
pub struct Class {
    entry: AtomicPtr<zend_class_entry>,
    /// Leaked at the first registration: a handler's object check is one load.
    handlers: AtomicPtr<zend_object_handlers>,
    /// A [`CustomClass`](crate::CustomClass)'s, whose objects only
    /// [`Minit::register_object`](crate::Minit::register_object)'s handlers lay out.
    pub(crate) custom: bool,
}

impl Default for Class {
    fn default() -> Self {
        Self::new()
    }
}

impl Class {
    #[must_use]
    pub const fn new() -> Self {
        Self::of(false)
    }

    /// A class, a [`CustomClass`](crate::CustomClass)'s when `custom`.
    pub(crate) const fn of(custom: bool) -> Self {
        Self {
            entry: AtomicPtr::new(ptr::null_mut()),
            handlers: AtomicPtr::new(ptr::null_mut()),
            custom,
        }
    }

    /// Register `name` with `methods`, extending `parent` (null for none), then
    /// let `configure` adjust the entry and a copy of the standard handlers
    /// (Swoole's `SW_INIT_CLASS_ENTRY` followed by its `SW_SET_CLASS_*` macros).
    /// # Safety
    /// Called at MINIT. `methods` is null or a static, `PHP_FE_END`-terminated
    /// table, such as one built from a stub's generated arginfo; `parent` is
    /// null or a registered class. A [`CustomClass`](crate::CustomClass)
    /// registers only through [`Minit::register_object`](crate::Minit::register_object).
    pub(crate) unsafe fn register_child(
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
        if self.handlers.load(Ordering::Acquire).is_null() {
            self.handlers
                .store(Box::leak(Box::new(handlers)), Ordering::Release);
        }
        self.entry.store(entry, Ordering::Release);
    }

    /// The registered class entry (null before MINIT).
    #[must_use]
    pub fn entry(&self) -> *mut zend_class_entry {
        self.entry.load(Ordering::Acquire)
    }

    /// `object_init_ex` of this class, then its `__construct(args)` (Swoole's
    /// `sw_zend_call_method_with_N_params`, at most two): the new object.
    /// # Errors
    /// The constructor bailed out.
    /// # Panics
    /// The class is not registered, or more than two arguments.
    pub fn instantiate(
        &self,
        args: &[&crate::Value],
    ) -> Result<crate::Owned, crate::bailout::Bailout> {
        let object = self.create_object();
        drop(object.call_method(Some(self), b"__construct", args)?);
        Ok(object)
    }

    /// `object_init_ex` of this class, with no constructor run: the new object.
    /// # Panics
    /// The class is not registered.
    #[must_use]
    pub fn create_object(&self) -> crate::Owned {
        let entry = self.entry();
        assert!(!entry.is_null(), "create an object of a registered class");
        let object = crate::Owned::default();
        // SAFETY: a slot this owns and a registered class; Zend refuses one it
        // cannot instantiate, leaving no object.
        unsafe { crate::init_object(object.as_ptr(), entry) };
        object
    }

    /// `zend_throw_exception(this class, message, code)`, from a handler.
    /// # Panics
    /// The class is not registered.
    pub fn throw(&self, message: &CStr, code: zend_long) {
        let entry = self.entry();
        assert!(!entry.is_null(), "throw a registered class");
        // SAFETY: a registered exception class; Zend copies the message.
        unsafe { zend_throw_exception(entry, message.as_ptr(), code) };
    }

    /// The offset of the slot of the instance property this class declares as
    /// `name` (`zend_property_info::offset`); 0 for no such property, or before
    /// MINIT registers the class.
    #[must_use]
    pub fn property_offset(&self, name: &CStr) -> u32 {
        let entry = self.entry();
        if entry.is_null() {
            return 0;
        }
        // SAFETY: a registered class, which lives for the process; Zend reads
        // its property table and `name` only.
        unsafe { swr_zend_http_property_offset(entry, name.as_ptr(), name.count_bytes()) }
    }

    /// The handlers this class's objects use (null before MINIT).
    #[must_use]
    #[inline]
    pub fn handlers(&self) -> *const zend_object_handlers {
        self.handlers.load(Ordering::Acquire)
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
    let own = unsafe { &*class };
    let mut root = own;
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
        let name = own.name;
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
