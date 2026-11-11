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

/// An assignment's `OP_DATA` instruction with the value it read, whose operand
/// this still owns: [`Data::free`] (or `abandon`, or `publish`) consumes it to
/// release the operand, once, and nothing reads the value after that.
struct Data<'a> {
    op: Op<'a>,
    value: &'a Value,
}

impl<'a> Data<'a> {
    /// `op`'s `OP_DATA` and its value (`get_op_data_zval_ptr_r`), or None for
    /// an unused operand.
    /// # Safety
    /// `op` is an `ASSIGN_DIM` or `ASSIGN_DIM_OP`, which `OP_DATA` follows, and
    /// nothing else frees that operand.
    unsafe fn of(op: Op<'a>) -> Result<Option<Self>, Bailout> {
        // SAFETY: per the caller.
        let op = unsafe { op.data() };
        Ok(operand(op)?.map(|value| Self { op, value }))
    }

    /// The operand's value, until it is freed.
    fn value(&self) -> &Value {
        self.value
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
unsafe fn separate(container: &Value, source: Typed) -> *mut z::HashTable {
    // SAFETY: Zend separates this exact live array slot.
    unsafe { z::swr_zend_separate_array(container.as_ptr()) };
    // SAFETY: successful separation leaves the container holding an initialized array.
    let current = unsafe { table(container.as_ptr()) };
    if source.table() != current {
        // SAFETY: source remains shared/immutable; current is the fresh unshared copy.
        unsafe { typed::copy_type(container.as_ptr(), source) };
    }
    // SAFETY: copying type metadata may have replaced the header allocation.
    unsafe { table(container.as_ptr()) }
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
/// `container` holds `source`.
unsafe fn assign(
    op: Op,
    data: Data,
    container: &Value,
    source: Typed,
    key: &Value,
) -> Result<(), Bailout> {
    let value = data.value();
    // SAFETY: per the caller.
    let ht = unsafe { separate(container, source) };
    let mut slot = ptr::null_mut();
    if key.kind() == z::IS_NULL {
        // SAFETY: the table takes the original operand reference on successful insertion.
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

/// # Safety
/// As [`assign`].
unsafe fn assign_op(
    op: Op,
    data: Data,
    container: &Value,
    source: Typed,
    key: &Value,
) -> Result<(), Bailout> {
    let value = data.value();
    // SAFETY: per the caller.
    let ht = unsafe { separate(container, source) };
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
    // SAFETY: the container holds its own typed copy now.
    let typed = unsafe { Typed::from_table(ht) };
    if !typed::operation(typed, op.extended_value == z::ZEND_CONCAT) {
        // A rejected operation follows the original temporary/result cleanup path.
        return data.abandon(op);
    }
    // SAFETY: Zend applies the exact current opcode to the live element and value.
    leaf(unsafe { swr_phpstdext_binary(target, value.as_ptr(), op.at) })?;
    // SAFETY: the element's live slot, or its reference's.
    unsafe { data.publish(op, target) }
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
    // SAFETY: per the caller, an assignment, which OP_DATA follows; only
    // `data` frees its operand.
    let Some(data) = (unsafe { Data::of(op) })? else {
        return Ok(DISPATCH);
    };
    if !typed::check(typed, KeyType::of(key), data.value()) {
        // Rejection frees OP_DATA and intentionally does not advance the opline.
        data.free()?;
        return Ok(CONTINUE);
    }
    if operation {
        // SAFETY: the typed operands and original instruction remain live throughout this operation.
        unsafe { assign_op(op, data, array, typed, key) }?;
    } else {
        // SAFETY: the typed operands and original instruction remain live throughout this assignment.
        unsafe { assign(op, data, array, typed, key) }?;
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
