//! Complete typed-array assignment opcode operations over Zend's actual frame.
use crate::{
    CONTINUE, DISPATCH, Frame, Op, offset,
    typed::{self, KeyType, Typed},
};
use std::{borrow::Cow, ffi::c_int, ptr};
use swoole_rs_zend::{
    Owned, Value,
    bailout::{self, Bailout},
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
    /// # Safety
    /// `element` is a live zval.
    unsafe fn publish(self, op: Op, element: *const zval) -> Result<(), Bailout> {
        if let Some(result) = op.result() {
            // SAFETY: per the caller; a retained copy of the actual
            // post-operation element.
            unsafe { z::swr_zval_copy(result.as_ptr(), element) };
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
    // SAFETY: a live execution frame points at its active function.
    let function = unsafe { (*frame.as_ptr()).func };
    // Every zend_function variant begins with this common header.
    let common = function.cast::<z::_zend_function__bindgen_ty_1>();
    // SAFETY: the active function's live common header.
    let flags = unsafe { (*common).fn_flags };
    flags & z::ZEND_ACC_STRICT_TYPES != 0
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
        // SAFETY: retain the variable value before the original FREE_OP decrements it.
        unsafe { z::swr_zval_try_addref(value.as_ptr()) };
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
    if element.kind() == z::IS_REFERENCE {
        // SAFETY: a reference zval holds `ref`, at offset 0.
        let reference = unsafe { element.as_ptr().cast::<*mut zend_reference>().read() };
        // SAFETY: this live reference owns its PHP property-source metadata.
        let sources = unsafe { (*reference).sources };
        // SAFETY: Zend tests the source-list union through its pointer member too.
        if !unsafe { sources.ptr }.is_null() {
            // SAFETY: the native typed-reference operation preserves PHP source
            // verification and strict types.
            leaf(unsafe {
                swr_phpstdext_typed_ref(op.frame.as_ptr(), reference, value.as_ptr(), op.at)
            })?;
            // SAFETY: the reference's live value slot.
            return unsafe { data.publish(op, target) };
        }
    }
    if !typed::operation(own, op.extended_value == z::ZEND_CONCAT) {
        // A rejected operation follows the original temporary/result cleanup path.
        return data.abandon(op);
    }
    // SAFETY: Zend applies the exact current opcode to the live element and value.
    leaf(unsafe { swr_phpstdext_binary(target, value.as_ptr(), op.at) })?;
    // SAFETY: the element's live slot, or its reference's.
    unsafe { data.publish(op, target) }
}

/// # Safety
/// Zend's user opcode handler contract: `frame` is the live frame it passed, at
/// an `ASSIGN_DIM` or `ASSIGN_DIM_OP` instruction.
unsafe fn dispatch(frame: *mut zend_execute_data, operation: bool) -> Result<c_int, Bailout> {
    // SAFETY: per the caller.
    let frame = unsafe { Frame::new(frame) };
    let op = frame.op();
    let Some(array) = op.op1_container() else {
        return Ok(DISPATCH);
    };
    let Some(typed) = Typed::of(array) else {
        return Ok(DISPATCH);
    };
    // An append's key is null (`&EG(uninitialized_zval)`).
    let null = Owned::null();
    let key = op.op2().unwrap_or(&null);
    // SAFETY: per the caller, an assignment, which its `OP_DATA` instruction follows.
    let data = unsafe { Op::at(frame, op.at.wrapping_add(1)) };
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
    frame.jump(frame.opline().wrapping_add(2));
    Ok(CONTINUE)
}

/// `ASSIGN_DIM`'s complete typed-array opcode handler.
/// # Safety
/// Zend supplies a live frame at its assignment instruction.
pub unsafe extern "C" fn swr_v1_phpstdext_assign(frame: *mut zend_execute_data) -> c_int {
    // SAFETY: Zend's user opcode callback contract; it installed this handler
    // for ASSIGN_DIM.
    finish(unsafe { dispatch(frame, false) })
}
/// `ASSIGN_DIM_OP`'s complete typed-array opcode handler.
/// # Safety
/// Zend supplies a live frame at its assignment operation instruction.
pub unsafe extern "C" fn swr_v1_phpstdext_assign_op(frame: *mut zend_execute_data) -> c_int {
    // SAFETY: Zend's user opcode callback contract; it installed this handler
    // for ASSIGN_DIM_OP.
    finish(unsafe { dispatch(frame, true) })
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
