//! Static-method classes such as `Swoole\Timer`: global function aliases of
//! their methods, a `create_object` that refuses instances, and array results.
//! Also [`Function`], a borrowed `zend_function`.
use crate::{
    Call, ClassEntry, ZStr,
    arginfo::Table,
    sys::{
        _zend_function__bindgen_ty_1, _zval_struct__bindgen_ty_1, IS_UNDEF, zend_class_entry,
        zend_function, zend_function_entry, zend_internal_function, zend_object,
        zend_object_handlers, zend_op, zend_op_array, zval,
    },
};
use std::{ffi::CStr, marker::PhantomData, mem::offset_of};

/// A live `zend_function`, borrowed for `'a`: one a frame is calling, or a
/// resolved callable's handler. Zend may change it (its flags, its run-time
/// cache) while PHP runs, so each read copies the header every kind of function
/// starts with and no reference to the function is ever made.
#[derive(Clone, Copy)]
pub struct Function<'a>(
    pub(crate) *mut zend_function,
    pub(crate) PhantomData<&'a zend_function>,
);

impl<'a> Function<'a> {
    /// `zend_function.type`'s `ZEND_INTERNAL_FUNCTION`.
    pub const INTERNAL: u8 = 1;
    /// `zend_function.type`'s `ZEND_USER_FUNCTION`.
    pub const USER: u8 = 2;

    /// The function at `function`, or None for null.
    /// # Safety
    /// `function` is null or a function that lives for `'a`.
    #[must_use]
    #[inline]
    pub unsafe fn from_ptr(function: *mut zend_function) -> Option<Self> {
        (!function.is_null()).then_some(Self(function, PhantomData))
    }

    #[must_use]
    #[inline]
    pub fn as_ptr(self) -> *mut zend_function {
        self.0
    }

    /// `function->common`, copied: plain integers and pointers.
    #[inline]
    fn common(self) -> _zend_function__bindgen_ty_1 {
        // SAFETY: a live function, per the type; `zend_function` is a union,
        // every member of which starts with this header.
        unsafe { self.0.cast::<_zend_function__bindgen_ty_1>().read() }
    }

    /// `function->type`: [`Self::INTERNAL`], [`Self::USER`] or `ZEND_EVAL_CODE`.
    #[must_use]
    #[inline]
    pub fn kind(self) -> u8 {
        self.common().type_
    }

    /// `function->common.fn_flags` (`ZEND_ACC_*`), as they are now.
    #[must_use]
    #[inline]
    pub fn flags(self) -> u32 {
        self.common().fn_flags
    }

    /// `function->common.num_args`: the declared parameters, without a variadic one.
    #[must_use]
    #[inline]
    pub fn num_args(self) -> u32 {
        self.common().num_args
    }

    /// `function->common.function_name`; None for a file's or `eval`'s code.
    #[must_use]
    #[inline]
    pub fn name(self) -> Option<&'a ZStr> {
        // SAFETY: null, or the name the function holds for as long as it lives.
        unsafe { ZStr::from_ptr(self.common().function_name) }
    }

    /// `function->common.scope`: a method's class; None for a plain function.
    #[must_use]
    #[inline]
    pub fn scope(self) -> Option<ClassEntry<'a>> {
        let scope = self.common().scope;
        // A function's class outlives it.
        (!scope.is_null()).then_some(ClassEntry(scope, PhantomData))
    }

    /// `function->common.scope->name`: a method's class's name.
    #[must_use]
    #[inline]
    pub fn scope_name(self) -> Option<&'a ZStr> {
        self.scope().map(ClassEntry::name)
    }

    /// `function->internal_function.handler` of an internal function; None
    /// for user code, which has none.
    #[must_use]
    #[inline]
    pub fn internal_handler(self) -> Option<crate::sys::zif_handler> {
        if self.kind() != Self::INTERNAL {
            return None;
        }
        let handler = self
            .0
            .cast::<zend_internal_function>()
            .wrapping_byte_add(offset_of!(zend_internal_function, handler))
            .cast::<crate::sys::zif_handler>();
        // SAFETY: a live internal function, per the type and its kind, is a
        // `zend_internal_function`; no Zend code writes its handler while this
        // copies it.
        Some(unsafe { handler.read() })
    }

    /// `&function->op_array` of user code (`ZEND_USER_CODE(type)`: a user
    /// function or `eval`'s code); None for an internal function. A pointer:
    /// Zend writes the op array's caches while it runs.
    #[must_use]
    #[inline]
    pub fn op_array(self) -> Option<*mut zend_op_array> {
        (self.kind() != Self::INTERNAL).then(|| self.0.cast())
    }

    /// `function->op_array.opcodes`: user code's first opline; None for an
    /// internal function.
    #[must_use]
    #[inline]
    pub fn opcodes(self) -> Option<*mut zend_op> {
        self.instructions().map(|(first, _)| first)
    }

    /// `function->op_array.opcodes` and `.last`: user code's first opline and how
    /// many follow it; None for an internal function.
    #[must_use]
    #[inline]
    pub fn instructions(self) -> Option<(*mut zend_op, u32)> {
        let op_array = self.op_array()?;
        // SAFETY: a live user function is its op array, which no Zend code
        // writes while this reads its two fields.
        let op_array = unsafe { &*op_array };
        Some((op_array.opcodes, op_array.last))
    }
}

impl Call {
    /// `add_assoc_bool_ex(return_value, key, value)`, after `set_array`
    /// (nothing unless the return slot holds an array).
    pub fn add_assoc_bool(&self, key: &CStr, value: bool) {
        self.add(|array| array.insert_bool(key.to_bytes(), value));
    }
}

/// Swoole's `sw_zend_create_object_deny`: the `create_object` of a class that
/// cannot be instantiated. It throws and returns the bare object Zend expects
/// (declared properties undefined), which Zend then releases.
/// # Safety
/// Installed as `create_object`, so Zend passes a registered class.
pub unsafe extern "C" fn create_object_deny(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: a registered class; creating an object never bails out.
    let object = unsafe { crate::sys::zend_objects_new(ce) };
    // Per Zend, a registered class, which lives for the process.
    let class = ClassEntry(ce, PhantomData);
    let count = usize::try_from(class.read(|ce| ce.default_properties_count)).unwrap_or(0);
    // `zend_objects_new` allocated the class's property slots from here.
    let slots = object
        .wrapping_byte_add(offset_of!(zend_object, properties_table))
        .cast::<zval>();
    for index in 0..count {
        // SAFETY: as above; `ZVAL_UNDEF`.
        unsafe {
            (*slots.wrapping_add(index)).u1 = _zval_struct__bindgen_ty_1 {
                type_info: IS_UNDEF,
            };
        }
    }
    // SAFETY: Zend's error construction is trapped before this callback returns.
    if unsafe { crate::sys::swr_zend_try_facade_creation_error(ce) } != 0 {
        crate::bailout::Bailout.defer();
    }
    crate::bailout::finish();
    object
}

/// `SW_SET_CLASS_CREATE(.., sw_zend_create_object_deny)`.
pub(crate) fn deny(entry: &mut zend_class_entry, _: &mut zend_object_handlers) {
    // `ce->create_object`, in the union it shares with an interface's hook.
    entry.__bindgen_anon_2.create_object = Some(create_object_deny);
}

/// The row of `method` in a method table, with its arginfo.
pub(crate) fn arg_info(methods: Table, method: &CStr) -> Option<&'static zend_function_entry> {
    methods
        .rows()
        .iter()
        .take_while(|entry| !entry.fname.is_null())
        // SAFETY: a `Table` row's name is a static C string; only `END`'s is null.
        .find(|entry| unsafe { CStr::from_ptr(entry.fname) } == method)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        arginfo::{ArgInfo, END, Methods, begin, entry},
        sys::zend_execute_data,
    };

    unsafe extern "C" fn handler(_: *mut zend_execute_data, _: *mut zval) {}

    static TICK: ArgInfo<1> = ArgInfo::new([begin(0, 0)]);
    static AFTER: ArgInfo<1> = ArgInfo::new([begin(0, 0)]);
    static CLEAR_ALL: ArgInfo<1> = ArgInfo::new([begin(0, 0)]);

    #[test]
    fn aliases_take_their_methods_arginfo() {
        static TABLE: Methods<4> = Methods::new([
            entry(c"tick", handler, &TICK, 0),
            entry(c"after", handler, &AFTER, 0),
            entry(c"clearAll", handler, &CLEAR_ALL, 0),
            END,
        ]);
        let info = |method| arg_info(TABLE.table(), method).map(|row| row.arg_info);
        assert_eq!(info(c"after"), Some(AFTER.as_ptr()), "after's arginfo");
        assert_eq!(info(c"clearAll"), Some(CLEAR_ALL.as_ptr()), "clearAll's");
        assert!(info(c"list").is_none(), "a missing method");
    }
}
