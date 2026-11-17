//! Internal classes registered at MINIT, with their object handlers.
use crate::{
    Minit, Parent, Value, ZStr,
    arginfo::{Rows, Table},
    helpers::CText,
    sys::{zend_class_entry, zend_long, zend_object_handlers, zval},
};
use std::{
    ffi::{CStr, CString, c_int, c_void},
    marker::PhantomData,
    ptr::{self, NonNull},
    sync::atomic::{AtomicPtr, AtomicU32, Ordering},
};

unsafe extern "C" {
    /// `INIT_CLASS_ENTRY_EX` then `zend_register_internal_class_ex`: the new
    /// entry, registered under `name` with `methods`, extending `parent`.
    /// Private, so only [`Class::register_child`] calls it, with the MINIT token.
    safe fn swr_zend_register_class(
        name: CText<'_>,
        methods: Rows,
        parent: Parent,
    ) -> *mut zend_class_entry;
    /// Zend's standard object handlers: a C `const`, initialised statically and
    /// never written.
    safe static std_object_handlers: zend_object_handlers;
    /// `instanceof_function`: it reads the two live classes' inheritance
    /// tables, and runs no PHP code.
    safe fn swr_zend_instanceof(class: ClassEntry<'_>, parent: ClassEntry<'_>) -> bool;
    /// Whether the live class's `properties_info` names `member`: it reads the
    /// table, which no PHP code changes.
    safe fn swr_zend_class_declares(class: ClassEntry<'_>, member: &ZStr) -> bool;
    /// The live class's last resolved ancestor (`ce->parent` while
    /// `ZEND_ACC_RESOLVED_PARENT` says it holds the entry, not its name), or the
    /// class itself: classes keep their parents alive, and it runs no PHP code.
    safe fn swr_zend_class_root(class: ClassEntry<'_>) -> ClassEntry<'_>;
    /// `zend_property_info::offset` of the property the live class declares as
    /// `name`, or 0 for none or a typed or static one: it reads the class's
    /// property table, which no PHP code changes, and `name`.
    safe fn swr_zend_http_property_offset(
        class: ClassEntry<'_>,
        name: crate::helpers::CText<'_>,
    ) -> u32;
    /// `array_init_size(target, size)`: `target` holds a new, empty array in
    /// place of what it held, which is not released. No PHP code runs.
    safe fn swr_zend_array_init_size(target: &Value, size: u32);
    /// `zend_lookup_class(name)` inside `zend_try` (nonzero for a bailout): it
    /// reads the name, may run an autoloader, and writes the class or null.
    safe fn swr_zend_try_lookup_class(name: &ZStr, class: &mut *mut zend_class_entry) -> c_int;
    /// `object_init_ex(target, class)` inside `zend_try` (nonzero for a
    /// bailout): `target` holds a new object of the live class (none, with an
    /// `Error` thrown, for one Zend cannot instantiate, which is a fatal error
    /// with no frame executing) in place of what it held, which is not released.
    safe fn swr_zend_try_facade_object_init(target: &crate::Owned, class: ClassEntry<'_>) -> c_int;
}

/// A live class entry, borrowed for `'a`: an object's class, which outlives it,
/// or a registered internal class, which lives for the process. Zend may change
/// the entry (its flags, its statics) whenever PHP runs, so each read borrows
/// the entry only while it copies a field out ([`ClassEntry::read`]).
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct ClassEntry<'a>(
    pub(crate) NonNull<zend_class_entry>,
    pub(crate) PhantomData<&'a zend_class_entry>,
);

impl<'a> ClassEntry<'a> {
    /// A class entry Zend holds for `'a`, or None for null: the parent a
    /// class keeps, a function's scope, a registered class.
    #[inline]
    pub(crate) fn live(entry: *mut zend_class_entry) -> Option<Self> {
        NonNull::new(entry).map(|entry| Self(entry, PhantomData))
    }

    #[must_use]
    #[inline]
    pub fn as_ptr(self) -> *mut zend_class_entry {
        self.0.as_ptr()
    }

    /// `f` of the entry, which it reads only: no PHP code runs, so nothing
    /// changes the entry, while it is borrowed.
    #[inline]
    pub(crate) fn read<R>(self, f: impl FnOnce(&zend_class_entry) -> R) -> R {
        // SAFETY: a live class entry, per the type; the borrow ends with `f`.
        f(unsafe { self.0.as_ref() })
    }

    /// `ce->name`: the class's name, which it keeps for as long as it lives.
    #[must_use]
    #[inline]
    pub fn name(self) -> &'a ZStr {
        let name = self.read(|ce| ce.name);
        // SAFETY: every class has a name, a string the class holds for `'a`;
        // `ZStr` is a transparent string.
        unsafe { &*name.cast::<ZStr>() }
    }

    /// `instanceof_function(this, parent)`: this class is `parent`, extends it
    /// or implements it.
    #[must_use]
    #[inline]
    pub fn instance_of(self, parent: ClassEntry<'_>) -> bool {
        swr_zend_instanceof(self, parent)
    }

    /// `object_init_ex` of this class, with no constructor run: the new object.
    /// Zend refuses a class it cannot instantiate, leaving no object; with no
    /// frame executing, that `Error` bails out, which is deferred.
    #[must_use]
    pub fn create_object(self) -> crate::Owned {
        let object = crate::Owned::default();
        crate::bailout::defer(crate::bailout::trapped(swr_zend_try_facade_object_init(
            &object, self,
        )));
        object
    }

    /// `ce->ce_flags` (`ZEND_ACC_*`), as they are now.
    #[must_use]
    #[inline]
    pub fn flags(self) -> u32 {
        self.read(|ce| ce.ce_flags)
    }
}

/// What lays out a [`CustomClass`](crate::CustomClass)'s objects: `T`'s class
/// and the create, free and destructor handlers that go with it. Only
/// [`Layout::of`] makes one, so a class's objects never get another type's.
#[derive(Clone, Copy)]
pub(crate) struct Layout(
    &'static Class,
    fn(&mut zend_class_entry, &mut zend_object_handlers),
);

impl Layout {
    pub(crate) fn of<T: crate::Custom>() -> Self {
        Self(T::class(), crate::object::install_custom::<T>)
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

    /// Register `name` with `methods` (or [`Table::NONE`]), extending `parent`,
    /// then let `configure` adjust the entry and a copy of the standard
    /// handlers (Swoole's `SW_INIT_CLASS_ENTRY` followed by its
    /// `SW_SET_CLASS_*` macros), and `layout` install its own.
    /// # Panics
    /// `layout` is not this class's: a [`CustomClass`](crate::CustomClass) registers with
    /// its own, any other class with none.
    pub(crate) fn register_child(
        &self,
        _minit: &Minit,
        name: &CStr,
        methods: Table,
        parent: Option<Parent>,
        layout: Option<Layout>,
        configure: impl FnOnce(&mut zend_class_entry, &mut zend_object_handlers),
    ) {
        match &layout {
            None => assert!(
                !self.custom,
                "a CustomClass registers through register_object"
            ),
            Some(layout) => assert!(ptr::eq(self, layout.0), "a CustomClass's own layout"),
        }
        let entry =
            swr_zend_register_class(name.into(), methods.rows(), parent.unwrap_or(Parent::NONE));
        let mut handlers = std_object_handlers;
        // SAFETY: Zend returned the registered entry, which nothing else uses yet.
        let registered = unsafe { &mut *entry };
        configure(registered, &mut handlers);
        if let Some(Layout(_, install)) = layout {
            install(registered, &mut handlers);
        }
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
        ClassEntry::live(entry)
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

    /// [`ClassEntry::create_object`] of this class.
    /// # Panics
    /// The class is not registered.
    #[must_use]
    pub fn create_object(&self) -> crate::Owned {
        let entry = self.class_entry();
        assert!(entry.is_some(), "create an object of a registered class");
        entry.map_or_else(crate::Owned::default, ClassEntry::create_object)
    }

    /// `zend_throw_exception(this class, message, code)`: with no frame
    /// executing, Zend reports it at once and bails out, which is deferred.
    /// # Panics
    /// The class is not registered.
    pub fn throw(&self, message: &CStr, code: zend_long) {
        assert!(self.class_entry().is_some(), "throw a registered class");
        crate::bailout::defer(crate::throw_trapped(Some(self), message, code));
    }

    /// The registered class as a handle: it lives for the process. None
    /// before MINIT registers it.
    #[must_use]
    #[inline]
    pub fn class_entry(&self) -> Option<ClassEntry<'static>> {
        ClassEntry::live(self.entry())
    }

    /// The handlers this class's objects use (null before MINIT).
    #[must_use]
    #[inline]
    pub fn handlers(&self) -> *const zend_object_handlers {
        self.handlers.load(Ordering::Acquire)
    }
}

/// `zend_lookup_class(name)` inside `zend_try` (an autoloader runs PHP code):
/// the class, or null for none. A pointer: a class PHP code declared lives only
/// until its request ends.
/// # Errors
/// An autoloader bailed out.
pub fn lookup_class(name: &ZStr) -> Result<*mut zend_class_entry, crate::bailout::Bailout> {
    let mut class = ptr::null_mut();
    crate::bailout::trapped(swr_zend_try_lookup_class(name, &mut class))?;
    Ok(class)
}

impl<'a> crate::Php<'a> {
    /// [`lookup_class`], borrowed: the running request keeps the class it
    /// found while PHP runs, and a class outlives the request's values.
    /// # Errors
    /// An autoloader bailed out.
    pub fn lookup_class(
        self,
        name: &ZStr,
    ) -> Result<Option<ClassEntry<'a>>, crate::bailout::Bailout> {
        lookup_class(name).map(ClassEntry::live)
    }
}

impl crate::Value {
    /// Whether this holds an instance of the class named `name`, which is
    /// looked up first ([`lookup_class`]): None for no such class.
    /// # Errors
    /// An autoloader bailed out.
    pub fn instance_of_named(&self, name: &ZStr) -> Result<Option<bool>, crate::bailout::Bailout> {
        // The class just looked up, read before any PHP code runs.
        let Some(class) = ClassEntry::live(lookup_class(name)?) else {
            return Ok(None);
        };
        Ok(Some(
            self.class_entry().is_some_and(|own| own.instance_of(class)),
        ))
    }
}

/// The run-time cache slot Zend passes an `unset_property` handler with the
/// property's name, for Zend's own handler. Rust never makes one: it is the
/// handler's parameter, which Zend's call stands for.
#[repr(transparent)]
struct Hint(*mut *mut c_void);

unsafe extern "C" {
    /// `handlers->unset_property = unset`, which Zend calls with a live object
    /// whose class has these handlers, a live member name and its cache slot.
    safe fn swr_slot_unset_property(
        handlers: &mut zend_object_handlers,
        unset: extern "C" fn(crate::ObjectRef<'_>, &ZStr, Hint),
    );
    /// `std_object_handlers.unset_property`: Zend's own handler, given a live
    /// object and member name, and the cache slot Zend's call passed with
    /// them, which only [`unset_property_deny`] holds (a [`Hint`]).
    safe fn zend_std_unset_property(object: crate::ObjectRef<'_>, name: &ZStr, cache_slot: Hint);
}

/// `SW_SET_CLASS_UNSET_PROPERTY_HANDLER(.., sw_zend_class_unset_property_deny)`.
pub fn deny_unset(handlers: &mut zend_object_handlers) {
    swr_slot_unset_property(handlers, unset_property_deny);
}

/// Swoole's `sw_zend_class_unset_property_deny`: declared properties of an
/// internal class cannot be unset; anything else unsets as usual. Installed as
/// `unset_property` ([`deny_unset`]), so Zend passes a live object and member name.
extern "C" fn unset_property_deny(object: crate::ObjectRef<'_>, name: &ZStr, cache_slot: Hint) {
    let own = object.class_entry();
    // The class the object's own inherits its declared properties from.
    if swr_zend_class_declares(swr_zend_class_root(own), name) {
        // Each name up to its NUL, as upstream's `%s` prints it.
        let message = [
            &b"Property "[..],
            name.c_str().to_bytes(),
            b" of class ",
            own.name().c_str().to_bytes(),
            b" cannot be unset",
        ]
        .concat();
        crate::throw_error(&CString::new(message).unwrap_or_default());
        return;
    }
    zend_std_unset_property(object, name, cache_slot);
}

/// An untyped instance property a registered class declares, as the offset of
/// its slot (`OBJ_PROP`) in every object of the class or of a subclass, which
/// keeps its parent's slots and may not type them. Found once in MINIT, then
/// read without a name lookup. Untyped, the slot takes any value.
pub struct Declared {
    class: AtomicPtr<zend_class_entry>,
    offset: AtomicU32,
}

impl Default for Declared {
    fn default() -> Self {
        Self::new()
    }
}

impl Declared {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            class: AtomicPtr::new(ptr::null_mut()),
            offset: AtomicU32::new(0),
        }
    }

    /// MINIT, once `class` is registered: its property `name`. Without such
    /// an untyped instance property, [`Declared::get`] finds none.
    pub fn resolve(&self, class: &Class, name: &CStr) {
        let Some(entry) = class.handle() else { return };
        let offset = swr_zend_http_property_offset(entry, name.into());
        self.offset.store(offset, Ordering::Relaxed);
        self.class.store(entry.as_ptr(), Ordering::Release);
    }

    /// The slot's offset; 0 for none, or before MINIT.
    #[must_use]
    #[inline]
    pub fn offset(&self) -> u32 {
        self.offset.load(Ordering::Relaxed)
    }

    /// `OBJ_PROP(Z_OBJ_P(object), offset)`: the slot in `object`, an object of
    /// the class or of a subclass; None for any other value, or before MINIT.
    #[must_use]
    #[inline]
    pub fn get<'a>(&self, object: &'a Value) -> Option<&'a Value> {
        let std = object.object()?;
        let own = object.class_entry()?;
        let offset = self.offset();
        if offset == 0 {
            return None;
        }
        let class = ClassEntry::live(self.class.load(Ordering::Acquire))?;
        if own.as_ptr() != class.as_ptr() && !own.instance_of(class) {
            return None;
        }
        let slot = std.wrapping_byte_add(offset as usize).cast::<zval>();
        // SAFETY: an object of the class that declares the property at this
        // offset (a registered class, so not null), which keeps the slot while
        // `object` holds it; `Value` is a transparent zval.
        unsafe { Value::from_ptr(slot) }
    }

    /// `ZVAL_COPY_VALUE` of `value` into `object`'s slot (see
    /// [`Declared::get`]), which takes its reference, as
    /// `zend_update_property` does once it has found the slot, but without
    /// releasing what the slot held: for a fresh object, whose default (a
    /// number, null or an interned string) needs no release. Without a slot,
    /// `value` is dropped.
    #[inline]
    pub fn init(&self, object: &Value, value: crate::Owned) {
        if let Some(slot) = self.get(object) {
            // The slot takes any value, and holds nothing to release.
            slot.put(value);
        }
    }

    /// `array_init_size` in `object`'s slot (see [`Declared::get`]), which
    /// is not released first: the new array, or None without a slot.
    #[inline]
    pub fn init_array<'a>(&self, object: &'a Value, size: u32) -> Option<&'a Value> {
        let slot = self.get(object)?;
        swr_zend_array_init_size(slot, size);
        Some(slot)
    }
}
