//! Swoole's experimental stdext (`SW_STDEXT`, with the `php` feature; see
//! docs/zend.md): typed arrays, methods on arrays, strings and streams, and
//! the `swoole_*` functions behind them.
//!
//! What stays C++ in `ext-src/swoole_stdext.cc` is what needs PHP's own VM
//! internals (`thirdparty/php/zend/zend_execute.c`): the `ASSIGN_DIM` and
//! `ASSIGN_DIM_OP` handlers, which call [`swr_v1_phpstdext_check`] and its
//! neighbours, and the PCRE and JSON library calls, lent as [`Ops`].
//!
//! PHP code (autoloaders, callbacks, destructors) runs under `zend_try` in the
//! C shim; a handler that trapped a bailout re-raises it as its last act, when
//! its frame owns nothing.
#![cfg(feature = "php")]
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

mod aliases;
mod invoke;
mod text;
mod typed;

pub use invoke::swr_v1_phpstdext_method_call;
pub use text::Ops;
pub use typed::{
    swr_v1_phpstdext_check, swr_v1_phpstdext_copy_type, swr_v1_phpstdext_foreach_begin,
    swr_v1_phpstdext_operation, swr_v1_phpstdext_unset,
};

use std::ffi::c_int;
use swoole_rs_zend::{
    bailout::Bailout,
    sys::{
        _zend_bailout, IS_INDIRECT, IS_REFERENCE, zend_execute_data, zend_op, zend_reference,
        znode_op, zval,
    },
    type_of,
};

/// `ZEND_USER_OPCODE_CONTINUE` and `ZEND_USER_OPCODE_DISPATCH`; the C shim
/// asserts their values.
const CONTINUE: c_int = 0;
const DISPATCH: c_int = 2;

const _: () = assert!(
    size_of::<usize>() == 8,
    "RT_CONSTANT below is the 64-bit (relative) form"
);

/// MINIT, after C++ set the opcode handlers: look up the functions stdext
/// calls and wraps.
/// # Safety
/// At MINIT; `ops` lives for the process.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpstdext_minit(ops: *const Ops) {
    // SAFETY: per the caller.
    unsafe { text::install(ops) };
    // SAFETY: at MINIT.
    unsafe { invoke::install() };
    // SAFETY: at MINIT.
    unsafe { typed::install() };
}

/// `EX_VAR(var)`.
#[inline]
fn ex_var(frame: *mut zend_execute_data, var: u32) -> *mut zval {
    frame.wrapping_byte_add(var as usize).cast()
}

/// `RT_CONSTANT(opline, node)`.
/// # Safety
/// `node` is a constant operand of `opline`.
#[inline]
unsafe fn rt_constant(opline: *const zend_op, node: znode_op) -> *mut zval {
    // SAFETY: a constant operand holds its literal's offset.
    let offset = unsafe { node.constant }.cast_signed();
    opline
        .wrapping_byte_offset(offset as isize)
        .cast_mut()
        .cast()
}

/// `_get_zval_ptr_ptr_var(var)`: the variable, through an indirection.
/// # Safety
/// `var` is a live variable slot of `frame`.
#[inline]
unsafe fn ptr_ptr_var(frame: *mut zend_execute_data, var: u32) -> *mut zval {
    let slot = ex_var(frame, var);
    // SAFETY: per the caller.
    if unsafe { type_of(slot) } == IS_INDIRECT {
        // SAFETY: as above.
        let value = unsafe { (*slot).value };
        // SAFETY: an indirect zval holds `zv`.
        return unsafe { value.zv };
    }
    slot
}

/// `ZVAL_DEREF`.
/// # Safety
/// `value` is a live zval.
#[inline]
unsafe fn deref(value: *mut zval) -> *mut zval {
    // SAFETY: per the caller.
    if unsafe { type_of(value) } == IS_REFERENCE {
        // SAFETY: as above.
        let inner = unsafe { (*value).value };
        // SAFETY: a reference zval holds `ref_`.
        let reference = unsafe { inner.ref_ };
        return reference
            .wrapping_byte_add(std::mem::offset_of!(zend_reference, val))
            .cast();
    }
    value
}

/// Re-raise a trapped bailout: a handler's last act, when it owns nothing.
fn raise(bailout: &Result<(), Bailout>) {
    if bailout.is_err() {
        // SAFETY: Zend's own `zend_bailout()`; see the crate note.
        unsafe { _zend_bailout(c"swoole_stdext".as_ptr().cast_mut(), 0) };
    }
}
