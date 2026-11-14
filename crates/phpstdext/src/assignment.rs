//! Complete typed-array assignment opcode operations over Zend's actual frame.
use crate::{
    CONTINUE, DISPATCH, Frame, Op, offset,
    typed::{self, KeyType, Typed},
};
use std::{borrow::Cow, ffi::c_int, ptr};
use swoole_rs_zend::{
    Owned, Value,
    bailout::{self, Bailout},
    opcode::Handler,
    sys::{self as z, zend_execute_data, zend_op, zend_reference, zval},
};

unsafe extern "C" {
    fn swr_phpstdext_undefined_cv(frame: *mut zend_execute_data, var: u32) -> u8;
    fn swr_phpstdext_dimension(
        frame: *mut zend_execute_data,
        table: *mut z::HashTable,
        key: *const zval,
        constant: bool,
        result: *mut *mut zval,
    ) -> u8;
    fn swr_phpstdext_assign(
        slot: *mut zval,
        value: *mut zval,
        kind: u8,
        strict: bool,
        result: *mut *mut zval,
    ) -> u8;
    fn swr_phpstdext_binary(slot: *mut zval, value: *mut zval, op: *const zend_op) -> u8;
    fn swr_phpstdext_typed_ref(
        frame: *mut zend_execute_data,
        reference: *mut zend_reference,
        value: *mut zval,
        op: *const zend_op,
    ) -> u8;
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
            // SAFETY: like ZVAL_NULL, overwrite only the result's type information.
            unsafe {
                (*slot.as_ptr()).u1 = z::_zval_struct__bindgen_ty_1 {
                    type_info: z::IS_NULL,
                }
            };
        }
        Ok(())
    }

    /// Publish `element` as `op`'s result, then free the operand.
    fn publish(self, op: Op, element: &Value) -> Result<(), Bailout> {
        if let Some(result) = op.result() {
            // SAFETY: two live zvals; a retained copy of the actual
            // post-operation element.
            unsafe { z::swr_zval_copy(result.as_ptr(), element.as_ptr()) };
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
    // SAFETY: Zend reads its own CV name and invokes any warning handler under a native trap.
    leaf(unsafe { swr_phpstdext_undefined_cv(data.frame.as_ptr(), offset(data.op1)) })?;
    Ok(Some(Cow::Owned(Owned::null())))
}

/// Whether the active function declares `strict_types`.
fn strict(frame: Frame) -> bool {
    frame.function().flags() & z::ZEND_ACC_STRICT_TYPES != 0
}

/// `ASSIGN_DIM` into `own`, the typed table the container holds unshared.
fn assign(op: Op, data: Data, own: Typed, key: &Value) -> Result<(), Bailout> {
    let value = data.value();
    let ht = own.table();
    let mut slot = ptr::null_mut();
    if key.kind() == z::IS_NULL {
        // SAFETY: a live table, which takes the original operand reference on
        // successful insertion.
        slot = unsafe { z::zend_hash_next_index_insert(ht, value.as_ptr()) };
        if slot.is_null() {
            // SAFETY: preserve Zend's exact occupied-next-index error, below a native trap.
            leaf(unsafe { z::swr_zend_try_stdext_cannot_add() })?;
            // This assignment failed before publishing its result.
            return data.abandon(op);
        }
    } else {
        let mut variable = ptr::null_mut();
        // SAFETY: lookup uses this assignment's original constant-key distinction.
        leaf(unsafe {
            swr_phpstdext_dimension(
                op.frame.as_ptr(),
                ht,
                key.as_ptr(),
                u32::from(op.op2_type) == z::IS_CONST,
                &raw mut variable,
            )
        })?;
        if variable.is_null() {
            // The native dimension lookup failed before assignment.
            return data.abandon(op);
        }
        let strict = strict(op.frame);
        // SAFETY: Zend performs its original assignment/refcount effect below a trap.
        leaf(unsafe {
            swr_phpstdext_assign(variable, value.as_ptr(), data.kind(), strict, &raw mut slot)
        })?;
        if slot.is_null() {
            // The native assignment failed before result publication.
            return data.abandon(op);
        }
    }
    if let Some(result) = op.result() {
        // SAFETY: Zend's copy retains the inserted/assigned payload for the result.
        unsafe { z::swr_zval_copy(result.as_ptr(), slot) };
    }
    if u32::from(data.kind()) == z::IS_VAR {
        // Retain the variable value before the original FREE_OP decrements it.
        value.add_ref();
    }
    // The assignment's original OP_DATA temporary/variable.
    data.free()
}

/// `ASSIGN_DIM_OP` on `own`, as [`assign`].
fn assign_op(op: Op, data: Data, own: Typed, key: &Value) -> Result<(), Bailout> {
    let value = data.value();
    let ht = own.table();
    let mut slot = ptr::null_mut();
    // Upstream selects W_CONST from the value's OP_DATA type here, unlike ASSIGN_DIM.
    // SAFETY: retain that exact distinction when invoking the native VM primitive.
    leaf(unsafe {
        swr_phpstdext_dimension(
            op.frame.as_ptr(),
            ht,
            key.as_ptr(),
            u32::from(data.kind()) == z::IS_CONST,
            &raw mut slot,
        )
    })?;
    // SAFETY: null, or the live writable array-element slot the lookup returned.
    let Some(element) = (unsafe { Value::from_ptr(slot) }) else {
        // Dimension lookup failed before the operation's result exists.
        return data.abandon(op);
    };
    let target = element.referent();
    // The reference whose typed property sources Zend must verify, if any.
    if let Some(typed_reference) = element.typed_reference() {
        // SAFETY: the native typed-reference operation preserves PHP source
        // verification and strict types.
        leaf(unsafe {
            swr_phpstdext_typed_ref(op.frame.as_ptr(), typed_reference, value.as_ptr(), op.at)
        })?;
    } else {
        if !typed::operation(own, op.extended_value == z::ZEND_CONCAT) {
            // A rejected operation follows the original temporary/result cleanup path.
            return data.abandon(op);
        }
        // SAFETY: Zend applies the exact current opcode to the live element and value.
        leaf(unsafe { swr_phpstdext_binary(target, value.as_ptr(), op.at) })?;
    }
    // SAFETY: the element's live slot, or its reference's, after the operation.
    let Some(target) = (unsafe { Value::from_ptr(target) }) else {
        // Never: a referent is not null.
        return data.abandon(op);
    };
    data.publish(op, target)
}

/// `ASSIGN_DIM` (or, for `operation`, `ASSIGN_DIM_OP`) at `frame`'s instruction.
/// # Safety
/// `frame` is at an `ASSIGN_DIM` or `ASSIGN_DIM_OP` instruction.
unsafe fn dispatch(frame: Frame, operation: bool) -> Result<c_int, Bailout> {
    let op = Op::current(frame);
    let Some(array) = op.op1_container() else {
        return Ok(DISPATCH);
    };
    let Some(typed) = Typed::of(array) else {
        return Ok(DISPATCH);
    };
    // An append's key is null (`&EG(uninitialized_zval)`).
    let null = Owned::null();
    let key = op.op2().unwrap_or(&null);
    // An assignment, which its `OP_DATA` instruction follows.
    let Some(data) = frame.next_instruction() else {
        return Ok(DISPATCH);
    };
    let data = Op::at(frame, data);
    // `get_op_data_zval_ptr_r`: None for an unused operand. Only `data` frees it.
    let Some(value) = operand(data)? else {
        return Ok(DISPATCH);
    };
    let data = Data { op: data, value };
    if !typed::check(typed, KeyType::of(key), data.value()) {
        // Rejection frees OP_DATA and intentionally does not advance the opline.
        data.free()?;
        return Ok(CONTINUE);
    }
    // SAFETY: `array` holds `typed`, read from it above and not used after this.
    let own = unsafe { typed::separate(array, typed) };
    if operation {
        assign_op(op, data, own, key)?;
    } else {
        assign(op, data, own, key)?;
    }
    // Reload the current frame opline after reentry, as EX(opline) += 2 did.
    frame.skip(2);
    Ok(CONTINUE)
}

/// `ASSIGN_DIM`'s complete typed-array opcode handler.
pub(crate) struct Assign;
/// `ASSIGN_DIM_OP`'s complete typed-array opcode handler.
pub(crate) struct AssignOp;

impl Handler for Assign {
    fn run(frame: Frame<'_>) -> c_int {
        // SAFETY: installed for `ASSIGN_DIM` only (lib.rs).
        finish(unsafe { dispatch(frame, false) })
    }
}

impl Handler for AssignOp {
    fn run(frame: Frame<'_>) -> c_int {
        // SAFETY: installed for `ASSIGN_DIM_OP` only (lib.rs).
        finish(unsafe { dispatch(frame, true) })
    }
}

fn finish(result: Result<c_int, Bailout>) -> c_int {
    let code = match result {
        Ok(code) => code,
        Err(error) => {
            error.defer();
            CONTINUE
        }
    };
    bailout::finish();
    code
}
