//! Complete typed-array assignment opcode operations over Zend's actual frame.
use crate::{
    Frame, Next, Op,
    typed::{self, KeyType, Typed},
};
use std::{borrow::Cow, ffi::c_int};
use swoole_rs_zend::{
    Owned, Value,
    bailout::Bailout,
    opcode::Handler,
    sys::{self as z, zval},
};

// stdext.c's VM primitives. The `Frame` ones read what they need of the
// frame's current instruction and do nothing (0) unless it is an `ASSIGN_DIM`
// or `ASSIGN_DIM_OP` (`binary` and `typed_ref`: only the latter), whose
// `OP_DATA` follows it, so a live frame and live zvals are all they trust. A
// bailout inside any of them is trapped (nonzero).
unsafe extern "C" {
    /// Zend's "Undefined variable" warning for the assignment's `OP_DATA`
    /// operand, when it is a CV.
    safe fn swr_phpstdext_undefined_cv(frame: Frame<'_>) -> u8;
    /// The writable element of the array `container` holds (separated first)
    /// for the instruction's key, into `result`, left alone on a bailout: none
    /// when the lookup failed, else a slot of the table that lives while
    /// nothing else changes it.
    safe fn swr_phpstdext_dimension<'a>(
        frame: Frame<'_>,
        container: &'a Value,
        result: &mut Option<&'a Value>,
    ) -> u8;
    /// `zend_assign_to_variable` into `result`, left alone on a bailout: the
    /// assigned slot, none when the assignment failed.
    fn swr_phpstdext_assign(
        slot: *mut zval,
        value: *mut zval,
        kind: u8,
        strict: bool,
        result: &mut Option<&Value>,
    ) -> u8;
    /// The instruction's binary operation on `slot` and `value`, into `slot`.
    safe fn swr_phpstdext_binary(frame: Frame<'_>, slot: &Value, value: &Value) -> u8;
    /// The same on the reference `element` holds, verified against the types
    /// of the properties it is a source of: nothing unless it has any.
    /// `typed` says whether it had (`ZEND_REF_HAS_TYPE_SOURCES`).
    safe fn swr_phpstdext_typed_ref(
        frame: Frame<'_>,
        element: &Value,
        value: &Value,
        typed: &mut bool,
    ) -> u8;
    /// The new element's slot, none when the next index is taken.
    #[link_name = "zend_hash_next_index_insert"]
    fn next_index_insert<'a>(table: *mut z::HashTable, value: *mut zval) -> Option<&'a Value>;
}
// crates/zend-sys' shims that take only live zvals, or nothing.
unsafe extern "C" {
    /// `ZVAL_COPY(target, source)`: `target` takes a new reference to what
    /// `source` holds; what `target` held is left alone, never released.
    #[link_name = "swr_zval_copy"]
    safe fn copy_into(target: &Value, source: &Value);
}

fn leaf(code: impl Into<c_int>) -> Result<(), Bailout> {
    if code.into() == 0 {
        Ok(())
    } else {
        Err(Bailout)
    }
}

/// An assignment's `OP_DATA` instruction with the value it read, whose operand
/// this still owns: [`Data::free`] (or `abandon`, or `publish`) consumes it to
/// release the operand, once, and nothing reads the value after that.
struct Data<'a> {
    op: Op<'a>,
    value: Cow<'a, Value>,
}

impl Data<'_> {
    /// The operand's value, until it is freed.
    fn value(&self) -> &Value {
        &self.value
    }

    /// The operand's type (`IS_CONST`, `IS_TMP_VAR`, `IS_VAR`, `IS_CV`).
    fn kind(&self) -> u8 {
        self.op.op1_type
    }

    /// Zend's `FREE_OP`, with its original no-GC destructor variant and branch.
    fn free(self) -> Result<(), Bailout> {
        if u32::from(self.op.op1_type) & (z::IS_TMP_VAR | z::IS_VAR) != 0
            && let Some(value) = self.op.op1()
        {
            // SAFETY: the operand's temporary slot, which this owned and gives
            // up here; its destructor is trapped below Rust.
            leaf(unsafe { z::swr_zend_try_stdext_nogc_dtor(value.as_ptr()) })?;
        }
        Ok(())
    }

    /// The original failed dimension/assignment path frees the value before
    /// NULL publication.
    fn abandon(self, op: Op) -> Result<(), Bailout> {
        self.free()?;
        if let Some(slot) = op.result() {
            // ZVAL_NULL: the result slot held nothing to release.
            copy_into(slot, &Owned::null());
        }
        Ok(())
    }

    /// Publish `element` as `op`'s result, then free the operand.
    fn publish(self, op: Op, element: &Value) -> Result<(), Bailout> {
        if let Some(result) = op.result() {
            // A retained copy of the actual post-operation element.
            copy_into(result, element);
        }
        self.free()
    }
}

/// `get_op_data_zval_ptr_r`: Rust selects the actual operand; only an undefined
/// CV's PHP warning is native, and it reads as null (`EG(uninitialized_zval)`).
/// References intentionally remain undereferenced.
fn operand(data: Op<'_>) -> Result<Option<Cow<'_, Value>>, Bailout> {
    let Some(value) = data.op1() else {
        return Ok(None);
    };
    if u32::from(data.op1_type) != z::IS_CV || value.kind() != z::IS_UNDEF {
        return Ok(Some(Cow::Borrowed(value)));
    }
    // Zend reads its own CV name and runs any warning handler under a native trap.
    leaf(swr_phpstdext_undefined_cv(data.frame))?;
    Ok(Some(Cow::Owned(Owned::null())))
}

/// Whether the active function declares `strict_types`.
fn strict(frame: Frame) -> bool {
    frame.function().flags() & z::ZEND_ACC_STRICT_TYPES != 0
}

/// `ASSIGN_DIM` into `own`, the typed table `container` holds unshared.
fn assign(op: Op, data: Data, own: Typed, container: &Value, key: &Value) -> Result<(), Bailout> {
    let value = data.value();
    let ht = own.table();
    let slot = if key.kind() == z::IS_NULL {
        // SAFETY: a live table, which takes the original operand reference on
        // successful insertion.
        let slot = unsafe { next_index_insert(ht, value.as_ptr()) };
        if slot.is_none() {
            // Zend's exact occupied-next-index error; this handler's frame is
            // executing, so Zend only sets the exception.
            swoole_rs_zend::throw_error(
                c"Cannot add element to the array as the next element is already occupied",
            );
            // This assignment failed before publishing its result.
            return data.abandon(op);
        }
        slot
    } else {
        let mut variable = None;
        // The lookup keeps this assignment's original constant-key distinction.
        leaf(swr_phpstdext_dimension(op.frame, container, &mut variable))?;
        let Some(variable) = variable else {
            // The native dimension lookup failed before assignment.
            return data.abandon(op);
        };
        let strict = strict(op.frame);
        let mut slot = None;
        // SAFETY: Zend performs its original assignment/refcount effect below a trap.
        leaf(unsafe {
            swr_phpstdext_assign(
                variable.as_ptr(),
                value.as_ptr(),
                data.kind(),
                strict,
                &mut slot,
            )
        })?;
        slot
    };
    let Some(slot) = slot else {
        // The native assignment failed before result publication.
        return data.abandon(op);
    };
    if let Some(result) = op.result() {
        // Zend's copy retains the inserted/assigned payload for the result.
        copy_into(result, slot);
    }
    if u32::from(data.kind()) == z::IS_VAR {
        // Retain the variable value before the original FREE_OP decrements it.
        value.add_ref();
    }
    // The assignment's original OP_DATA temporary/variable.
    data.free()
}

/// `ASSIGN_DIM_OP` on `own`, the typed table `container` holds unshared.
fn assign_op(op: Op, data: Data, own: Typed, container: &Value) -> Result<(), Bailout> {
    let value = data.value();
    let mut slot = None;
    // Upstream selects W_CONST from the value's OP_DATA type here, unlike
    // ASSIGN_DIM; the primitive keeps that distinction.
    leaf(swr_phpstdext_dimension(op.frame, container, &mut slot))?;
    let Some(element) = slot else {
        // Dimension lookup failed before the operation's result exists.
        return data.abandon(op);
    };
    // The element's slot, or its reference's, which the element keeps alive.
    let target = element.referred();
    // On a reference whose typed property sources Zend must verify, the
    // native typed-reference operation preserves PHP source verification and
    // strict types.
    let mut typed = false;
    leaf(swr_phpstdext_typed_ref(
        op.frame, element, value, &mut typed,
    ))?;
    if !typed {
        if !typed::operation(own, op.extended_value == z::ZEND_CONCAT) {
            // A rejected operation follows the original temporary/result cleanup path.
            return data.abandon(op);
        }
        // Zend applies the exact current opcode to the live element and value.
        leaf(swr_phpstdext_binary(op.frame, target, value))?;
    }
    data.publish(op, target)
}

/// `ASSIGN_DIM` (or, for `operation`, `ASSIGN_DIM_OP`) at `frame`'s instruction; an
/// instruction of any other opcode is left to Zend.
fn dispatch(frame: Frame, operation: bool) -> Result<Next, Bailout> {
    let opcode = if operation {
        z::ZEND_ASSIGN_DIM_OP
    } else {
        z::ZEND_ASSIGN_DIM
    };
    if u32::from(frame.instruction().opcode) != opcode {
        return Ok(Next::Dispatch);
    }
    let op = Op::current(frame);
    let Some(array) = op.op1_container() else {
        return Ok(Next::Dispatch);
    };
    let Some(typed) = Typed::of(array) else {
        return Ok(Next::Dispatch);
    };
    // An append's key is null (`&EG(uninitialized_zval)`).
    let null = Owned::null();
    let key = op.op2().unwrap_or(&null);
    // An assignment, which its `OP_DATA` instruction follows.
    let Some(data) = frame.next_instruction() else {
        return Ok(Next::Dispatch);
    };
    let data = Op::at(frame, data);
    // `get_op_data_zval_ptr_r`: None for an unused operand. Only `data` frees it.
    let Some(value) = operand(data)? else {
        return Ok(Next::Dispatch);
    };
    let data = Data { op: data, value };
    if !typed::check(typed, KeyType::of(key), data.value()) {
        // Rejection frees OP_DATA and intentionally does not advance the opline.
        data.free()?;
        return Ok(Next::Continue);
    }
    // `array` holds `typed`, read from it above: its own, unshared.
    let Some(own) = typed::separate(array) else {
        return data.free().map(|()| Next::Continue);
    };
    if operation {
        assign_op(op, data, own, array)?;
    } else {
        assign(op, data, own, array, key)?;
    }

    // Reload the current frame opline after reentry, as EX(opline) += 2 did.
    frame.skip(2);
    Ok(Next::Continue)
}

/// `ASSIGN_DIM`'s complete typed-array opcode handler.
pub(crate) struct Assign;
/// `ASSIGN_DIM_OP`'s complete typed-array opcode handler.
pub(crate) struct AssignOp;

impl Handler for Assign {
    const OPCODE: u32 = swoole_rs_zend::sys::ZEND_ASSIGN_DIM;
    fn run(frame: Frame<'_>) -> Next {
        finish(dispatch(frame, false))
    }
}

impl Handler for AssignOp {
    const OPCODE: u32 = swoole_rs_zend::sys::ZEND_ASSIGN_DIM_OP;
    fn run(frame: Frame<'_>) -> Next {
        finish(dispatch(frame, true))
    }
}

fn finish(result: Result<Next, Bailout>) -> Next {
    match result {
        Ok(code) => code,
        Err(error) => {
            error.defer();
            Next::Continue
        }
    }
}
