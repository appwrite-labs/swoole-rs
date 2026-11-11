//! Swoole's experimental stdext (`SW_STDEXT`, with the `php` feature; see
//! docs/zend.md): typed arrays, methods on arrays, strings and streams, and
//! the `swoole_*` functions behind them.
//!
//! Rust owns the complete `ASSIGN_DIM` and `ASSIGN_DIM_OP` operations,
//! operand/result/refcount order, PCRE cache custody and opcode installation.
//! src/stdext.c (README.md) exposes only individual private Zend VM, PCRE and
//! JSON library effects.
//!
//! PHP code (autoloaders, callbacks, destructors) runs under `zend_try` in the
//! C shim; a handler that trapped a bailout re-raises it as its last act, when
//! its frame owns nothing.
#![cfg(all(feature = "php", feature = "active"))]
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

mod aliases;
mod assignment;
mod invoke;
mod text;
mod typed;

pub use assignment::{swr_v1_phpstdext_assign, swr_v1_phpstdext_assign_op};
pub use invoke::swr_v1_phpstdext_method_call;
pub use typed::{
    swr_v1_phpstdext_check, swr_v1_phpstdext_foreach_begin, swr_v1_phpstdext_operation,
    swr_v1_phpstdext_unset,
};

use std::ffi::c_int;
use swoole_rs_zend::{
    sys::{IS_INDIRECT, zend_execute_data, zend_op, znode_op, zval},
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

/// Install the complete Rust-owned opcode handlers, then stdext's function overrides.
/// # Safety
/// Called once at MINIT, before any PHP code executes.
#[expect(
    clippy::cast_possible_truncation,
    reason = "Zend opcode constants are encoded in its opcode byte"
)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpstdext_minit() {
    for (opcode, handler) in [
        (
            swoole_rs_zend::sys::ZEND_INIT_METHOD_CALL,
            swr_v1_phpstdext_method_call as unsafe extern "C" fn(*mut zend_execute_data) -> c_int,
        ),
        (
            swoole_rs_zend::sys::ZEND_ASSIGN_DIM,
            swr_v1_phpstdext_assign,
        ),
        (
            swoole_rs_zend::sys::ZEND_ASSIGN_DIM_OP,
            swr_v1_phpstdext_assign_op,
        ),
        (swoole_rs_zend::sys::ZEND_UNSET_DIM, swr_v1_phpstdext_unset),
        (
            swoole_rs_zend::sys::ZEND_FE_RESET_RW,
            swr_v1_phpstdext_foreach_begin,
        ),
    ] {
        // SAFETY: each callback obeys Zend's user opcode ABI and lives for the process.
        unsafe { swoole_rs_zend::sys::zend_set_user_opcode_handler(opcode as u8, Some(handler)) };
    }
    // SAFETY: install function lookups and overrides at MINIT in the original order.
    unsafe { invoke::install() };
    // SAFETY: PHP's function table is complete and no request has started.
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
