//! Complete typed-array assignment opcode operations over Zend's actual frame.
use crate::{CONTINUE, DISPATCH, deref, ex_var, ptr_ptr_var, rt_constant, typed};
use std::{ffi::c_int, mem::offset_of, ptr};
use swoole_rs_zend::{
    bailout::{self, Bailout},
    sys::{self as z, zend_execute_data, zend_op, zend_reference, zval},
    type_of,
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
unsafe fn free_operand(frame: *mut zend_execute_data, data: &zend_op) -> Result<(), Bailout> {
    if u32::from(data.op1_type) & (z::IS_TMP_VAR | z::IS_VAR) != 0 {
        // SAFETY: a temporary/variable node stores its valid byte offset in this frame.
        let var = unsafe { data.op1.var };
        // SAFETY: this is the operand's owned temporary slot; its destructor is trapped below Rust.
        leaf(unsafe { z::swr_zend_try_stdext_nogc_dtor(ex_var(frame, var)) })?;
    }
    Ok(())
}
unsafe fn result_slot(frame: *mut zend_execute_data, op: &zend_op) -> *mut zval {
    // SAFETY: the used result operand contains its valid frame-relative variable offset.
    ex_var(frame, unsafe { op.result.var })
}
fn result_used(op: &zend_op) -> bool {
    u32::from(op.result_type) != z::IS_UNUSED
}

/// The original failed dimension/assignment path frees the value before NULL publication.
unsafe fn abandon(
    frame: *mut zend_execute_data,
    op: &zend_op,
    data: &zend_op,
) -> Result<(), Bailout> {
    // SAFETY: these are this assignment's live frame and OP_DATA operand.
    unsafe { free_operand(frame, data) }?;
    if result_used(op) {
        // SAFETY: a used assignment result has a writable slot in this frame.
        let slot = unsafe { result_slot(frame, op) };
        // SAFETY: like ZVAL_NULL, overwrite only the result's type information.
        unsafe {
            (*slot).u1 = z::_zval_struct__bindgen_ty_1 {
                type_info: z::IS_NULL,
            }
        };
    }
    Ok(())
}

/// `get_op_data_zval_ptr_r`: Rust selects the actual operand; only an undefined
/// CV's PHP warning is native. References intentionally remain undereferenced.
unsafe fn operand(
    frame: *mut zend_execute_data,
    data_ptr: *const zend_op,
    data: &zend_op,
) -> Result<*mut zval, Bailout> {
    let kind = u32::from(data.op1_type);
    if kind & (z::IS_TMP_VAR | z::IS_VAR) != 0 {
        // SAFETY: a live variable/temporary node contains its frame-relative byte offset.
        return Ok(ex_var(frame, unsafe { data.op1.var }));
    }
    if kind == z::IS_CONST {
        // SAFETY: OP_DATA owns this constant node, whose relative offset uses OP_DATA itself.
        return Ok(unsafe { rt_constant(data_ptr, data.op1) });
    }
    if kind == z::IS_CV {
        // SAFETY: this CV node contains its frame-relative byte offset.
        let var = unsafe { data.op1.var };
        let value = ex_var(frame, var);
        // SAFETY: the compiler allocated this live CV slot in the current frame.
        if unsafe { type_of(value) } != z::IS_UNDEF {
            return Ok(value);
        }
        let mut undefined = ptr::null_mut();
        // SAFETY: Zend reads its own CV name and invokes any warning handler under a native trap.
        leaf(unsafe { swr_phpstdext_undefined_cv(frame, var, &raw mut undefined) })?;
        return Ok(undefined);
    }
    Ok(ptr::null_mut())
}

unsafe fn separate(container: *mut zval) -> Result<*mut z::HashTable, Bailout> {
    // SAFETY: the dispatcher established an initialized typed array zval.
    let source = unsafe { table(container) };
    // SAFETY: Zend separates this exact live array slot under a native allocation trap.
    leaf(unsafe { z::swr_zend_try_stdext_separate(container) })?;
    // SAFETY: successful separation leaves the container holding an initialized array.
    let current = unsafe { table(container) };
    if source != current {
        // SAFETY: source remains shared/immutable; current is the fresh unshared copy.
        unsafe { typed::copy_type(container, source) }?;
    }
    // SAFETY: copying type metadata may have replaced the header allocation.
    Ok(unsafe { table(container) })
}

unsafe fn strict(frame: *mut zend_execute_data) -> bool {
    // SAFETY: a live execution frame points at its active function.
    let function = unsafe { (*frame).func };
    // SAFETY: this active function has a live common function header.
    let function = unsafe { &*function };
    // SAFETY: every zend_function variant begins with this common header.
    let common = unsafe { function.common };
    common.fn_flags & z::ZEND_ACC_STRICT_TYPES != 0
}

unsafe fn assign(
    frame: *mut zend_execute_data,
    op: &zend_op,
    data: &zend_op,
    container: *mut zval,
    key: *const zval,
    value: *mut zval,
) -> Result<(), Bailout> {
    // SAFETY: the dispatcher supplied a live typed array slot.
    let ht = unsafe { separate(container) }?;
    let mut slot = ptr::null_mut();
    // SAFETY: this live key is the selected literal/CV/undefined value.
    if unsafe { type_of(key) } == z::IS_NULL {
        // SAFETY: the table takes the original operand reference on successful insertion.
        leaf(unsafe { z::swr_zend_try_stdext_append(ht, value, &raw mut slot) })?;
        if slot.is_null() {
            // SAFETY: preserve Zend's exact occupied-next-index error, below a native trap.
            leaf(unsafe { z::swr_zend_try_stdext_cannot_add() })?;
            // SAFETY: this assignment failed before publishing its result.
            return unsafe { abandon(frame, op, data) };
        }
    } else {
        let mut variable = ptr::null_mut();
        // SAFETY: lookup uses this assignment's original constant-key distinction.
        leaf(unsafe {
            swr_phpstdext_dimension(
                frame,
                ht,
                key,
                u32::from(op.op2_type) == z::IS_CONST,
                &raw mut variable,
            )
        })?;
        if variable.is_null() {
            // SAFETY: the native dimension lookup failed before assignment.
            return unsafe { abandon(frame, op, data) };
        }
        // SAFETY: read only the active function's strict-types flag.
        let strict = unsafe { strict(frame) };
        // SAFETY: Zend performs its original assignment/refcount effect below a trap.
        leaf(unsafe {
            swr_phpstdext_assign(variable, value, data.op1_type, strict, &raw mut slot)
        })?;
        if slot.is_null() {
            // SAFETY: the native assignment failed before result publication.
            return unsafe { abandon(frame, op, data) };
        }
    }
    if result_used(op) {
        // SAFETY: the used result slot is writable in this active frame.
        let result = unsafe { result_slot(frame, op) };
        // SAFETY: Zend's copy retains the inserted/assigned payload for the result.
        unsafe { z::swr_zval_copy(result, slot) };
    }
    if u32::from(data.op1_type) == z::IS_VAR {
        // SAFETY: retain the variable value before the original FREE_OP decrements it.
        unsafe { z::swr_zval_try_addref(value) };
    }
    // SAFETY: this is the assignment's original OP_DATA temporary/variable.
    unsafe { free_operand(frame, data) }
}

unsafe fn assign_op(
    frame: *mut zend_execute_data,
    op_ptr: *const zend_op,
    op: &zend_op,
    data: &zend_op,
    container: *mut zval,
    key: *const zval,
    value: *mut zval,
) -> Result<(), Bailout> {
    // SAFETY: this live typed array is separated before native dimension lookup.
    let ht = unsafe { separate(container) }?;
    let mut slot = ptr::null_mut();
    // Upstream selects W_CONST from the value's OP_DATA type here, unlike ASSIGN_DIM.
    // SAFETY: retain that exact distinction when invoking the native VM primitive.
    leaf(unsafe {
        swr_phpstdext_dimension(
            frame,
            ht,
            key,
            u32::from(data.op1_type) == z::IS_CONST,
            &raw mut slot,
        )
    })?;
    if slot.is_null() {
        // SAFETY: dimension lookup failed before the operation's result exists.
        return unsafe { abandon(frame, op, data) };
    }
    // SAFETY: successful lookup returns a live writable array-element slot.
    let reference = if unsafe { type_of(slot) } == z::IS_REFERENCE {
        // SAFETY: a reference zval contains an initialized reference pointer.
        let value = unsafe { (*slot).value };
        // SAFETY: the active variant of this union is ref_.
        unsafe { value.ref_ }
    } else {
        ptr::null_mut()
    };
    let mut typed_reference = false;
    if !reference.is_null() {
        slot = reference
            .wrapping_byte_add(offset_of!(zend_reference, val))
            .cast();
        // SAFETY: this live reference owns its PHP property-source metadata.
        let sources = unsafe { (*reference).sources };
        // SAFETY: Zend tests the source-list union through its pointer member too.
        typed_reference = !unsafe { sources.ptr }.is_null();
    }
    if typed_reference {
        // SAFETY: the native typed-reference operation preserves PHP source verification and strict types.
        leaf(unsafe { swr_phpstdext_typed_ref(frame, reference, value, op_ptr) })?;
    } else {
        // SAFETY: the live typed table is checked before the original binary operation.
        if !unsafe { typed::swr_v1_phpstdext_operation(ht, op.extended_value == z::ZEND_CONCAT) } {
            // SAFETY: a rejected operation follows the original temporary/result cleanup path.
            return unsafe { abandon(frame, op, data) };
        }
        // SAFETY: Zend applies the exact current opcode to the live element and value.
        leaf(unsafe { swr_phpstdext_binary(slot, value, op_ptr) })?;
    }
    if result_used(op) {
        // SAFETY: the active frame owns this used result slot.
        let result = unsafe { result_slot(frame, op) };
        // SAFETY: publish a retained copy of the actual post-operation element.
        unsafe { z::swr_zval_copy(result, slot) };
    }
    // SAFETY: release the original OP_DATA operand after result publication.
    unsafe { free_operand(frame, data) }
}

unsafe fn dispatch(frame: *mut zend_execute_data, operation: bool) -> Result<c_int, Bailout> {
    // SAFETY: Zend calls this user opcode handler at a live assignment instruction.
    let op_ptr = unsafe { (*frame).opline };
    // SAFETY: the active script owns this live plain C instruction record. Snapshot it
    // without borrowing its native storage across VM effects; the record has no destructor.
    let op = unsafe { op_ptr.read() };
    // SAFETY: the assignment's first operand contains its allocated frame offset.
    let array_var = unsafe { op.op1.var };
    // SAFETY: this original array operand is a live variable slot in the active frame.
    let array = unsafe { ptr_ptr_var(frame, array_var) };
    // SAFETY: the original handler dereferences this optional reference once.
    let array = unsafe { deref(array) };
    // SAFETY: the selected frame slot is initialized.
    if unsafe { type_of(array) } != z::IS_ARRAY {
        return Ok(DISPATCH);
    }
    // SAFETY: this initialized array slot owns a live hash table.
    let ht = unsafe { table(array) };
    // SAFETY: only marked typed arrays have the required trailing type information.
    if !unsafe { typed::is_typed(ht) } {
        return Ok(DISPATCH);
    }
    let data_ptr = op_ptr.wrapping_add(1);
    // SAFETY: each assignment has this live plain C OP_DATA record, with no destructor.
    // Copy the fields without borrowing native instruction storage across effects.
    let data = unsafe { data_ptr.read() };
    // SAFETY: select and read the current OP_DATA value, containing any CV warning.
    let value = unsafe { operand(frame, data_ptr, &data) }?;
    let key = match u32::from(op.op2_type) {
        // SAFETY: the original assignment instruction owns this constant key.
        z::IS_CONST => unsafe { rt_constant(op_ptr, op.op2) },
        // SAFETY: Zend supplies this thread's initialized undefined zval.
        z::IS_UNUSED => unsafe { z::swr_zend_uninitialized_zval() },
        // SAFETY: a non-constant key operand contains its frame-relative offset.
        _ => ex_var(frame, unsafe { op.op2.var }),
    };
    // SAFETY: the existing typed-array validator borrows these live key/value slots.
    if !unsafe { typed::swr_v1_phpstdext_check(ht, key, value) } {
        // SAFETY: rejection frees OP_DATA and intentionally does not advance the opline.
        unsafe { free_operand(frame, &data) }?;
        return Ok(CONTINUE);
    }
    if operation {
        // SAFETY: the typed operands and original instruction remain live throughout this operation.
        unsafe { assign_op(frame, op_ptr, &op, &data, array, key, value) }?;
    } else {
        // SAFETY: the typed operands and original instruction remain live throughout this assignment.
        unsafe { assign(frame, &op, &data, array, key, value) }?;
    }
    // SAFETY: reload the current frame opline after reentry, as EX(opline) += 2 did.
    let current = unsafe { (*frame).opline };
    // SAFETY: Zend's active assignment has a following instruction after OP_DATA.
    unsafe { (*frame).opline = current.wrapping_add(2) };
    Ok(CONTINUE)
}

/// `ASSIGN_DIM`'s complete typed-array opcode handler.
/// # Safety
/// Zend supplies a live frame at its assignment instruction.
pub unsafe extern "C" fn swr_v1_phpstdext_assign(frame: *mut zend_execute_data) -> c_int {
    // SAFETY: this native entry follows Zend's user opcode callback contract.
    finish(unsafe { dispatch(frame, false) })
}
/// `ASSIGN_DIM_OP`'s complete typed-array opcode handler.
/// # Safety
/// Zend supplies a live frame at its assignment operation instruction.
pub unsafe extern "C" fn swr_v1_phpstdext_assign_op(frame: *mut zend_execute_data) -> c_int {
    // SAFETY: this native entry follows Zend's user opcode callback contract.
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
