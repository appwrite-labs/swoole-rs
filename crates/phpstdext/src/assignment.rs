//! Complete typed-array assignment opcode operations over Zend's actual frame.
use crate::{
    CONTINUE, DISPATCH, Frame, Op, offset,
    typed::{self, KeyType, Typed},
};
use std::{ffi::c_int, ptr};
use swoole_rs_zend::{
    Value,
    bailout::{self, Bailout},
    sys::{self as z, zend_execute_data, zend_op, zend_reference, zval},
    value::table,
};

unsafe extern "C" {
    fn swr_phpstdext_undefined_cv(
        frame: *mut zend_execute_data,
        var: u32,
        result: *mut *mut zval,
    ) -> u8;
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

/// Zend's `FREE_OP`, with its original no-GC destructor variant and branch.
/// # Safety
/// `data`'s operand is still owned: nothing freed it yet.
unsafe fn free_operand(data: Op) -> Result<(), Bailout> {
    if u32::from(data.op1_type) & (z::IS_TMP_VAR | z::IS_VAR) != 0
        && let Some(value) = data.op1()
    {
        // SAFETY: per the caller, the operand's owned temporary slot; its
        // destructor is trapped below Rust.
        leaf(unsafe { z::swr_zend_try_stdext_nogc_dtor(value.as_ptr()) })?;
    }
    Ok(())
}

/// The original failed dimension/assignment path frees the value before NULL publication.
/// # Safety
/// As [`free_operand`].
unsafe fn abandon(op: Op, data: Op) -> Result<(), Bailout> {
    // SAFETY: per the caller.
    unsafe { free_operand(data) }?;
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

/// `get_op_data_zval_ptr_r`: Rust selects the actual operand; only an undefined
/// CV's PHP warning is native. References intentionally remain undereferenced.
fn operand(data: Op<'_>) -> Result<Option<&Value>, Bailout> {
    let Some(value) = data.op1() else {
        return Ok(None);
    };
    if u32::from(data.op1_type) != z::IS_CV || value.kind() != z::IS_UNDEF {
        return Ok(Some(value));
    }
    let mut undefined = ptr::null_mut();
    // SAFETY: Zend reads its own CV name and invokes any warning handler under a native trap.
    leaf(unsafe {
        swr_phpstdext_undefined_cv(data.frame.as_ptr(), offset(data.op1), &raw mut undefined)
    })?;
    // SAFETY: null or Zend's uninitialized zval, which lives for the request.
    Ok(unsafe { Value::from_ptr(undefined) })
}

/// `&EG(uninitialized_zval)`: an append's key.
fn uninitialized<'a>() -> Option<&'a Value> {
    // SAFETY: the executor's global, for the running request.
    let undefined = unsafe { z::swr_zend_uninitialized_zval() };
    // SAFETY: as above; it lives for the request.
    unsafe { Value::from_ptr(undefined) }
}

/// `SEPARATE_ARRAY(container)`, carrying the definition to a copy.
/// # Safety
/// `container` holds `source`, which nothing uses afterwards.
unsafe fn separate(container: &Value, source: Typed) -> Result<*mut z::HashTable, Bailout> {
    // SAFETY: Zend separates this exact live array slot under a native allocation trap.
    leaf(unsafe { z::swr_zend_try_stdext_separate(container.as_ptr()) })?;
    // SAFETY: successful separation leaves the container holding an initialized array.
    let current = unsafe { table(container.as_ptr()) };
    if source.table() != current {
        // SAFETY: source remains shared/immutable; current is the fresh unshared copy.
        unsafe { typed::copy_type(container.as_ptr(), source) }?;
    }
    // SAFETY: copying type metadata may have replaced the header allocation.
    Ok(unsafe { table(container.as_ptr()) })
}

/// Whether the active function declares `strict_types`.
fn strict(frame: Frame) -> bool {
    // SAFETY: a live execution frame points at its active function.
    let function = unsafe { (*frame.as_ptr()).func };
    // SAFETY: this active function has a live common function header.
    let function = unsafe { &*function };
    // SAFETY: every zend_function variant begins with this common header.
    let common = unsafe { function.common };
    common.fn_flags & z::ZEND_ACC_STRICT_TYPES != 0
}

/// # Safety
/// `container` holds `source`; `data`'s operand is still owned.
unsafe fn assign(
    op: Op,
    data: Op,
    container: &Value,
    source: Typed,
    key: &Value,
    value: &Value,
) -> Result<(), Bailout> {
    // SAFETY: per the caller.
    let ht = unsafe { separate(container, source) }?;
    let mut slot = ptr::null_mut();
    if key.kind() == z::IS_NULL {
        // SAFETY: the table takes the original operand reference on successful insertion.
        leaf(unsafe { z::swr_zend_try_stdext_append(ht, value.as_ptr(), &raw mut slot) })?;
        if slot.is_null() {
            // SAFETY: preserve Zend's exact occupied-next-index error, below a native trap.
            leaf(unsafe { z::swr_zend_try_stdext_cannot_add() })?;
            // SAFETY: this assignment failed before publishing its result.
            return unsafe { abandon(op, data) };
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
            // SAFETY: the native dimension lookup failed before assignment.
            return unsafe { abandon(op, data) };
        }
        let strict = strict(op.frame);
        // SAFETY: Zend performs its original assignment/refcount effect below a trap.
        leaf(unsafe {
            swr_phpstdext_assign(
                variable,
                value.as_ptr(),
                data.op1_type,
                strict,
                &raw mut slot,
            )
        })?;
        if slot.is_null() {
            // SAFETY: the native assignment failed before result publication.
            return unsafe { abandon(op, data) };
        }
    }
    if let Some(result) = op.result() {
        // SAFETY: Zend's copy retains the inserted/assigned payload for the result.
        unsafe { z::swr_zval_copy(result.as_ptr(), slot) };
    }
    if u32::from(data.op1_type) == z::IS_VAR {
        // SAFETY: retain the variable value before the original FREE_OP decrements it.
        unsafe { z::swr_zval_try_addref(value.as_ptr()) };
    }
    // SAFETY: this is the assignment's original OP_DATA temporary/variable.
    unsafe { free_operand(data) }
}

/// # Safety
/// As [`assign`].
unsafe fn assign_op(
    op: Op,
    data: Op,
    container: &Value,
    source: Typed,
    key: &Value,
    value: &Value,
) -> Result<(), Bailout> {
    // SAFETY: per the caller.
    let ht = unsafe { separate(container, source) }?;
    let mut slot = ptr::null_mut();
    // Upstream selects W_CONST from the value's OP_DATA type here, unlike ASSIGN_DIM.
    // SAFETY: retain that exact distinction when invoking the native VM primitive.
    leaf(unsafe {
        swr_phpstdext_dimension(
            op.frame.as_ptr(),
            ht,
            key.as_ptr(),
            u32::from(data.op1_type) == z::IS_CONST,
            &raw mut slot,
        )
    })?;
    // SAFETY: null, or the live writable array-element slot the lookup returned.
    let Some(element) = (unsafe { Value::from_ptr(slot) }) else {
        // SAFETY: dimension lookup failed before the operation's result exists.
        return unsafe { abandon(op, data) };
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
            // SAFETY: per the caller.
            return unsafe { publish(op, data, target) };
        }
    }
    // SAFETY: the container holds its own typed copy now.
    let typed = unsafe { Typed::from_table(ht) };
    if !typed::operation(typed, op.extended_value == z::ZEND_CONCAT) {
        // SAFETY: a rejected operation follows the original temporary/result cleanup path.
        return unsafe { abandon(op, data) };
    }
    // SAFETY: Zend applies the exact current opcode to the live element and value.
    leaf(unsafe { swr_phpstdext_binary(target, value.as_ptr(), op.at) })?;
    // SAFETY: per the caller.
    unsafe { publish(op, data, target) }
}

/// Publish `element` as `op`'s result, then free `data`'s operand.
/// # Safety
/// As [`free_operand`].
unsafe fn publish(op: Op, data: Op, element: *const zval) -> Result<(), Bailout> {
    if let Some(result) = op.result() {
        // SAFETY: publish a retained copy of the actual post-operation element.
        unsafe { z::swr_zval_copy(result.as_ptr(), element) };
    }
    // SAFETY: per the caller; released after result publication.
    unsafe { free_operand(data) }
}

/// # Safety
/// `frame` is at an `ASSIGN_DIM` or `ASSIGN_DIM_OP` instruction.
unsafe fn dispatch(frame: Frame, operation: bool) -> Result<c_int, Bailout> {
    let op = frame.op();
    let Some(array) = op.op1_container() else {
        return Ok(DISPATCH);
    };
    let Some(typed) = Typed::of(array) else {
        return Ok(DISPATCH);
    };
    let Some(key) = op.op2().or_else(uninitialized) else {
        return Ok(DISPATCH);
    };
    // SAFETY: per the caller, an assignment, which OP_DATA follows.
    let data = unsafe { op.data() };
    let Some(value) = operand(data)? else {
        return Ok(DISPATCH);
    };
    if !typed::check(typed, KeyType::of(key), value) {
        // SAFETY: rejection frees OP_DATA and intentionally does not advance the opline.
        unsafe { free_operand(data) }?;
        return Ok(CONTINUE);
    }
    if operation {
        // SAFETY: the typed operands and original instruction remain live throughout this operation.
        unsafe { assign_op(op, data, array, typed, key, value) }?;
    } else {
        // SAFETY: the typed operands and original instruction remain live throughout this assignment.
        unsafe { assign(op, data, array, typed, key, value) }?;
    }
    // Reload the current frame opline after reentry, as EX(opline) += 2 did.
    frame.jump(frame.opline().wrapping_add(2));
    Ok(CONTINUE)
}

/// `ASSIGN_DIM`'s complete typed-array opcode handler.
/// # Safety
/// Zend supplies a live frame at its assignment instruction.
pub unsafe extern "C" fn swr_v1_phpstdext_assign(frame: *mut zend_execute_data) -> c_int {
    // SAFETY: this native entry follows Zend's user opcode callback contract.
    let frame = unsafe { Frame::new(frame) };
    // SAFETY: Zend installed this handler for ASSIGN_DIM.
    finish(unsafe { dispatch(frame, false) })
}
/// `ASSIGN_DIM_OP`'s complete typed-array opcode handler.
/// # Safety
/// Zend supplies a live frame at its assignment operation instruction.
pub unsafe extern "C" fn swr_v1_phpstdext_assign_op(frame: *mut zend_execute_data) -> c_int {
    // SAFETY: this native entry follows Zend's user opcode callback contract.
    let frame = unsafe { Frame::new(frame) };
    // SAFETY: Zend installed this handler for ASSIGN_DIM_OP.
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
