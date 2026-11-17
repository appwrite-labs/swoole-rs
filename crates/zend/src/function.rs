//! Static-method classes such as `Swoole\Timer`: global function aliases of
//! their methods, a `create_object` that refuses instances, and array results.
//! Also [`Function`], a borrowed `zend_function`.
use crate::{
    Call, ClassEntry, Invalid, ZStr,
    arginfo::Table,
    sys::{
        _zend_function__bindgen_ty_1, zend_class_entry, zend_fcall_info, zend_fcall_info_cache,
        zend_function, zend_function_entry, zend_internal_function, zend_object,
        zend_object_handlers, zend_op, zend_op_array,
    },
};
use std::{ffi::CStr, marker::PhantomData, mem::offset_of};

unsafe extern "C" {
    /// `sw_zend_create_object_deny`'s body: a bare new object of the live class
    /// (its declared properties undefined), then the "can not be created"
    /// `Error`, inside `zend_try` (`bailed` set for a bailout).
    safe fn swr_zend_object_deny(class: ClassEntry<'_>, bailed: &mut bool) -> *mut zend_object;
}

/// A live `zend_function`, borrowed for `'a`: one a frame is calling, or a
/// resolved callable's handler. Zend may change it (its flags, its run-time
/// cache) while PHP runs, so each read copies the header every kind of function
/// starts with and no reference to the function is ever made.
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct Function<'a>(
    pub(crate) *mut zend_function,
    pub(crate) PhantomData<&'a zend_function>,
);

impl<'a> Function<'a> {
    /// `zend_function.type`'s `ZEND_INTERNAL_FUNCTION`.
    pub const INTERNAL: u8 = 1;
    /// `zend_function.type`'s `ZEND_USER_FUNCTION`.
    pub const USER: u8 = 2;

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
        // A function's class outlives it.
        ClassEntry::live(self.common().scope)
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

/// A callable's cache, lent for `'a`, whose function (none while it is
/// unresolved) the callable keeps live as long: one [`Call::resolve`] parsed, a
/// kept [`crate::Callable`]'s, or the one a coroutine call's shim lends its
/// [`crate::host::Create`] leaf. Only those make one.
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct Fcc<'a>(pub(crate) &'a zend_fcall_info_cache);

impl<'a> Fcc<'a> {
    /// The cache's fields.
    #[must_use]
    #[inline]
    pub fn get(self) -> &'a zend_fcall_info_cache {
        self.0
    }

    /// `fcc->function_handler`: the function, None while unresolved.
    #[must_use]
    #[inline]
    pub fn function(self) -> Option<Function<'a>> {
        let function = self.0.function_handler;
        (!function.is_null()).then_some(Function(function, PhantomData))
    }
}

impl Call {
    /// `Z_PARAM_FUNC` of passed argument `n` (1-based) into `fci` and `fcc`, as
    /// [`Call::callable`]: the resolved cache, lent while the argument keeps
    /// its function.
    /// # Errors
    /// It is not callable; Zend has raised its parameter error.
    /// # Panics
    /// Argument `n` was not passed (the caller's arity check allows it).
    pub fn resolve<'c>(
        &'c self,
        n: u32,
        fci: &mut zend_fcall_info,
        fcc: &'c mut zend_fcall_info_cache,
    ) -> Result<Fcc<'c>, Invalid> {
        self.callable(n, fci, fcc)?;
        Ok(Fcc(fcc))
    }

    /// `add_assoc_bool_ex(return_value, key, value)`, after `set_array`
    /// (nothing unless the return slot holds an array).
    pub fn add_assoc_bool(&self, key: &CStr, value: bool) {
        self.add(|array| array.insert_bool(key.to_bytes(), value));
    }
}

crate::exports! {
/// Swoole's `sw_zend_create_object_deny`: the `create_object` of a class that
/// cannot be instantiated. It throws and returns the bare object Zend expects
/// (declared properties undefined), which Zend then releases. Installed as
/// `create_object` ([`deny`]), so Zend passes a registered class.
extern "C" fn create_object_deny(class: ClassEntry<'_>) -> *mut zend_object {
    let mut bailed = false;
    let object = swr_zend_object_deny(class, &mut bailed);
    if bailed {
        crate::bailout::Bailout.defer();
    }
    object
}

}

/// `SW_SET_CLASS_CREATE(.., sw_zend_create_object_deny)`.
pub(crate) fn deny(entry: &mut zend_class_entry, _: &mut zend_object_handlers) {
    crate::object::set_create(entry, create_object_deny);
}

/// The row of `method` in a method table, with its arginfo.
pub(crate) fn arg_info(methods: Table, method: &CStr) -> Option<&'static zend_function_entry> {
    methods.find(method)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arginfo::{ArgInfo, END, Methods, begin, entry};

    extern "C" fn handler(_: crate::Entered<'_>, _: crate::Returned<'_>) {}

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
