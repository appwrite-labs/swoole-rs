//! Objects that embed their `zend_object` after Rust data, as Swoole's C++
//! structs do (`struct { T data; zend_object std; }`).
use crate::{
    ArrayMut, Class, Value,
    sys::{
        _zval_struct__bindgen_ty_1, IS_OBJECT_EX, object_init_ex, object_properties_init,
        swr_zend_class_set_create_object, swr_zend_object_alloc, swr_zend_property_array,
        zend_class_entry, zend_long, zend_object, zend_object_handlers, zend_object_std_dtor,
        zend_object_std_init, zend_update_property, zend_update_property_bool,
        zend_update_property_long, zend_update_property_null, zend_update_property_stringl,
        zend_value, zval,
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
    /// all-zero bytes are a valid `T`.
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

    /// Scope, object and the name as the `zend_update_property_*` take them.
    fn at(
        self,
        name: &CStr,
    ) -> (
        *mut zend_class_entry,
        *mut zend_object,
        *const c_char,
        usize,
    ) {
        (self.0, self.1, name.as_ptr(), name.count_bytes())
    }

    /// `zend_update_property_long`.
    #[inline]
    pub fn long(self, name: &CStr, value: zend_long) {
        let (scope, object, name, length) = self.at(name);
        // SAFETY: a live object and a registered class or null.
        unsafe { zend_update_property_long(scope, object, name, length, value) };
    }

    /// `zend_update_property_bool`.
    #[inline]
    pub fn bool(self, name: &CStr, value: bool) {
        let (scope, object, name, length) = self.at(name);
        // SAFETY: as above.
        unsafe { zend_update_property_bool(scope, object, name, length, value.into()) };
    }

    /// `zend_update_property_stringl`.
    #[inline]
    pub fn bytes(self, name: &CStr, value: &[u8]) {
        let (scope, object, name, length) = self.at(name);
        let (data, size) = (value.as_ptr().cast(), value.len());
        // SAFETY: as above.
        unsafe { zend_update_property_stringl(scope, object, name, length, data, size) };
    }

    /// `zend_update_property_null`.
    #[inline]
    pub fn null(self, name: &CStr) {
        let (scope, object, name, length) = self.at(name);
        // SAFETY: as above.
        unsafe { zend_update_property_null(scope, object, name, length) };
    }

    /// `zend_update_property`: the property takes a copy of `value`.
    #[inline]
    pub fn value(self, name: &CStr, value: &Value) {
        let (scope, object, name, length) = self.at(name);
        // SAFETY: as above, and a live value.
        unsafe { zend_update_property(scope, object, name, length, value.as_ptr()) };
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
        let (scope, object, name, length) = self.at(name);
        // SAFETY: a live object and a registered class or null.
        let array = unsafe { swr_zend_property_array(scope, object, name, length) };
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

/// `object_init_ex(target, ce)`: a new object of class `ce` in `target`.
/// # Safety
/// `target` is a writable zval and `ce` a registered, instantiable class.
pub unsafe fn init_object(target: *mut zval, ce: *mut zend_class_entry) {
    // SAFETY: per the caller; it only fails for abstract classes and interfaces.
    let _ = unsafe { object_init_ex(target, ce) };
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
