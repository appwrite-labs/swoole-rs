//! Internal classes registered at MINIT, with their object handlers.
use crate::{
    ZStr,
    sys::{
        HashTable, ZEND_ACC_RESOLVED_PARENT, swr_zend_http_property_offset,
        swr_zend_register_class, zend_class_entry, zend_function_entry, zend_hash_find, zend_long,
        zend_object, zend_object_handlers, zend_string, zend_throw_error, zend_throw_exception,
    },
};
use std::{
    ffi::{CStr, c_char, c_void},
    marker::PhantomData,
    mem::offset_of,
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
};

unsafe extern "C" {
    /// Zend's standard object handlers: a C `const`, initialised statically and
    /// never written.
    safe static std_object_handlers: zend_object_handlers;
}

/// A live class entry, borrowed for `'a`: an object's class, which outlives it,
/// or a registered internal class, which lives for the process. Zend may change
/// the entry (its flags, its statics) at any time, so each read takes one field
/// through the pointer and no reference to the entry is ever made.
#[derive(Clone, Copy)]
pub struct ClassEntry<'a>(
    pub(crate) *mut zend_class_entry,
    pub(crate) PhantomData<&'a zend_class_entry>,
);

impl<'a> ClassEntry<'a> {
    /// The class at `entry`, or None for null.
    /// # Safety
    /// `entry` is null or a class entry that lives for `'a`.
    #[must_use]
    #[inline]
    pub unsafe fn from_ptr(entry: *mut zend_class_entry) -> Option<Self> {
        (!entry.is_null()).then_some(Self(entry, PhantomData))
    }

    #[must_use]
    #[inline]
    pub fn as_ptr(self) -> *mut zend_class_entry {
        self.0
    }

    /// `ce->name`: the class's name, which it keeps for as long as it lives.
    #[must_use]
    #[inline]
    pub fn name(self) -> &'a ZStr {
        // SAFETY: a live class entry, per the type.
        let name = unsafe { (*self.0).name };
        // SAFETY: every class has a name, a string the class holds for `'a`;
        // `ZStr` is a transparent string.
        unsafe { &*name.cast::<ZStr>() }
    }

    /// `ce->ce_flags` (`ZEND_ACC_*`), as they are now.
    #[must_use]
    #[inline]
    pub fn flags(self) -> u32 {
        // SAFETY: a live class entry, per the type.
        unsafe { (*self.0).ce_flags }
    }

    /// `ce->parent`: the class this one extends, once Zend resolved it
    /// (`ZEND_ACC_RESOLVED_PARENT`: every internal class and every linked user
    /// class, so the class of any object); None for no parent, or one still
    /// held by name.
    #[must_use]
    #[inline]
    pub fn parent(self) -> Option<Self> {
        if self.flags() & ZEND_ACC_RESOLVED_PARENT == 0 {
            return None;
        }
        // The union of the parent's entry and, until it is resolved, its name:
        // two pointers, either of which starts it.
        let slot = self
            .0
            .wrapping_byte_add(offset_of!(zend_class_entry, __bindgen_anon_1))
            .cast::<*mut zend_class_entry>();
        // SAFETY: a field of a live class entry; the flag says it holds the
        // parent's entry.
        let parent = unsafe { slot.read() };
        // A class keeps its parent alive.
        (!parent.is_null()).then_some(Self(parent, PhantomData))
    }
}

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
        let mut handlers = std_object_handlers;
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

    /// The registered class; None before MINIT registers it.
    #[must_use]
    pub fn handle(&self) -> Option<ClassEntry<'static>> {
        let entry = self.entry();
        // A registered internal class lives for the process.
        (!entry.is_null()).then_some(ClassEntry(entry, PhantomData))
    }

    /// `ZSTR_VAL(ce->name)`: the name the class registered under (up to a
    /// NUL); None before MINIT registers it.
    #[must_use]
    pub fn name(&self) -> Option<&CStr> {
        Some(self.handle()?.name().c_str())
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
    // SAFETY: a live object, per Zend.
    let own = unsafe { crate::Props::new(ptr::null_mut(), object) }.class_entry();
    // The class the object's own inherits its declared properties from.
    let mut root = own;
    while let Some(parent) = root.parent() {
        root = parent;
    }
    let declared = root
        .as_ptr()
        .wrapping_byte_add(offset_of!(zend_class_entry, properties_info))
        .cast::<HashTable>();
    // SAFETY: a live class's property table and the member name.
    if !unsafe { zend_hash_find(declared, member) }.is_null() {
        // SAFETY: a printf format whose two `%s` take the NUL-terminated member
        // and class names.
        unsafe {
            zend_throw_error(
                ptr::null_mut(),
                c"Property %s of class %s cannot be unset".as_ptr(),
                text(member),
                text(own.name().as_ptr()),
            );
        }
        return;
    }
    if let Some(unset) = std_object_handlers.unset_property {
        // SAFETY: forwarding Zend's own arguments.
        unsafe { unset(object, member, cache_slot) };
    }
}

/// `ZSTR_VAL(Z_OBJCE_P(object)->name)`.
/// # Safety
/// `object` is live; the name lives as long as its class.
#[must_use]
pub unsafe fn class_name<'a>(object: *const zend_object) -> &'a CStr {
    // SAFETY: a live object, per the caller.
    let props = unsafe { crate::Props::new(ptr::null_mut(), object.cast_mut()) };
    props.class_entry().name().c_str()
}

/// `ZSTR_VAL`: a `zend_string`'s NUL-terminated bytes.
fn text(string: *const zend_string) -> *const c_char {
    string
        .wrapping_byte_add(offset_of!(zend_string, val))
        .cast()
}
