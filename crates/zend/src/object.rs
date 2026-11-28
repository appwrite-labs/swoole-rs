//! Objects that embed their `zend_object` after Rust data, as Swoole's C++
//! structs do (`struct { T data; zend_object std; }`).
use crate::{
    ArrayMut, Class, ClassEntry, Declared, Owned, OwnedArray, Value, ZString, bailout,
    sys::{
        IS_OBJECT, zend_class_entry, zend_long, zend_object, zend_object_handlers,
        zend_objects_destroy_object, zend_update_property,
    },
};
use std::{
    ffi::{CStr, c_char, c_int},
    io::IoSlice,
    marker::PhantomData,
    mem::offset_of,
    ptr::NonNull,
};

unsafe extern "C" {
    /// `zend_object_alloc(size, ce)`, then `zend_object_std_init`,
    /// `object_properties_init` and `handlers`, on the `zend_object` `offset`
    /// bytes in: a new object, with its one reference, never null.
    fn swr_zend_object_new(
        size: usize,
        offset: usize,
        ce: *mut zend_class_entry,
        handlers: *const zend_object_handlers,
    ) -> Fresh;
    /// `sw_zend_read_property_ex` without its copy: null (`None`), or the slot
    /// of the instance property the live object's class declares as the lent
    /// bytes (an `IoSlice` is an iovec), which lives as long as the object.
    /// No PHP code runs.
    safe fn swr_lent_property_slot<'a>(
        object: ObjectRef<'a>,
        name: IoSlice<'_>,
    ) -> Option<&'a Value>;
    /// `zend_update_property(scope, object, name, value)` inside `zend_try`
    /// (nonzero for a bailout): a live object, under its own class, a class it
    /// sees, or none; Zend reads the lent name and copies `value` first, so the
    /// PHP code the write runs can't free what it reads.
    safe fn swr_lent_update_property(
        scope: Option<ClassEntry<'_>>,
        object: ObjectRef<'_>,
        name: IoSlice<'_>,
        value: &Value,
    ) -> c_int;
    /// Unlink string entries in the array the slot holds (or references) and
    /// remove them from `SG(rfc1867_uploaded_files)`. No PHP code runs.
    safe fn swr_zend_http_uploads_release(tmpfiles: &Value);
}

/// A live object, borrowed for `'a`: one a zval or frame holds, or one Zend
/// passes a handler. Zend changes it (its refcount, its properties) whenever PHP
/// runs, so each read borrows it only while it copies a field out
/// ([`ObjectRef::read`]).
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct ObjectRef<'a>(
    pub(crate) *mut zend_object,
    pub(crate) PhantomData<&'a zend_object>,
);

impl<'a> ObjectRef<'a> {
    #[must_use]
    #[inline]
    pub fn as_ptr(self) -> *mut zend_object {
        self.0
    }

    /// `f` of the object, which it reads only: no PHP code runs, so nothing
    /// changes the object, while it is borrowed.
    #[inline]
    pub(crate) fn read<R>(self, f: impl FnOnce(&zend_object) -> R) -> R {
        // SAFETY: ObjectRef points to a live zend_object for the duration of `f`.
        f(unsafe { &*self.0 })
    }

    /// `obj->handle`: its slot in the executor's object store.
    #[must_use]
    #[inline]
    pub fn handle(self) -> u32 {
        self.read(|object| object.handle)
    }

    /// `obj->handlers`.
    #[must_use]
    #[inline]
    pub fn handlers(self) -> *const zend_object_handlers {
        self.read(|object| object.handlers)
    }

    /// `obj->ce`: the object's class, which outlives it.
    #[must_use]
    #[inline]
    pub fn class_entry(self) -> ClassEntry<'a> {
        match ClassEntry::live(self.read(|object| object.ce)) {
            Some(class) => class,
            None => classless(),
        }
    }

    /// `ZVAL_OBJ`: a value naming the object without a reference of its own,
    /// for a slot that must not outlive the borrow or release it, such as a
    /// call's argument array (Zend copies each argument, with a reference, into
    /// the frame it calls). Nothing here reads the object.
    #[must_use]
    #[inline]
    pub(crate) fn copy_value(self) -> Value {
        Value::naming(self)
    }

    /// Lend this live object as a value without taking another reference.
    /// The temporary slot cannot escape the call or outlive the object borrow.
    ///
    /// ```compile_fail
    /// use swoole_rs_zend::{ObjectRef, Value};
    /// fn escape<'a>(object: ObjectRef<'a>) -> &'a Value {
    ///     object.with_value(|value| value)
    /// }
    /// ```
    #[inline]
    pub fn with_value<R>(self, call: impl FnOnce(&Value) -> R) -> R {
        call(&self.copy_value())
    }

    /// `T`'s data in the object, when it is one of `T`'s objects: made by
    /// [`create`] or [`new_object`], whose handlers are `T`'s class's.
    #[must_use]
    #[inline]
    pub fn custom<T: Custom>(self) -> Option<&'a T> {
        if self.handlers() != T::class().handlers() {
            return None;
        }
        // SAFETY: only `create::<T>` and `new_object::<T>` give an object the
        // handlers `register_object::<T>` made, after writing its data; only
        // `free::<T>` drops it, when no reference to the object (so none to its
        // data) remains. Handlers are null only before registration.
        Some(unsafe { &*Object::<T>::data_ptr(self.as_ptr()) })
    }
}

/// `data` followed by the object Zend sees; `std` must stay last, since Zend
/// allocates the class's property slots after it. Zend finds the allocation
/// from `std` using the offset registered for this particular `T`.
// `repr(C)`: Zend allocates the object's property slots after `std`, which must stay last (`zend_object_alloc`).
#[repr(C)]
pub struct Object<T> {
    pub data: T,
    pub std: zend_object,
}

impl<T> Object<T> {
    /// Where `std` sits: the handlers' `offset`.
    pub const OFFSET: usize = offset_of!(Self, std);

    /// Where the data of the object around `std` sits, for callers that keep
    /// no reference to it across PHP code (cells, or raw field access).
    #[must_use]
    #[inline]
    pub fn data_ptr(std: *mut zend_object) -> *mut T {
        // `data` leads the `repr(C)` struct, `OFFSET` bytes before `std`.
        std.wrapping_byte_sub(Self::OFFSET).cast()
    }

    #[must_use]
    pub fn std(this: *mut Self) -> *mut zend_object {
        this.wrapping_byte_add(Self::OFFSET).cast()
    }
}

/// `T`'s registered class, or a subclass of it that inherited its create
/// handler: an entry whose objects are `Object<T>`s with `T`'s handlers. Made
/// only by [`new_object`], from `T`'s own registered entry, and by [`create`],
/// from the entry Zend passes `T`'s create handler.
struct LaidOut<T>(*mut zend_class_entry, PhantomData<fn() -> T>);

impl<T: Custom> LaidOut<T> {
    /// The body of a `create_object` handler: allocate, initialise the
    /// standard object and its properties, install `T`'s class's handlers,
    /// and write `data` before the object is shared.
    #[must_use]
    fn create(self, data: T) -> Fresh {
        let Self(ce, _) = self;
        let (size, offset) = (size_of::<Object<T>>(), Object::<T>::OFFSET);
        // SAFETY: an `Object<T>` laid out for class `ce`, its `zend_object`
        // last, `offset` bytes in, with `T`'s class's handlers.
        let std = unsafe { swr_zend_object_new(size, offset, ce, T::class().handlers()) };
        // SAFETY: the fresh zend_object is embedded in Object<T> with an uninitialized data slot.
        unsafe { Object::<T>::data_ptr(std.0.as_ptr()).write(data) };
        std
    }
}

/// An object `swr_zend_object_new` just made, with its one reference, which
/// whoever takes it holds. Only that call makes one.
#[repr(transparent)]
pub(crate) struct Fresh(NonNull<zend_object>);

impl Fresh {
    /// The object, giving up its reference to the caller.
    pub(crate) fn into_ptr(self) -> *mut zend_object {
        self.0.as_ptr()
    }
}

/// The Rust data a class's objects embed (Swoole's `SW_SET_CLASS_CUSTOM_OBJECT`),
/// with no `unsafe` in the family crate: [`Minit::register_object`](crate::Minit::register_object)
/// installs create and free handlers that make each object's data with
/// [`Custom::new`] and drop it with the object, and [`Call::custom`](crate::Call::custom)
/// and [`Value::custom`](crate::Value::custom) borrow it. Mutable state goes in
/// `Cell`s or `OnceCell`s: methods, and PHP code they run, share the data.
pub trait Custom: Sized + 'static {
    /// Enumerate native-owned PHP references during cycle collection.
    const TRACE: Option<GcTrace<Self>> = None;
    /// The class.
    fn class() -> &'static CustomClass<Self>;
    /// A new object's data, before its constructor runs.
    fn new() -> Self;
    /// Whether [`Custom::destruct`] does anything: only then does the class get
    /// a `dtor_obj` handler, which Zend otherwise skips for classes without
    /// `__destruct`.
    const DESTRUCTS: bool = false;
    /// The object's `dtor_obj` step after `__destruct` ran (Zend runs it once,
    /// on the last reference or at shutdown, before the object is freed). PHP
    /// code it runs sees a live object; a bailout it defers is raised after it,
    /// unless one was already pending. `last` says the object's last reference
    /// went, so Zend frees it next unless this step gives it a new one; at
    /// shutdown an object something still holds is destructed with `last` false.
    fn destruct(&self, _last: bool) {}
    /// The object's `free_obj` step before its data drops. Access to the dying
    /// object's properties is limited to cleanup that cannot run PHP code.
    fn free(&self, _properties: FreeProperties<'_>) {}
    /// Whether PHP may serialize the objects, asked once, when the class registers.
    #[must_use]
    fn serializable() -> bool {
        false
    }
}

/// Restricted access to an object's properties during `free_obj`. The object
/// cannot be borrowed as a [`Value`], called, or given another reference here.
///
/// ```compile_fail
/// use swoole_rs_zend::FreeProperties;
/// fn resurrect(properties: FreeProperties<'_>) {
///     properties.call_method(None, b"save", &[]);
/// }
/// ```
pub struct FreeProperties<'a>(&'a Value);

impl FreeProperties<'_> {
    /// Unlink uploaded files named by the property's current array, including
    /// one PHP replaced, and forget them in the SAPI upload registry. Non-string
    /// entries and non-array properties are ignored; no PHP code runs.
    pub fn release_uploads(self, property: &Declared) {
        if let Some(files) = property.slot(self.0) {
            swr_zend_http_uploads_release(files);
        }
    }
}

/// A class whose objects embed a `T`, which only
/// [`Minit::register_object`](crate::Minit::register_object) registers.
pub struct CustomClass<T>(Class, PhantomData<fn() -> T>);

impl<T> CustomClass<T> {
    #[must_use]
    pub const fn new() -> Self {
        Self(Class::of(true), PhantomData)
    }
}

impl<T> Default for CustomClass<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> std::ops::Deref for CustomClass<T> {
    type Target = Class;

    fn deref(&self) -> &Class {
        &self.0
    }
}

/// One of `T`'s objects by its handle, for a holder the object must not keep
/// alive, such as a hook its own data registers. It is found again in the
/// executor's object store at each use, so it reads `None` once the object is
/// being freed, or when its handle went to an object that is no `T`. Another
/// `T` can take the handle later: a holder that needs the same object compares
/// what it finds. The handle means something only to the executor of the
/// thread that made it, so a `Weak` is neither `Send` nor `Sync`.
pub struct Weak<T>(u32, PhantomData<*const T>);

impl<T> Copy for Weak<T> {}
impl<T> Clone for Weak<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Custom> Weak<T> {
    /// The handle of the `T` object `value` holds.
    #[must_use]
    pub fn of(value: &Value) -> Option<Self> {
        value.custom::<T>()?;
        Some(Self(value.object_ref()?.handle(), PhantomData))
    }

    /// The `T` object at `handle`, as [`Weak::handle`] gave it: each use
    /// finds it again, so any handle is one.
    #[must_use]
    #[inline]
    pub const fn at(handle: u32) -> Self {
        Self(handle, PhantomData)
    }

    /// The object's handle in the executor's store, never 0.
    #[must_use]
    #[inline]
    pub const fn handle(self) -> u32 {
        self.0
    }

    /// The object, with a reference of its own.
    #[must_use]
    #[inline]
    pub fn get(&self) -> Option<Owned> {
        let object = ObjectRef(find(self.0, T::class().handlers())?, PhantomData);
        object.with_value(Value::copy_unfreed)
    }

    /// The object, borrowed as long as `part` is, when `part` lies in its data:
    /// for a holder inside the object, which finds the object it is part of.
    /// None once Zend took the object out of its store to free it (shutdown
    /// frees objects in place, so it finds them then). A zero-sized `part` is
    /// in no object.
    #[must_use]
    pub fn owner<'a, P>(&self, part: &'a P) -> Option<ObjectRef<'a>> {
        let std = find(self.0, T::class().handlers())?;
        if size_of::<P>() == 0 {
            return None;
        }
        let data = Object::<T>::data_ptr(std).addr();
        let at = std::ptr::from_ref(part).addr();
        let inside = data <= at && at + size_of::<P>() <= data + size_of::<T>();
        // A live object of `T`'s class, so an `Object<T>` (as `custom` reads it):
        // `part` borrows bytes of its data for `'a`, and the data lives until
        // `free::<T>` drops it, which needs nothing to borrow it, so the object
        // outlives `'a`.
        inside.then_some(ObjectRef(std, PhantomData))
    }
}

unsafe extern "C" {
    /// The object the executor's store holds at `handle` (`IS_OBJ_VALID`), or
    /// null: only compared or tested here. No reference is taken and no PHP
    /// code runs.
    safe fn swr_zend_objects_live(handle: u32) -> *mut zend_object;
}

/// The live object the executor's store holds at `handle`, if its handlers
/// are `handlers` (compared, never read). No reference is taken.
#[inline]
fn find(handle: u32, handlers: *const zend_object_handlers) -> Option<*mut zend_object> {
    let object = swr_zend_objects_live(handle);
    // A live object, which the store holds.
    (!object.is_null() && ObjectRef(object, PhantomData).handlers() == handlers).then_some(object)
}

/// A new object of `T`'s class around `data`, as its `create_object` handler
/// makes one, but with no constructor run.
/// # Panics
/// `T`'s class is not registered by [`Minit::register_object`](crate::Minit::register_object).
#[must_use]
pub fn new_object<T: Custom>(data: T) -> Owned {
    const { assert!(align_of::<T>() <= 8, "emalloc aligns to 8 bytes") };
    let class = T::class();
    assert!(!class.entry().is_null(), "a registered class");
    // The registered class's own entry, laid out for `T`.
    Owned::object(LaidOut::<T>(class.entry(), PhantomData).create(data))
}

/// `T`'s `create_object` handler. Creating an object never bails out (Zend's
/// allocator queues a `memory_limit` error), so it needs no `zend_try`.
/// Installed only by [`install_custom`] for `T`'s class, so Zend calls it with
/// that class or a subclass of it, which inherits it.
extern "C" fn create<T: Custom>(ce: ClassEntry<'_>) -> *mut zend_object {
    const { assert!(align_of::<T>() <= 8, "emalloc aligns to 8 bytes") };
    // Per Zend; `register_object` gave `T`'s class `T`'s handlers.
    LaidOut::<T>(ce.as_ptr(), PhantomData)
        .create(T::new())
        .into_ptr()
}

/// The object Zend passes a `dtor_obj` handler [`install_custom`] installed for
/// `T`'s class: one of `T`'s live objects, which Zend destructs once. Rust never
/// makes one: it is the handler's parameter, which Zend's call stands for, as
/// [`crate::Minfo`] is MINFO's.
#[repr(transparent)]
pub(crate) struct Handled(NonNull<zend_object>);

/// The object Zend passes a `free_obj` handler [`install_custom`] installed for
/// `T`'s class: one of `T`'s objects whose last reference went, which Zend frees
/// once. Rust never makes one: it is the handler's parameter, which Zend's call
/// stands for, and [`std_dtor`] spends it.
#[repr(transparent)]
pub(crate) struct Freed(NonNull<zend_object>);

unsafe extern "C" {
    /// `zend_object_std_dtor`: releases the standard object's properties, guards
    /// and weak references, as a `free_obj` handler does once; Zend frees the
    /// allocation after the handler returns.
    #[link_name = "zend_object_std_dtor"]
    fn std_dtor(object: Freed);
}

/// Release the Rust data while Zend still holds the object's allocation.
fn free_data<T: Custom>(freed: &Freed) {
    let std = freed.0.as_ptr();
    let object = ObjectRef(std, PhantomData);
    if let Some(data) = object.custom::<T>() {
        data.free(FreeProperties(&object.copy_value()));
    }
    // SAFETY: Zend calls free_obj once, after the last reference goes.
    unsafe { Object::<T>::data_ptr(std).drop_in_place() };
}

crate::exports! {
    preserve_pending:
/// Zend's native `free_obj` boundary. Property destruction may execute PHP.
extern "C" fn free<T: Custom>(freed: Freed) {
    free_data::<T>(&freed);
    // SAFETY: all Rust data has been dropped; Zend lends the object for this last release.
    unsafe { std_dtor(freed) };
}

/// `T`'s `dtor_obj` handler: `__destruct`, then [`Custom::destruct`]. Zend
/// calls it once, with one of `T`'s live objects.
extern "C" fn destruct<T: Custom>(object: Handled) {
    let std = object.0.as_ptr();
    if let Err(error) = crate::contain_with(|| {
        // SAFETY: Zend lends its live object to this destructor. The native
        // trap contains any bailout from __destruct.
        unsafe { zend_objects_destroy_object(std) };
    }) {
        error.defer();
        return;
    }
    // Per Zend, a live object. Zend holds one reference across this handler,
    // so a count of one means nothing else names the object.
    let last = ObjectRef(std, PhantomData).read(|object| object.gc.refcount) == 1;
    {
        // Per Zend, a live object of `T`'s class, which outlives this call.
        if let Some(data) = ObjectRef(std, PhantomData).custom::<T>() {
            data.destruct(last);
        }
    }
}

}

/// An audited enumerator of an object's native-owned PHP references.
pub struct GcTrace<T>(fn(&T, &mut GcRoots));

impl<T> GcTrace<T> {
    /// # Safety
    /// Report each object-owned PHP reference at most once, never a borrowed or
    /// externally owned reference. The visit must not run PHP or change ownership.
    pub const unsafe fn new(visit: fn(&T, &mut GcRoots)) -> Self {
        Self(visit)
    }
}

/// Zend's buffer of native-owned references, borrowed for one cycle collector visit.
#[repr(transparent)]
pub struct GcRoots(crate::sys::zend_get_gc_buffer);

impl GcRoots {
    /// Record one reference owned by the object being traced.
    pub fn owned(&mut self, value: &Owned) {
        swr_slot_gc_add(self, value);
    }
}

type GcTable = std::mem::MaybeUninit<*mut crate::sys::zval>;
type GcCount = std::mem::MaybeUninit<c_int>;
type GetGc = extern "C" fn(ObjectRef<'_>, &mut GcTable, &mut GcCount) -> *mut crate::sys::HashTable;

extern "C" fn get_gc<T: Custom>(
    object: ObjectRef<'_>,
    table: &mut GcTable,
    count: &mut GcCount,
) -> *mut crate::sys::HashTable {
    extern "C" fn visit<T: Custom>(object: ObjectRef<'_>, roots: &mut GcRoots) {
        if let (Some(data), Some(trace)) = (object.custom::<T>(), T::TRACE) {
            (trace.0)(data, roots);
        }
    }
    swr_slot_gc_properties(object, table, count, visit::<T>)
}

/// Native object handlers, called by Zend inside its bailout scope.
type Create = unsafe extern "C" fn(ClassEntry<'_>) -> *mut zend_object;
type Free = unsafe extern "C" fn(Freed);
type Destruct = unsafe extern "C" fn(Handled);

unsafe extern "C" {
    safe fn swr_slot_gc_add(roots: &mut GcRoots, value: &Value);
    safe fn swr_slot_gc_properties(
        object: ObjectRef<'_>,
        table: &mut GcTable,
        count: &mut GcCount,
        visit: extern "C" fn(ObjectRef<'_>, &mut GcRoots),
    ) -> *mut crate::sys::HashTable;
    /// `ce->create_object = create`, which Zend calls with the class or a
    /// subclass that inherits it.
    safe fn swr_slot_create_object(entry: &mut zend_class_entry, create: Create);
    /// The handlers' `free_obj` and, unless `None`, `dtor_obj`, which Zend calls
    /// with an object whose class has these handlers.
    safe fn swr_slot_object_free(
        handlers: &mut zend_object_handlers,
        free: Free,
        destruct: Option<Destruct>,
        get_gc: Option<GetGc>,
    );
}

/// `ce->create_object = create`, as `SW_SET_CLASS_CREATE`.
pub(crate) fn set_create(entry: &mut zend_class_entry, create: Create) {
    swr_slot_create_object(entry, create);
}

/// The handlers [`Minit::register_object`](crate::Minit::register_object)
/// installs for `T`: not cloneable, `unset` denied, `T`'s create and free, and
/// its destructor when it has one.
pub(crate) fn install_custom<T: Custom>(
    entry: &mut zend_class_entry,
    handlers: &mut zend_object_handlers,
) {
    handlers.clone_obj = None;
    crate::class::deny_unset(handlers);
    // Swoole's `SW_SET_CLASS_CUSTOM_OBJECT`: `create::<T>` makes `Object<T>`s
    // with `Object::create`, and `free::<T>` ends with `zend_object_std_dtor`
    // without freeing the object: Zend then frees the allocation at `std - OFFSET`.
    set_create(entry, create::<T>);
    let destruct: Destruct = destruct::<T>;
    swr_slot_object_free(
        handlers,
        free::<T>,
        T::DESTRUCTS.then_some(destruct),
        T::TRACE.map(|_| -> GetGc { get_gc::<T> }),
    );
    handlers.offset = c_int::try_from(Object::<T>::OFFSET).unwrap_or(c_int::MAX);
}

/// Property writes on a live object, under a class that sees them (Zend's
/// `scope`). Zend copies every name and value.
#[derive(Clone, Copy)]
pub struct Props<'a>(Option<ClassEntry<'a>>, ObjectRef<'a>);

impl Props<'_> {
    /// The same object, under its own class (`Z_OBJCE_P`).
    #[must_use]
    #[inline]
    pub fn own(self) -> Self {
        Self(Some(self.class_entry()), self.1)
    }

    /// `Z_OBJCE_P`: the object's own class.
    #[inline]
    pub(crate) fn class(self) -> *mut zend_class_entry {
        self.class_entry().as_ptr()
    }

    /// `zend_update_property` inside `zend_try`: the property takes a copy of
    /// `value`.
    /// # Errors
    /// PHP code the write ran (`__set`, a property hook, a conversion, the old
    /// value's `__destruct`, an error handler) bailed out.
    #[inline]
    pub(crate) fn update_trapped(self, name: &[u8], value: &Value) -> Result<(), bailout::Bailout> {
        bailout::trapped(swr_lent_update_property(
            self.0,
            self.1,
            IoSlice::new(name),
            value,
        ))
    }

    /// `zend_update_property_long`.
    #[inline]
    pub fn long(self, name: &CStr, value: zend_long) {
        self.value(name, &Owned::long(value));
    }

    /// `zend_update_property_bool`.
    #[inline]
    pub fn bool(self, name: &CStr, value: bool) {
        self.value(name, &Owned::bool(value));
    }

    /// `zend_update_property_stringl`.
    #[inline]
    pub fn bytes(self, name: &CStr, value: &[u8]) {
        self.value(name, &Owned::string(ZString::new(value)));
    }

    /// `zend_update_property_null`.
    #[inline]
    pub fn null(self, name: &CStr) {
        self.value(name, &Owned::null());
    }

    /// `zend_update_property`: the property takes a copy of `value`. On a user
    /// class the write can run PHP code (a subclass's `__set` or property hook),
    /// as can an object converted for a typed property (`__toString`), so those
    /// writes run inside `zend_try` and a bailout is deferred. An internal
    /// class's object takes a scalar or string without running any. Nothing is
    /// written while a bailout is pending: upstream never ran past it.
    #[inline]
    pub fn value(self, name: &CStr, value: &Value) {
        /// `ZEND_INTERNAL_CLASS` (`zend_compile.h`).
        const INTERNAL: c_char = 1;
        if bailout::pending() {
            return;
        }
        let internal = self.class_entry().read(|ce| ce.type_) == INTERNAL;
        if internal && value.kind() != IS_OBJECT {
            let scope = self.0.map_or(std::ptr::null_mut(), ClassEntry::as_ptr);
            let (object, length) = (self.1.as_ptr(), name.count_bytes());
            // SAFETY: a live object, a registered class or null, and a live value
            // Zend copies.
            unsafe { zend_update_property(scope, object, name.as_ptr(), length, value.as_ptr()) };
            return;
        }
        bailout::defer(self.update_trapped(name.to_bytes(), value));
    }

    /// The `set()` merge: `php_array_merge` of `settings` into the array
    /// property `name` ([`Props::array_slot`]); nothing when `settings` is no array.
    #[inline]
    pub fn merge(self, name: &CStr, settings: &Value) {
        // The array is lent out of its property for the merge, so a replaced
        // value's `__destruct` can't free or replace it meanwhile.
        self.edit_array(name, |array| array.merge_array(settings));
    }
}

impl<'a> Props<'a> {
    /// Property writes on `object` under `scope` (`None` for none): a
    /// registered class or null.
    #[must_use]
    #[inline]
    pub fn of(scope: Option<&crate::Class>, object: ObjectRef<'a>) -> Self {
        Self(scope.and_then(Class::class_entry), object)
    }

    /// The object, live for `'a` ([`Props::of`]).
    #[must_use]
    #[inline]
    pub fn object(self) -> ObjectRef<'a> {
        self.1
    }

    /// `Z_OBJCE_P` as a handle: the object's own class, which outlives it.
    #[must_use]
    #[inline]
    pub fn class_entry(self) -> crate::ClassEntry<'a> {
        self.object().class_entry()
    }

    /// An owned snapshot of the declared instance property `name`. A later
    /// write through any alias leaves it valid. Uninitialised typed properties
    /// read as null; None for no such property.
    #[must_use]
    #[inline]
    pub fn slot(self, name: &CStr) -> Option<Owned> {
        self.property_slot(name).map(|slot| slot.referred().copy())
    }

    fn property_slot(self, name: &CStr) -> Option<&'a Value> {
        swr_lent_property_slot(self.1, IoSlice::new(name.to_bytes()))
    }

    /// The array [`Props::array_slot`] finds, lent to `edit` out of its
    /// property, which takes it back when `edit`
    /// returns, in place of whatever was put there meanwhile. None as
    /// [`Props::array_slot`].
    pub fn edit_array<R>(self, name: &CStr, edit: impl FnOnce(&mut ArrayMut) -> R) -> Option<R> {
        self.array_slot(name)?.edit_array_slot(edit)
    }

    /// `sw_zend_read_and_convert_property_array`: the slot of the declared
    /// property `name`, holding an array. Any other value is replaced through
    /// `zend_update_property` (inside `zend_try`), never destroyed in its slot,
    /// because releasing it can run PHP code (a `__destruct`) that writes the
    /// property again. None when the object declares no `name`, a bailout is
    /// pending, or that code left something other than an array there.
    fn array_slot(self, name: &CStr) -> Option<&'a Value> {
        let array = || {
            self.property_slot(name)
                .filter(|slot| slot.array().is_some())
        };
        let slot = match array() {
            Some(slot) => slot,
            None if bailout::pending() => return None,
            None => {
                // `value` would skip `zend_try` for an internal class, but the
                // old value's release can still run PHP code.
                bailout::defer(self.update_trapped(name.to_bytes(), &OwnedArray::default()));
                // The release may have written an array and then bailed out.
                if bailout::pending() {
                    return None;
                }
                array()?
            }
        };
        Some(slot)
    }
}

/// A live object without a class: Zend never makes one. Kept out of line so
/// `class_entry` inlines as the bare load it was.
#[cold]
#[inline(never)]
fn classless() -> ! {
    unreachable!("a live object has its class")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn std_follows_the_data_as_in_swoole_structs() {
        // `struct { sw_atomic_t *ptr; zend_object std; }`
        assert_eq!(
            Object::<*mut u32>::OFFSET,
            size_of::<*mut u32>(),
            "pointer-sized data puts std right after it"
        );
        let this = std::ptr::without_provenance_mut::<Object<u64>>(4096);
        assert_eq!(
            Object::std(this).addr() - this.addr(),
            Object::<u64>::OFFSET,
            "std() sits OFFSET bytes in"
        );
    }
}
