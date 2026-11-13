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

use swoole_rs_zend::{
    Value,
    opcode::{CONTINUE, DISPATCH, Frame, handler},
    sys::{
        IS_CONST, IS_CV, IS_INDIRECT, IS_UNUSED, IS_VAR, user_opcode_handler_t, zend_op, znode_op,
        zval,
    },
};

const _: () = assert!(
    size_of::<usize>() == 8,
    "RT_CONSTANT below is the 64-bit (relative) form"
);

/// Install the complete Rust-owned opcode handlers, then stdext's function overrides:
/// a MINIT step, which the token shows runs before any PHP code executes.
#[expect(
    clippy::cast_possible_truncation,
    reason = "Zend opcode constants are encoded in its opcode byte"
)]
pub fn stdext_minit(minit: &swoole_rs_zend::Minit) {
    let handlers: [(u32, user_opcode_handler_t); 5] = [
        (
            swoole_rs_zend::sys::ZEND_INIT_METHOD_CALL,
            Some(handler::<invoke::MethodCall>),
        ),
        (
            swoole_rs_zend::sys::ZEND_ASSIGN_DIM,
            Some(handler::<assignment::Assign>),
        ),
        (
            swoole_rs_zend::sys::ZEND_ASSIGN_DIM_OP,
            Some(handler::<assignment::AssignOp>),
        ),
        (
            swoole_rs_zend::sys::ZEND_UNSET_DIM,
            Some(handler::<typed::Unset>),
        ),
        (
            swoole_rs_zend::sys::ZEND_FE_RESET_RW,
            Some(handler::<typed::ForeachBegin>),
        ),
    ];
    for (opcode, handler) in handlers {
        // SAFETY: MINIT; each handler is installed for the one opcode its body
        // is written for, and lives for the process.
        unsafe { swoole_rs_zend::sys::zend_set_user_opcode_handler(opcode as u8, handler) };
    }
    // Function lookups, then overrides, in the original order.
    invoke::install();
    typed::install(minit);
}

/// An operand's `var` or `constant`: a byte offset either way.
fn offset(node: znode_op) -> u32 {
    // SAFETY: every member of the union is a `u32`.
    unsafe { node.var }
}

const _: () = assert!(
    size_of::<znode_op>() == size_of::<u32>(),
    "znode_op holds one u32 on 64-bit builds"
);

/// An instruction of a live frame's function: its fields, copied so no
/// borrow of the script outlives a VM effect.
#[derive(Clone, Copy)]
struct Op<'a> {
    frame: Frame<'a>,
    at: *const zend_op,
    op1: znode_op,
    op2: znode_op,
    result: znode_op,
    extended_value: u32,
    op1_type: u8,
    op2_type: u8,
    result_type: u8,
}

impl<'a> Op<'a> {
    /// `frame`'s current instruction.
    fn current(frame: Frame<'a>) -> Self {
        // SAFETY: the frame's current instruction.
        unsafe { Self::at(frame, frame.opline()) }
    }

    /// # Safety
    /// `at` is an instruction of `frame`'s function.
    unsafe fn at(frame: Frame<'a>, at: *const zend_op) -> Self {
        // SAFETY: per the caller, a live plain C record.
        let op = unsafe { &*at };
        Self {
            frame,
            at,
            op1: op.op1,
            op2: op.op2,
            result: op.result,
            extended_value: op.extended_value,
            op1_type: op.op1_type,
            op2_type: op.op2_type,
            result_type: op.result_type,
        }
    }

    /// An operand of type `kind`: its literal, its slot, or None if unused.
    fn operand(self, kind: u8, node: znode_op) -> Option<&'a Value> {
        let operand = match u32::from(kind) {
            IS_UNUSED => return None,
            // `RT_CONSTANT(opline, node)`.
            IS_CONST => self
                .at
                .wrapping_byte_offset(offset(node).cast_signed() as isize)
                .cast(),
            // `EX_VAR(node.var)`.
            _ => self
                .frame
                .as_ptr()
                .wrapping_byte_add(offset(node) as usize)
                .cast(),
        };
        // SAFETY: a constant operand's offset leads from its instruction to
        // the literal the script holds, and a variable operand's to a live
        // slot of its frame; `Value` is a transparent zval.
        unsafe { Value::from_ptr(operand) }
    }

    fn op1(self) -> Option<&'a Value> {
        self.operand(self.op1_type, self.op1)
    }

    fn op2(self) -> Option<&'a Value> {
        self.operand(self.op2_type, self.op2)
    }

    /// The result slot, if the result is used.
    fn result(self) -> Option<&'a Value> {
        self.operand(self.result_type, self.result)
    }

    /// `_get_zval_ptr_ptr_var` and `ZVAL_DEREF`: the slot op1's variable
    /// holds its value in, through an indirection and a reference.
    fn op1_container(self) -> Option<&'a Value> {
        let mut slot = self.op1()?;
        if matches!(u32::from(self.op1_type), IS_VAR | IS_CV) && slot.kind() == IS_INDIRECT {
            // SAFETY: an indirect zval holds `zv`, at offset 0.
            let target = unsafe { slot.as_ptr().cast::<*mut zval>().read() };
            // SAFETY: the live slot it points to.
            slot = unsafe { Value::from_ptr(target) }?;
        }
        // SAFETY: the variable's own slot, or its reference's, which the
        // opcode handler works on in place, as Zend's does.
        unsafe { Value::from_ptr(slot.referent()) }
    }
}
