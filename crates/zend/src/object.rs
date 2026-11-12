//! Objects that embed their `zend_object` after Rust data, as Swoole's C++
//! structs do (`struct { T data; zend_object std; }`).
use crate::{
    ArrayMut, Class, Owned, OwnedArray, Value, ZString, bailout,
    sys::{
        _zval_struct__bindgen_ty_1, IS_OBJECT, IS_OBJECT_EX, object_init_ex,
        object_properties_init, swr_zend_class_set_create_object, swr_zend_object_alloc,
        swr_zend_objects_hold, swr_zend_property_slot, swr_zend_try_facade_update,
        zend_class_entry, zend_long, zend_object, zend_object_handlers, zend_object_std_dtor,
        zend_object_std_init, zend_objects_destroy_object, zend_update_property, zend_value, zval,
    },
};
use std::{
    ffi::{CStr, c_char, c_int},
    marker::PhantomData,
    mem::offset_of,
};

/// `data` followed by the object Zend sees; `std` must stay last, since Zend
/// allocates the class's property slots after it.
#[repr(C)]
pub struct Object<T> {
    pub data: T,
    pub std: zend_object,
}

impl<T> Object<T> {
    /// Where `std` sits: the handlers' `offset`.
    pub const OFFSET: usize = offset_of!(Self, std);

    /// The body of a `create_object` handler: allocate, zero `data`, initialise
    /// the standard object and its properties, and install `class`'s handlers.
    /// # Safety
    /// `ce` is `class`'s entry or a subclass of it, `class` is registered, and
    /// all-zero bytes are a valid `T` or the caller writes `data` before the
    /// object is shared.
    #[must_use]
    pub unsafe fn create(class: &Class, ce: *mut zend_class_entry) -> *mut zend_object {
        // SAFETY: Zend's allocator; it zeroes everything before `std`.
        let this = unsafe { swr_zend_object_alloc(size_of::<Self>(), ce) }.cast::<Self>();
        let std = Self::std(this);
        // SAFETY: `std` is freshly allocated storage for an object of class `ce`.
        unsafe { zend_object_std_init(std, ce) };
        // SAFETY: as above; the property slots follow `std`.
        unsafe { object_properties_init(std, ce) };
        // SAFETY: `std` is initialised and not yet shared.
        unsafe { (*std).handlers = class.handlers() };
        std
    }

    /// Swoole's `SW_SET_CLASS_CUSTOM_OBJECT`: the class creates `Object<T>`s
    /// with `create` and frees them with `free`.
    /// # Safety
    /// `create` returns the `std` of a new `Object<T>` (one [`Self::create`]
    /// made), and `free` destroys the standard object
    /// it is given ([`Self::dtor`]) without freeing it:
    /// Zend then frees the allocation at `std - OFFSET`.
    pub unsafe fn install(
        entry: &mut zend_class_entry,
        handlers: &mut zend_object_handlers,
        create: unsafe extern "C" fn(*mut zend_class_entry) -> *mut zend_object,
        free: unsafe extern "C" fn(*mut zend_object),
    ) {
        // SAFETY: a registered entry, written at MINIT.
        unsafe { swr_zend_class_set_create_object(entry, Some(create)) };
        handlers.free_obj = Some(free);
        handlers.offset = c_int::try_from(Self::OFFSET).unwrap_or(c_int::MAX);
    }

    /// Where the data of the object around `std` sits, for callers that keep
    /// no reference to it across PHP code (cells, or raw field access).
    #[must_use]
    #[inline]
    pub fn data_ptr(std: *mut zend_object) -> *mut T {
        // `data` leads the `repr(C)` struct, `OFFSET` bytes before `std`.
        std.wrapping_byte_sub(Self::OFFSET).cast()
    }

    /// The data of the object around `std` (`fetch_object(std)->data`).
    /// # Safety
    /// `std` is the `std` of a live `Object<T>`, and no other reference to its
    /// data is live while the result is.
    #[must_use]
    #[inline]
    pub unsafe fn data<'a>(std: *mut zend_object) -> &'a mut T {
        let this = std.wrapping_byte_sub(Self::OFFSET).cast::<Self>();
        // SAFETY: per the caller, `this` is the live object around `std`.
        unsafe { &mut (*this).data }
    }

    #[must_use]
    pub fn std(this: *mut Self) -> *mut zend_object {
        this.wrapping_byte_add(Self::OFFSET).cast()
    }

    /// The last step of a `free_obj` handler.
    /// # Safety
    /// `std` is a live object Zend is freeing.
    pub unsafe fn dtor(std: *mut zend_object) {
        // SAFETY: per the caller.
        unsafe { zend_object_std_dtor(std) };
    }
}

/// The Rust data a class's objects embed (Swoole's `SW_SET_CLASS_CUSTOM_OBJECT`),
/// with no `unsafe` in the family crate: [`Minit::register_object`](crate::Minit::register_object)
/// installs create and free handlers that make each object's data with
/// [`Custom::new`] and drop it with the object, and [`Call::custom`](crate::Call::custom)
/// and [`Value::custom`](crate::Value::custom) borrow it. Mutable state goes in
/// `Cell`s or `OnceCell`s: methods, and PHP code they run, share the data.
pub trait Custom: Sized + 'static {
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
    /// unless one was already pending.
    fn destruct(&self) {}
    /// Whether PHP may serialize the objects, asked once, when the class registers.
    #[must_use]
    fn serializable() -> bool {
        false
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

/// `T`'s data in `std`, when `std` is one of `T`'s objects: made by [`create`]
/// or [`new_object`], whose handlers are `T`'s class's. For an object held
/// by pointer; [`Call::custom`](crate::Call::custom) and
/// [`Value::custom`](crate::Value::custom) borrow one a frame or zval holds.
/// # Safety
/// `std` is a live object that outlives `'a`.
pub unsafe fn custom<'a, T: Custom>(std: *mut zend_object) -> Option<&'a T> {
    // SAFETY: per the caller.
    if unsafe { (*std).handlers } != T::class().handlers() {
        return None;
    }
    // SAFETY: only `create::<T>` and `new_object::<T>` give an object the
    // handlers `register_object::<T>` made, after writing its data; only
    // `free::<T>` drops it, when no reference to the object (so none to its
    // data) remains. Handlers are null only before registration.
    Some(unsafe { &*Object::<T>::data_ptr(std) })
}

/// One of `T`'s objects by its handle, for a holder the object must not keep
/// alive, such as a hook its own data registers. It is found again in the
/// executor's object store at each use, so it reads `None` once the object is
/// being freed, or when its handle went to an object that is no `T`. Another
/// `T` can take the handle later: a holder that needs the same object compares
/// what it finds.
pub struct Weak<T>(u32, PhantomData<fn() -> T>);

impl<T: Custom> Weak<T> {
    /// The handle of the `T` object `value` holds.
    #[must_use]
    pub fn of(value: &Value) -> Option<Self> {
        value.custom::<T>()?;
        // SAFETY: the zval holds the live object.
        Some(Self(unsafe { (*value.object()?).handle }, PhantomData))
    }

    /// The object, with a reference of its own.
    #[must_use]
    pub fn get(&self) -> Option<Owned> {
        let found = Owned::default();
        // SAFETY: reads the executor's object store on its thread, and writes
        // the empty zval `found` owns only with a counted object.
        unsafe { swr_zend_objects_hold(self.0, T::class().handlers(), found.as_ptr()) }
            .then_some(found)
    }
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
    // SAFETY: the registered class's own entry, laid out for `T`; `data` is
    // written before the object is shared.
    let std = unsafe { Object::<T>::create(class, class.entry()) };
    // SAFETY: the object's data, allocated and unused.
    unsafe { Object::<T>::data_ptr(std).write(data) };
    // SAFETY: the new object, whose one reference this takes.
    unsafe { Owned::object(std) }
}

/// `T`'s `create_object` handler. Creating an object never bails out (Zend's
/// allocator queues a `memory_limit` error), so it needs no `zend_try`.
/// # Safety
/// Zend calls it with `T`'s class or a subclass of it, which inherits it.
unsafe extern "C" fn create<T: Custom>(ce: *mut zend_class_entry) -> *mut zend_object {
    const { assert!(align_of::<T>() <= 8, "emalloc aligns to 8 bytes") };
    // SAFETY: per Zend; `register_object` gave `T`'s class `T`'s handlers, and
    // the data is written below, before the object is shared.
    let std = unsafe { Object::<T>::create(T::class(), ce) };
    // SAFETY: the object's data, allocated and unused.
    unsafe { Object::<T>::data_ptr(std).write(T::new()) };
    std
}

/// `T`'s `free_obj` handler: drop the data, then free the standard object.
/// A bailout the drop defers (releasing a callable can run PHP code) is
/// raised last, unless one was already pending.
/// # Safety
/// Zend calls it once, with one of `T`'s objects whose last reference went.
unsafe extern "C" fn free<T: Custom>(std: *mut zend_object) {
    bailout::finish_after(|| {
        // SAFETY: per Zend; nothing borrows the data any more.
        unsafe { Object::<T>::data_ptr(std).drop_in_place() };
        // SAFETY: as above.
        unsafe { zend_object_std_dtor(std) };
    });
}

/// `T`'s `dtor_obj` handler: `__destruct`, then [`Custom::destruct`].
/// # Safety
/// Zend calls it once, with one of `T`'s live objects.
unsafe extern "C" fn destruct<T: Custom>(std: *mut zend_object) {
    // SAFETY: per Zend; a bailout from `__destruct` crosses only this frame,
    // which owns nothing yet.
    unsafe { zend_objects_destroy_object(std) };
    bailout::finish_after(|| {
        // SAFETY: a live object of `T`'s class, which outlives this call.
        if let Some(data) = unsafe { custom::<T>(std) } {
            data.destruct();
        }
    });
}

/// The handlers [`Minit::register_object`](crate::Minit::register_object)
/// installs for `T`: not cloneable, `unset` denied, `T`'s create and free, and
/// its destructor when it has one.
pub(crate) fn install_custom<T: Custom>(
    entry: &mut zend_class_entry,
    handlers: &mut zend_object_handlers,
) {
    handlers.clone_obj = None;
    handlers.unset_property = Some(crate::unset_property_deny);
    // SAFETY: `create::<T>` makes `Object<T>`s with `Object::create`, and
    // `free::<T>` ends with `zend_object_std_dtor`.
    unsafe { Object::<T>::install(entry, handlers, create::<T>, free::<T>) };
    if T::DESTRUCTS {
        handlers.dtor_obj = Some(destruct::<T>);
    }
}

/// Property writes on a live object, under a class that sees them (Zend's
/// `scope`). Zend copies every name and value.
#[derive(Clone, Copy)]
pub struct Props<'a>(
    *mut zend_class_entry,
    *mut zend_object,
    PhantomData<&'a zend_object>,
);

impl Props<'_> {
    /// # Safety
    /// `object` is live for `'a`, and `scope` is null or a registered class.
    #[must_use]
    #[inline]
    pub unsafe fn new(scope: *mut zend_class_entry, object: *mut zend_object) -> Self {
        Self(scope, object, PhantomData)
    }

    /// The same object, under its own class (`Z_OBJCE_P`).
    #[must_use]
    #[inline]
    pub fn own(self) -> Self {
        // SAFETY: a live object.
        Self(unsafe { (*self.1).ce }, self.1, PhantomData)
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
        let (scope, object, name, length) = (self.0, self.1, name.as_ptr(), name.count_bytes());
        // SAFETY: a live object.
        let class = unsafe { (*object).ce };
        // SAFETY: its class, which outlives it.
        let internal = unsafe { (*class).type_ } == INTERNAL;
        if internal && value.kind() != IS_OBJECT {
            // SAFETY: a live object, a registered class or null, and a live value
            // Zend copies.
            unsafe { zend_update_property(scope, object, name, length, value.as_ptr()) };
            return;
        }
        // SAFETY: as above.
        bailout::defer(bailout::trapped(unsafe {
            swr_zend_try_facade_update(scope, object, name, length, value.as_ptr())
        }));
    }

    /// The `set()` merge: `php_array_merge` of `settings` into the array
    /// property `name` ([`Props::array`]); nothing when `settings` is no array.
    #[inline]
    pub fn merge(self, name: &CStr, settings: &Value) {
        // SAFETY: the array serves this one merge, which separates it first and
        // traps a replaced value's `__destruct`.
        if let Some(array) = unsafe { self.array(name) } {
            array.merge_array(settings);
        }
    }
}

impl<'a> Props<'a> {
    /// `sw_zend_read_and_convert_property_array`: the declared property `name`,
    /// as an array to add to. Any other value is replaced through
    /// `zend_update_property` (inside `zend_try`), never destroyed in its slot,
    /// because releasing it can run PHP code (a `__destruct`) that writes the
    /// property again. None when the object declares no `name`, a bailout is
    /// pending, or that code left something other than an array there.
    /// # Safety
    /// Nothing else borrows the property, and no PHP code runs, while the
    /// result is borrowed.
    #[must_use]
    #[inline]
    pub unsafe fn array(self, name: &CStr) -> Option<&'a mut ArrayMut> {
        let array = || {
            // SAFETY: a live object; Zend reads `name` only.
            let slot = unsafe { swr_zend_property_slot(self.1, name.as_ptr(), name.count_bytes()) };
            // SAFETY: null, or a slot of the live object.
            unsafe { Value::from_ptr(slot) }?.array().map(|_| slot)
        };
        let slot = match array() {
            Some(slot) => slot,
            None if bailout::pending() => return None,
            None => {
                // `value` would skip `zend_try` for an internal class, but the
                // old value's release can still run PHP code.
                // SAFETY: a live object, a registered class or null, and a
                // live value Zend copies.
                bailout::defer(bailout::trapped(unsafe {
                    swr_zend_try_facade_update(
                        self.0,
                        self.1,
                        name.as_ptr(),
                        name.count_bytes(),
                        OwnedArray::default().as_ptr(),
                    )
                }));
                // The release may have written an array and then bailed out.
                if bailout::pending() {
                    return None;
                }
                array()?
            }
        };
        // SAFETY: an array slot of a live object; per the caller.
        Some(unsafe { ArrayMut::from_ptr(slot) })
    }
}

/// `ZVAL_OBJ(target, std)`: `target` takes over the reference `std` carries.
/// # Safety
/// `target` is a writable zval and `std` a live object.
pub unsafe fn set_object(target: *mut zval, std: *mut zend_object) {
    // SAFETY: per the caller.
    unsafe { (*target).value = zend_value { obj: std } };
    // SAFETY: as above.
    unsafe {
        (*target).u1 = _zval_struct__bindgen_ty_1 {
            type_info: IS_OBJECT_EX,
        };
    }
}

/// `object_init_ex(target, ce)`: a new object of class `ce` in `target`,
/// which holds its reference; the object.
/// # Safety
/// `target` is a writable zval and `ce` a registered, instantiable class.
pub unsafe fn init_object(target: *mut zval, ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: per the caller; it only fails for abstract classes and interfaces.
    let _ = unsafe { object_init_ex(target, ce) };
    // SAFETY: `target` now holds the new object.
    unsafe { crate::value::obj_of(target) }
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
