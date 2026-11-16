//! Zend's conversions of a value used as an array offset, for containers that
//! key their own tables as PHP arrays do (crates/phpthread's `Map`).
use crate::{
    Call, Value, ZStr,
    sys::{zend_long, zend_ulong},
};

unsafe extern "C" {
    /// `_zend_handle_numeric_str`: whether a live string is a canonical integer,
    /// which `index` then holds. It reads the string only.
    safe fn swr_zend_handle_numeric_str(key: &ZStr, index: &mut zend_ulong) -> bool;
    /// `Z_RES_HANDLE_P`: the handle of the resource a live zval holds, read
    /// only; 0 for any other value.
    safe fn swr_zend_resource_handle(value: &Value) -> zend_long;
}

impl ZStr {
    /// The integer the string is the canonical spelling of
    /// (`ZEND_HANDLE_NUMERIC_STR`), if any.
    #[must_use]
    pub fn numeric_index(&self) -> Option<zend_ulong> {
        let mut index = 0;
        swr_zend_handle_numeric_str(self, &mut index).then_some(index)
    }
}

impl Value {
    /// `Z_RES_HANDLE_P`: the handle of the resource this holds; 0 for any other value.
    #[must_use]
    pub fn resource_handle(&self) -> zend_long {
        swr_zend_resource_handle(self)
    }
}

impl Call {
    /// A double used as an offset (`zend_dval_to_lval_safe`), which may raise
    /// `E_DEPRECATED` and so run the user's error handler: in the call, which
    /// proves this thread runs PHP.
    #[must_use]
    pub fn double_offset(&self, number: f64) -> zend_long {
        unsafe extern "C" {
            /// `zend_dval_to_lval_safe`, which may raise `E_DEPRECATED`.
            safe fn swr_zend_dval_to_lval_safe(value: f64) -> zend_long;
        }
        swr_zend_dval_to_lval_safe(number)
    }

    /// A resource used as an offset: Zend's warning, which may run the user's error
    /// handler (in the call, which proves this thread runs PHP), then its handle.
    #[must_use]
    pub fn resource_offset(&self, dim: &Value) -> zend_long {
        unsafe extern "C" {
            /// Zend's warning for a resource used as an offset; it reads the live zval.
            safe fn zend_use_resource_as_offset(dim: &Value);
        }
        zend_use_resource_as_offset(dim);
        dim.resource_handle()
    }
}
