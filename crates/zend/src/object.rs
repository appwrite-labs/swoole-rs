//! Objects that embed their `zend_object` after Rust data, as Swoole's C++
//! structs do (`struct { T data; zend_object std; }`).
use crate::{
    ArrayMut, Class, Owned, Value, ZString, bailout,
    sys::{
        _zval_struct__bindgen_ty_1, IS_OBJECT, IS_OBJECT_EX, object_init_ex,
        object_properties_init, swr_zend_class_set_create_object, swr_zend_object_alloc,
        swr_zend_property_array, swr_zend_try_facade_update, zend_class_entry, zend_long,
        zend_object, zend_object_handlers, zend_object_std_dtor, zend_object_std_init,
        zend_objects_destroy_object, zend_update_property, zend_value, zval,
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

    /// Create an object while containing every allocation bailout below this frame.
    /// The class's create handler raises an error only after handling this result.
    /// # Errors
    /// A Zend allocation or initialization bailed out.
    /// # Safety
    /// The same class/layout/all-zero-data contract as [`Self::create`].
    pub unsafe fn try_create(
        class: &Class,
        ce: *mut zend_class_entry,
    ) -> Result<*mut zend_object, crate::bailout::Bailout> {
        let mut allocation = std::ptr::null_mut();
        // SAFETY: Zend allocates enough space for this layout and the class's properties.
        crate::bailout::trapped(unsafe {
            crate::sys::swr_zend_try_facade_object_alloc(size_of::<Self>(), ce, &raw mut allocation)
        })?;
        let object = Self::std(allocation.cast::<Self>());
        // SAFETY: the fresh aligned allocation contains the standard object's storage.
        crate::bailout::trapped(unsafe {
            crate::sys::swr_zend_try_facade_object_init(object, ce)
        })?;
        // SAFETY: the standard object has been initialized with this class.
        crate::bailout::trapped(unsafe {
            crate::sys::swr_zend_try_facade_properties_init(object, ce)
        })?;
        // SAFETY: this initialized object is not shared until it is returned.
        unsafe { (*object).handlers = class.handlers() };
        Ok(object)
    }

    /// The standard destructor, with PHP property-destructor bailouts contained.
    /// # Errors
    /// A property destructor bailed out.
    /// # Safety
    /// The same live-object contract as [`Self::dtor`].
    pub unsafe fn try_dtor(object: *mut zend_object) -> Result<(), crate::bailout::Bailout> {
        // SAFETY: Zend is freeing this initialized standard object.
        crate::bailout::trapped(unsafe { crate::sys::swr_zend_try_facade_object_dtor(object) })
    }

    /// Swoole's `SW_SET_CLASS_CUSTOM_OBJECT`: the class creates `Object<T>`s
    /// with `create` and frees them with `free`.
    pub fn install(
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

/// A new object of `T`'s class around `data`, as its `create_object` handler
/// makes one, but with no constructor run. When Zend's allocation bails out,
/// `data` drops, the bailout is deferred ([`bailout::defer`]) and the value is
/// undef.
/// # Panics
/// `T`'s class is not registered by [`Minit::register_object`](crate::Minit::register_object).
#[must_use]
pub fn new_object<T: Custom>(data: T) -> Owned {
    let class = T::class();
    assert!(!class.entry().is_null(), "a registered class");
    // SAFETY: the registered class's own entry, laid out for `T`.
    match unsafe { make(class, class.entry(), data) } {
        // SAFETY: the new object, whose one reference this takes.
        Ok(std) => unsafe { Owned::object(std) },
        Err(bailed) => {
            bailed.defer();
            Owned::default()
        }
    }
}

/// Allocate an object of `ce` (`class` or a subclass of it) around `data`,
/// containing Zend's bailouts so that `data` drops before one continues.
/// # Safety
/// `ce` is `class`'s entry or a subclass's, and `class`'s handlers are `T`'s.
unsafe fn make<T: Custom>(
    class: &Class,
    ce: *mut zend_class_entry,
    data: T,
) -> Result<*mut zend_object, bailout::Bailout> {
    const { assert!(align_of::<T>() <= 8, "emalloc aligns to 8 bytes") };
    // SAFETY: per the caller; `data` is written before the object is shared.
    let std = unsafe { Object::<T>::try_create(class, ce) }?;
    // SAFETY: the object's data, allocated and unused.
    unsafe { Object::<T>::data_ptr(std).write(data) };
    Ok(std)
}

/// `T`'s `create_object` handler. The object is allocated before `T`'s data
/// exists, so a bailout from Zend's allocator crosses only this frame, which
/// owns nothing yet, as upstream's handlers do: no `zend_try` per object.
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
    // A bailout pending before is its deferrer's to raise.
    let pending = bailout::pending();
    // SAFETY: per Zend; nothing borrows the data any more.
    unsafe { Object::<T>::data_ptr(std).drop_in_place() };
    // SAFETY: as above.
    unsafe { zend_object_std_dtor(std) };
    if !pending {
        bailout::finish();
    }
}

/// `T`'s `dtor_obj` handler: `__destruct`, then [`Custom::destruct`].
/// # Safety
/// Zend calls it once, with one of `T`'s live objects.
unsafe extern "C" fn destruct<T: Custom>(std: *mut zend_object) {
    // SAFETY: per Zend; a bailout from `__destruct` crosses only this frame,
    // which owns nothing yet.
    unsafe { zend_objects_destroy_object(std) };
    // A bailout pending before is its deferrer's to raise.
    let pending = bailout::pending();
    // SAFETY: a live object of `T`'s class, which outlives this call.
    if let Some(data) = unsafe { custom::<T>(std) } {
        data.destruct();
    }
    if !pending {
        bailout::finish();
    }
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
    Object::<T>::install(entry, handlers, create::<T>, free::<T>);
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
}

impl<'a> Props<'a> {
    /// The array property `name`, made an empty array first unless it is one
    /// (`swr_zend_property_array`), to add to.
    /// # Safety
    /// Nothing else borrows the property, and no PHP code runs, while the
    /// result is borrowed.
    #[must_use]
    #[inline]
    pub unsafe fn array(self, name: &CStr) -> &'a mut ArrayMut {
        // SAFETY: a live object and a registered class or null.
        let array =
            unsafe { swr_zend_property_array(self.0, self.1, name.as_ptr(), name.count_bytes()) };
        // SAFETY: an array property of a live object; per the caller.
        unsafe { ArrayMut::from_ptr(array) }
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
