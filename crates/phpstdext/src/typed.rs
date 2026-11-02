//! Typed arrays: a `HashTable` allocated with its type definition behind it
//! and flagged [`TYPED`]. Writes check keys and values against the definition:
//! `swoole_typed_array`, the `array_push`/`array_unshift`/`array_splice`
//! overrides, and the `ASSIGN_DIM`/`UNSET_DIM`/`FE_RESET_RW` opcode handlers.
use crate::{
    CONTINUE, DISPATCH, deref, ex_var, invoke::compiled_function, ptr_ptr_var, raise, rt_constant,
};
use std::{
    ffi::{CStr, c_char, c_int, c_long, c_void},
    mem::offset_of,
    ptr, slice,
    sync::OnceLock,
};
use swoole_rs_zend::{
    Call,
    bailout::Bailout,
    handlers, string,
    sys::{
        _zend_hash_init, _zval_struct__bindgen_ty_1, _zval_struct__bindgen_ty_2, HashTable,
        IS_ARRAY, IS_ARRAY_EX, IS_CONST, IS_CV, IS_DOUBLE, IS_FALSE, IS_LONG, IS_NULL, IS_OBJECT,
        IS_RESOURCE, IS_STRING, IS_TRUE, IS_UNDEF, IS_VAR, swr_zend_efree, swr_zend_emalloc,
        swr_zend_hash_foreach, swr_zend_hash_foreach_key, swr_zend_instanceof,
        swr_zend_string_release, swr_zend_try_lookup_class, swr_zend_uninitialized_zval,
        swr_zval_copy, swr_zval_try_addref, zend_array_count, zend_class_entry, zend_execute_data,
        zend_function, zend_get_type_by_const, zend_hash_add, zend_hash_index_add,
        zend_internal_function, zend_object, zend_string, zend_throw_error, zend_type_error,
        zend_ulong, zend_value, zif_handler, zval, zval_ptr_dtor,
    },
    type_of,
    value::{long_of, obj_of, set_long, set_str, table},
};

/// `HASH_FLAG_TYPED_ARRAY`, a `HashTable` flag bit Zend leaves unused.
const TYPED: u32 = 1 << 12;

/// A Zend type code as upstream stores it, in a byte.
#[expect(
    clippy::cast_possible_truncation,
    reason = "Zend type codes are below 256"
)]
const fn code(kind: u32) -> u8 {
    kind as u8
}

/// Upstream's `ArrayTypeValue`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct TypeValue {
    value: u8,
    key: u8,
    offset: u16,
    len: u16,
}

/// Upstream's `ArrayTypeInfo`, followed by `len` bytes of definition and a NUL.
#[repr(C)]
struct TypeInfo {
    this: TypeValue,
    element: TypeValue,
    ce: *mut zend_class_entry,
    len: u16,
    text: [u8; 0],
}

/// Why a definition did not parse.
enum Failure {
    /// Zend has thrown.
    Thrown,
    Bailout(Bailout),
}

/// `HT_FLAGS(ht)`.
fn flags_of(ht: *mut HashTable) -> *mut u32 {
    ht.wrapping_byte_add(offset_of!(HashTable, u)).cast()
}

/// `HT_FLAGS(ht) & HASH_FLAG_TYPED_ARRAY`.
/// # Safety
/// `ht` is live.
#[inline]
unsafe fn is_typed(ht: *mut HashTable) -> bool {
    // SAFETY: per the caller.
    (unsafe { *flags_of(ht) } & TYPED) != 0
}

/// # Safety
/// `ht` is live.
unsafe fn set_typed(ht: *mut HashTable) {
    // SAFETY: per the caller.
    unsafe { *flags_of(ht) |= TYPED };
}

/// `get_type_info`: the definition behind a typed array's table.
fn info_of(ht: *mut HashTable) -> *mut TypeInfo {
    ht.wrapping_byte_add(size_of::<HashTable>()).cast()
}

/// The definition's text.
fn text_of(info: *mut TypeInfo) -> *mut u8 {
    info.wrapping_byte_add(offset_of!(TypeInfo, text)).cast()
}

/// Bytes a `TypeInfo` with `len` bytes of text takes.
fn info_size(len: usize) -> usize {
    size_of::<TypeInfo>() + len + 1
}

/// # Safety
/// `info` is a parsed definition.
unsafe fn text<'a>(info: *mut TypeInfo) -> &'a [u8] {
    // SAFETY: per the caller.
    let len = usize::from(unsafe { (*info).len });
    // SAFETY: per the caller, `len` bytes of text.
    unsafe { slice::from_raw_parts(text_of(info), len) }
}

/// `get_value_type_str()` with its length.
/// # Safety
/// `info` is a parsed definition.
unsafe fn value_bytes<'a>(info: *mut TypeInfo) -> &'a [u8] {
    // SAFETY: per the caller.
    let this = unsafe { (*info).this };
    let start = text_of(info).wrapping_add(usize::from(this.offset));
    // SAFETY: the value part lies within the text.
    unsafe { slice::from_raw_parts(start, usize::from(this.len)) }
}

/// `ArrayTypeInfo::equals`.
/// # Safety
/// Both are parsed definitions.
unsafe fn equals(a: *mut TypeInfo, b: *mut TypeInfo) -> bool {
    // SAFETY: per the caller.
    let x = unsafe { (*a).this };
    // SAFETY: per the caller.
    let y = unsafe { (*b).this };
    // SAFETY: per the caller.
    // SAFETY: per the caller.
    let left = unsafe { text(a) };
    // SAFETY: per the caller.
    let same = left == unsafe { text(b) };
    x.key == y.key && x.value == y.value && same
}

/// `ArrayTypeInfo::element_type_equals`: whether `element` is the type of
/// `info`'s values.
/// # Safety
/// Both are parsed definitions.
unsafe fn element_equals(info: *mut TypeInfo, element: *mut TypeInfo) -> bool {
    // SAFETY: per the caller.
    let expected = unsafe { (*info).element };
    // SAFETY: per the caller.
    let actual = unsafe { (*element).this };
    // SAFETY: per the caller.
    // SAFETY: per the caller.
    let actual_text = unsafe { text(element) };
    // SAFETY: per the caller.
    let same = actual_text == unsafe { value_bytes(info) };
    actual.key == expected.key && actual.value == expected.value && same
}

unsafe extern "C" {
    /// libc's `isspace`, as upstream's `remove_all_spaces` classifies bytes.
    safe fn isspace(c: c_int) -> c_int;
}

/// `get_type`.
fn type_of_name(name: &[u8]) -> u8 {
    const NAMES: [(&[u8], u32); 4] = [
        (b"int", IS_LONG),
        (b"float", IS_DOUBLE),
        (b"string", IS_STRING),
        (b"bool", IS_TRUE),
    ];
    let named = |text: &[u8]| name.eq_ignore_ascii_case(text);
    let kind = if let Some((_, kind)) = NAMES.iter().find(|(text, _)| named(text)) {
        *kind
    } else if name.first() == Some(&b'<') && name.last() == Some(&b'>') {
        IS_ARRAY
    } else if named(b"resource") {
        IS_RESOURCE
    } else if named(b"null") {
        IS_NULL
    } else {
        IS_OBJECT
    };
    code(kind)
}

/// `ArrayTypeValue::parse` of the `len` bytes at `start`, whose `strchr`
/// runs to the NUL ending the whole definition.
/// # Safety
/// `start` begins `len` bytes of a NUL-terminated definition.
unsafe fn parse_value(target: &mut TypeValue, start: *const u8, len: usize) -> bool {
    // SAFETY: per the caller.
    match unsafe { split_value(start, len) } {
        Ok(value) => {
            *target = value;
            true
        }
        Err(rest) => {
            // SAFETY: a printf format with the NUL-terminated rest.
            unsafe {
                zend_throw_error(
                    ptr::null_mut(),
                    c"The key type of array must be string or int, but got %s".as_ptr(),
                    rest,
                );
            }
            false
        }
    }
}

/// [`parse_value`]'s split: the value, or where the text after a key that is
/// not `string` or `int` begins.
/// # Safety
/// As [`parse_value`].
unsafe fn split_value(start: *const u8, len: usize) -> Result<TypeValue, *const u8> {
    // SAFETY: per the caller, NUL-terminated.
    let rest = unsafe { CStr::from_ptr(start.cast()) }.to_bytes();
    // SAFETY: per the caller, within the definition.
    let bytes = |from: usize, count: usize| unsafe {
        slice::from_raw_parts(start.wrapping_add(from), count)
    };
    let mut target = TypeValue::default();
    match rest.iter().position(|&byte| byte == b',') {
        None => target.offset = 1,
        Some(comma) => {
            target.key = type_of_name(bytes(1, comma - 1));
            if !matches!(u32::from(target.key), IS_STRING | IS_LONG) {
                return Err(start.wrapping_add(comma + 1));
            }
            // Definitions are shorter than 65535 bytes.
            target.offset = u16::try_from(comma + 1).unwrap_or(u16::MAX);
        }
    }
    let len = u16::try_from(len).unwrap_or(u16::MAX);
    target.len = len.wrapping_sub(target.offset).wrapping_sub(1);
    target.value = type_of_name(bytes(usize::from(target.offset), usize::from(target.len)));
    Ok(target)
}

/// `ArrayTypeInfo::parse` into `info`, which has room for the definition.
/// # Safety
/// `info` has `info_size(len)` bytes; `definition` is live.
unsafe fn parse(info: *mut TypeInfo, definition: *mut zend_string) -> Result<(), Failure> {
    // SAFETY: a live string.
    let source = unsafe { string::bytes(definition) };
    let Ok(source_len) = u16::try_from(source.len()) else {
        throw(c"The type definition string is too long (must be less than 65535 characters)");
        return Err(Failure::Thrown);
    };
    if source_len == u16::MAX {
        throw(c"The type definition string is too long (must be less than 65535 characters)");
        return Err(Failure::Thrown);
    }
    let text = text_of(info);
    // `zend_string_tolower`, then `remove_all_spaces`.
    let mut len = 0;
    for &byte in source {
        let lower = byte.to_ascii_lowercase();
        if isspace(c_int::from(lower)) == 0 {
            // SAFETY: at most `source.len()` bytes.
            unsafe { text.wrapping_add(len).write(lower) };
            len += 1;
        }
    }
    // SAFETY: room for the NUL.
    unsafe { text.wrapping_add(len).write(0) };
    // SAFETY: room for the header; at most `source_len`.
    unsafe {
        info.write(TypeInfo {
            this: TypeValue::default(),
            element: TypeValue::default(),
            ce: ptr::null_mut(),
            len: u16::try_from(len).unwrap_or(source_len),
            text: [],
        });
    }
    // SAFETY: the bytes just written.
    let written = unsafe { slice::from_raw_parts(text, len) };
    if written.first() != Some(&b'<') || written.last() != Some(&b'>') {
        throw(c"The type definition of typed array must start with '<' and end with '>'");
        return Err(Failure::Thrown);
    }
    let mut this = TypeValue::default();
    // SAFETY: a NUL-terminated definition of `len` bytes.
    if !unsafe { parse_value(&mut this, text, len) } {
        return Err(Failure::Thrown);
    }
    // SAFETY: the header written above.
    unsafe { (*info).this = this };
    if u32::from(this.value) == IS_OBJECT {
        // SAFETY: parsed above.
        let name = string::copy(unsafe { value_bytes(info) });
        let ce = lookup_class(name);
        // SAFETY: the reference made above.
        unsafe { swr_zend_string_release(name) };
        let ce = ce?;
        // SAFETY: the header written above.
        unsafe { (*info).ce = ce };
    }
    Ok(())
}

/// `zend_lookup_class(name)`, throwing "Class '%s' not found".
fn lookup_class(name: *mut zend_string) -> Result<*mut zend_class_entry, Failure> {
    let mut ce = ptr::null_mut();
    // SAFETY: a live name; autoloaders run under `zend_try`.
    if unsafe { swr_zend_try_lookup_class(name, &raw mut ce) } != 0 {
        return Err(Failure::Bailout(Bailout));
    }
    if ce.is_null() {
        // SAFETY: a printf format with the NUL-terminated name.
        unsafe {
            zend_throw_error(
                ptr::null_mut(),
                c"Class '%s' not found".as_ptr(),
                string::val(name),
            );
        }
        return Err(Failure::Thrown);
    }
    Ok(ce)
}

fn throw(message: &CStr) {
    // SAFETY: a literal message without directives.
    unsafe { zend_throw_error(ptr::null_mut(), c"%s".as_ptr(), message.as_ptr()) };
}

fn type_name(kind: u32) -> *const c_char {
    // SAFETY: Zend's name table (NULL for an unnamed type, which printf shows).
    unsafe { zend_get_type_by_const(kind.cast_signed()) }
}

/// `get_array_type_def`: `<string,value>`, `<int,value>` or, for a list, `value>`.
/// # Safety
/// `info` is a parsed definition.
unsafe fn definition_of(info: *mut TypeInfo) -> *mut zend_string {
    // SAFETY: per the caller.
    let key = u32::from(unsafe { (*info).this.key });
    let prefix: &[u8] = match key {
        IS_STRING => b"<string,",
        IS_LONG => b"<int,",
        // Upstream writes the `<` but only steps past it for a keyed type.
        _ => b"",
    };
    // SAFETY: per the caller.
    let value = unsafe { value_bytes(info) };
    let result = string::alloc(value.len() + 16);
    let start = string::val(result);
    // SAFETY: at most 8 + `value.len()` + 2 bytes of the allocation.
    unsafe { ptr::copy_nonoverlapping(prefix.as_ptr(), start, prefix.len()) };
    let tail = start.wrapping_add(prefix.len());
    // SAFETY: as above.
    unsafe { ptr::copy_nonoverlapping(value.as_ptr(), tail, value.len()) };
    // SAFETY: as above.
    unsafe { tail.wrapping_add(value.len()).write(b'>') };
    // SAFETY: as above; writes the NUL.
    unsafe { string::truncate(result, prefix.len() + value.len() + 1) };
    result
}

/// `ArrayTypeInfo::check`: whether `key => value` may go in the typed array
/// `ht`; if not, Zend has thrown.
/// # Safety
/// `ht` is a live typed array; `key` and `value` are live zvals.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpstdext_check(
    ht: *mut HashTable,
    key: *const zval,
    value: *const zval,
) -> bool {
    let info = info_of(ht);
    // SAFETY: per the caller.
    let this = unsafe { (*info).this };
    // SAFETY: per the caller.
    if !unsafe { check_key(ht, this, key) } {
        return false;
    }
    // SAFETY: per the caller.
    let value = unsafe { deref(value.cast_mut()) };
    // SAFETY: as above.
    let value_type = unsafe { type_of(value) };
    let expected = u32::from(this.value);
    let bool_type = |kind| kind == IS_TRUE || kind == IS_FALSE;
    if bool_type(expected) && bool_type(value_type) {
        return true;
    }
    if value_type != expected {
        // SAFETY: printf arguments: two strings.
        unsafe {
            zend_type_error(
                c"Array value type mismatch, expected `%s`, got `%s`".as_ptr(),
                type_name(expected),
                type_name(value_type),
            );
        }
        return false;
    }
    match expected {
        IS_OBJECT => {
            // SAFETY: an object value.
            let object = unsafe { obj_of(value) };
            // SAFETY: a parsed object definition.
            unsafe { check_object(info, object) }
        }
        IS_ARRAY => {
            // SAFETY: an array value.
            let element = unsafe { table(value) };
            // SAFETY: a parsed array definition.
            unsafe { check_array(info, element) }
        }
        _ => true,
    }
}

/// The key half of [`swr_v1_phpstdext_check`].
/// # Safety
/// As there.
unsafe fn check_key(ht: *mut HashTable, this: TypeValue, key: *const zval) -> bool {
    // SAFETY: per the caller.
    let key_type = unsafe { type_of(key) };
    if this.key > 0 {
        if key_type == u32::from(this.key) {
            return true;
        }
        // SAFETY: printf arguments: two strings.
        unsafe {
            zend_type_error(
                c"Array key type mismatch, expected `%s`, got `%s`".as_ptr(),
                type_name(this.key.into()),
                type_name(key_type),
            );
        }
        return false;
    }
    if key_type == IS_LONG {
        // SAFETY: an int key.
        let index = unsafe { long_of(key) };
        // SAFETY: a live table.
        if index > i64::from(unsafe { (*ht).nNumOfElements }) {
            // SAFETY: printf arguments: a long.
            unsafe {
                zend_throw_error(
                    ptr::null_mut(),
                    c"Incorrect array key `%ld`, out of the permitted range".as_ptr(),
                    index as c_long,
                );
            }
            return false;
        }
    } else if !matches!(key_type, IS_UNDEF | IS_NULL) {
        throw(c"Incorrect array key, must be undef or int");
        return false;
    }
    true
}

/// # Safety
/// `info` is a parsed object definition; `object` is live.
unsafe fn check_object(info: *mut TypeInfo, object: *mut zend_object) -> bool {
    // SAFETY: per the caller.
    let actual = unsafe { (*object).ce };
    // SAFETY: per the caller.
    let ce = unsafe { (*info).ce };
    // SAFETY: live classes.
    if unsafe { swr_zend_instanceof(actual, ce) } {
        return true;
    }
    // SAFETY: live classes.
    let expected_name = unsafe { (*ce).name };
    // SAFETY: as above.
    let actual_name = unsafe { (*actual).name };
    // SAFETY: printf arguments: two class names.
    unsafe {
        zend_type_error(
            c"Array value type mismatch, expected `%s`, got `%s`".as_ptr(),
            string::val(expected_name),
            string::val(actual_name),
        );
    }
    false
}

/// # Safety
/// `info` is a parsed array-of-arrays definition; `element` is live.
unsafe fn check_array(info: *mut TypeInfo, element: *mut HashTable) -> bool {
    // SAFETY: per the caller.
    let expected = unsafe { value_bytes(info) };
    // Shorter than 65535 bytes.
    let expected_len = c_int::try_from(expected.len()).unwrap_or(c_int::MAX);
    // SAFETY: a live table.
    if !unsafe { is_typed(element) } {
        // SAFETY: printf arguments: a length and its bytes.
        unsafe {
            zend_type_error(
                c"Array value type mismatch, expected `%.*s`, got `array`".as_ptr(),
                expected_len,
                expected.as_ptr(),
            );
        }
        return false;
    }
    // SAFETY: both typed.
    if unsafe { element_equals(info, info_of(element)) } {
        return true;
    }
    // SAFETY: a typed array's definition.
    let actual = unsafe { definition_of(info_of(element)) };
    // SAFETY: the string just made.
    let actual_bytes = unsafe { string::bytes(actual) };
    // SAFETY: printf arguments: two lengths and their bytes.
    unsafe {
        zend_type_error(
            c"Array value type mismatch, expected `%.*s`, got `%.*s`".as_ptr(),
            expected_len,
            expected.as_ptr(),
            c_int::try_from(actual_bytes.len()).unwrap_or(c_int::MAX),
            actual_bytes.as_ptr(),
        );
    }
    // SAFETY: the string made above.
    unsafe { swr_zend_string_release(actual) };
    false
}

/// `copy_array_type_info`: after `SEPARATE_ARRAY(container)` made a copy of
/// the typed array `source`, give the copy `source`'s definition.
/// # Safety
/// `container` holds the fresh unshared copy; `source` is a live typed array.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpstdext_copy_type(container: *mut zval, source: *mut HashTable) {
    let source_info = info_of(source);
    // SAFETY: per the caller.
    let extra = info_size(usize::from(unsafe { (*source_info).len }));
    // SAFETY: an array zval.
    let ht = unsafe { table(container) };
    // SAFETY: Zend's request allocator.
    let copy = unsafe { swr_zend_emalloc(size_of::<HashTable>() + extra) }.cast::<HashTable>();
    // SAFETY: both hold a table.
    unsafe { ptr::copy_nonoverlapping(ht, copy, 1) };
    // SAFETY: the source holds `extra` bytes of definition; the copy has room.
    unsafe { ptr::copy_nonoverlapping(source_info.cast::<u8>(), info_of(copy).cast(), extra) };
    // SAFETY: the table just made.
    unsafe { set_typed(copy) };
    // SAFETY: an array zval, which now holds the copy.
    unsafe { (*container).value = zend_value { arr: copy } };
    // SAFETY: the old allocation, which nothing references now.
    unsafe { swr_zend_efree(ht.cast()) };
}

/// The `ASSIGN_DIM_OP` restriction: concatenation needs string values,
/// arithmetic int or float values. If not, Zend has thrown.
/// # Safety
/// `ht` is a live typed array.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpstdext_operation(ht: *mut HashTable, concat: bool) -> bool {
    // SAFETY: per the caller.
    let value = u32::from(unsafe { (*info_of(ht)).this.value });
    let (allowed, message) = if concat {
        (value == IS_STRING, c"Only string support concat operation")
    } else {
        (
            value == IS_LONG || value == IS_DOUBLE,
            c"Only int or float support arithmetic operation",
        )
    };
    if !allowed {
        // SAFETY: a literal message without directives.
        unsafe { zend_type_error(c"%s".as_ptr(), message.as_ptr()) };
    }
    allowed
}

/// The `UNSET_DIM` user opcode handler: a typed list keeps its keys.
/// # Safety
/// Zend's user opcode handler contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpstdext_unset(frame: *mut zend_execute_data) -> c_int {
    // SAFETY: a live frame.
    let opline = unsafe { (*frame).opline };
    // SAFETY: the frame's current opline.
    let op1 = unsafe { (*opline).op1 };
    // SAFETY: a variable operand holds its slot.
    let var = unsafe { op1.var };
    // SAFETY: `get_array_on_opline`: op1's slot.
    let slot = unsafe { ptr_ptr_var(frame, var) };
    // SAFETY: a live zval.
    let array = unsafe { deref(slot) };
    // SAFETY: as above.
    if unsafe { type_of(array) } != IS_ARRAY {
        return DISPATCH;
    }
    // SAFETY: an array zval.
    let ht = unsafe { table(array) };
    // SAFETY: a live table.
    if !unsafe { is_typed(ht) } {
        return DISPATCH;
    }
    // SAFETY: a typed array's definition.
    if unsafe { (*info_of(ht)).this.key } == 0 {
        throw(c"The typed array list do not support random deletion of elements");
        return CONTINUE;
    }
    DISPATCH
}

/// The `FE_RESET_RW` user opcode handler: no `foreach` by reference over a
/// typed array.
/// # Safety
/// Zend's user opcode handler contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpstdext_foreach_begin(frame: *mut zend_execute_data) -> c_int {
    // SAFETY: a live frame.
    let opline = unsafe { (*frame).opline };
    // SAFETY: the frame's current opline.
    let op = unsafe { &*opline };
    // SAFETY: an operand holds its slot or literal.
    let var = unsafe { op.op1.var };
    // SAFETY: as above.
    let result = unsafe { op.result.var };
    let operand = match u32::from(op.op1_type) {
        // SAFETY: a variable operand's slot.
        IS_VAR | IS_CV => unsafe { ptr_ptr_var(frame, var) },
        // SAFETY: a constant operand.
        IS_CONST => unsafe { rt_constant(opline, op.op1) },
        _ => ex_var(frame, var),
    };
    // SAFETY: the operand's live zval.
    let array = unsafe { deref(operand) };
    // SAFETY: as above.
    if unsafe { type_of(array) } != IS_ARRAY {
        return DISPATCH;
    }
    // SAFETY: an array zval.
    let ht = unsafe { table(array) };
    // SAFETY: a live table.
    if !unsafe { is_typed(ht) } {
        return DISPATCH;
    }
    throw(c"The type array do not support using references for element value during iteration");
    let result = ex_var(frame, result);
    // SAFETY: the result slot: `ZVAL_UNDEF`.
    unsafe {
        (*result).u1 = _zval_struct__bindgen_ty_1 {
            type_info: IS_UNDEF,
        }
    };
    // SAFETY: as above: `Z_FE_ITER_P(result) = -1`.
    unsafe {
        (*result).u2 = _zval_struct__bindgen_ty_2 {
            fe_iter_idx: u32::MAX,
        }
    };
    CONTINUE
}

/// `make_typed_array`: a table with room for `len` bytes of definition.
fn make_typed_array(size: u32, len: usize) -> *mut HashTable {
    // SAFETY: Zend's request allocator.
    let ht = unsafe { swr_zend_emalloc(size_of::<HashTable>() + info_size(len)) }.cast();
    // SAFETY: room for a table.
    unsafe { _zend_hash_init(ht, size, Some(zval_ptr_dtor), false) };
    // SAFETY: the table just made.
    unsafe { set_typed(ht) };
    ht
}

/// `emalloc` room for a definition parsed from `definition`.
/// # Safety
/// `definition` is live.
unsafe fn scratch(definition: *mut zend_string) -> *mut TypeInfo {
    // SAFETY: per the caller.
    let len = unsafe { (*definition).len };
    // SAFETY: Zend's request allocator.
    unsafe { swr_zend_emalloc(info_size(len)) }.cast()
}

handlers! {
    zif_swoole_typed_array => typed_array;
    zif_swoole_array_is_typed => array_is_typed;
}

/// `swoole_typed_array(string $typeDef, ?array $initArray = null)`.
fn typed_array(call: &Call) {
    let parsed = call.arity(1, 2).and_then(|()| {
        let definition = call.str(1)?;
        let init = if call.num_args() >= 2 {
            Some(call.array(2)?)
        } else {
            None
        };
        Ok((definition, init))
    });
    let Ok((Some(definition), init)) = parsed else {
        return;
    };
    // SAFETY: a live argument.
    let scratch = unsafe { scratch(definition) };
    // SAFETY: room for the definition.
    let outcome = unsafe { parse(scratch, definition) }.and_then(|()| match init {
        // SAFETY: a parsed definition and a live array argument.
        Some(init) => unsafe { from_array(call, scratch, init) },
        // SAFETY: a parsed definition.
        None => unsafe { fill(call, scratch, None) },
    });
    // SAFETY: allocated above; `fill` copied what it needed.
    unsafe { swr_zend_efree(scratch.cast()) };
    if let Err(failure) = outcome {
        call.set_null();
        if let Failure::Bailout(bailout) = failure {
            raise(&Err(bailout));
        }
    }
}

/// `swoole_typed_array` with initial values: a typed array of the same
/// definition is returned as is.
/// # Safety
/// `scratch` is parsed; `init` is a live array.
unsafe fn from_array(call: &Call, scratch: *mut TypeInfo, init: *mut zval) -> Result<(), Failure> {
    // SAFETY: per the caller.
    let ht = unsafe { table(init) };
    // SAFETY: a live table.
    if !unsafe { is_typed(ht) } {
        // SAFETY: per the caller.
        return unsafe { fill(call, scratch, Some(ht)) };
    }
    // SAFETY: both parsed definitions.
    if unsafe { equals(scratch, info_of(ht)) } {
        // SAFETY: the return slot takes a new reference.
        unsafe { swr_zval_copy(call.return_value(), init) };
    } else {
        throw(c"The type definition of the typed array does not match the initial values");
    }
    Ok(())
}

/// A new typed array of `scratch`'s definition, holding `init`'s entries.
/// # Safety
/// `scratch` is parsed; `init` is live.
unsafe fn fill(
    call: &Call,
    scratch: *mut TypeInfo,
    init: Option<*mut HashTable>,
) -> Result<(), Failure> {
    // SAFETY: a live table.
    let size = init.map_or(0, |init| unsafe { zend_array_count(init) });
    // SAFETY: parsed.
    let len = usize::from(unsafe { (*scratch).len });
    let array = make_typed_array(size, len);
    let value = call.return_value();
    // SAFETY: `ZVAL_ARR(return_value, array)`.
    unsafe { (*value).value = zend_value { arr: array } };
    // SAFETY: as above.
    unsafe {
        (*value).u1 = _zval_struct__bindgen_ty_1 {
            type_info: IS_ARRAY_EX,
        }
    };
    let info = info_of(array);
    // SAFETY: both hold `info_size(len)` bytes.
    unsafe { ptr::copy_nonoverlapping(scratch.cast::<u8>(), info.cast(), info_size(len)) };
    // SAFETY: a parsed definition.
    let this = unsafe { (*info).this };
    if u32::from(this.value) == IS_ARRAY {
        let mut element = TypeValue::default();
        let start = text_of(info).wrapping_add(usize::from(this.offset));
        // SAFETY: the value part of the NUL-terminated definition.
        if !unsafe { parse_value(&mut element, start, usize::from(this.len)) } {
            // SAFETY: the array made above.
            unsafe { zval_ptr_dtor(value) };
            return Err(Failure::Thrown);
        }
        // SAFETY: as above.
        unsafe { (*info).element = element };
    }
    let Some(init) = init else {
        return Ok(());
    };
    let mut fill = Fill { array, ok: true };
    // SAFETY: a live table; `fill` outlives the walk.
    unsafe { swr_zend_hash_foreach_key(init, Some(add), (&raw mut fill).cast()) };
    if !fill.ok {
        // SAFETY: the array made above.
        unsafe { zval_ptr_dtor(value) };
        return Err(Failure::Thrown);
    }
    Ok(())
}

struct Fill {
    array: *mut HashTable,
    ok: bool,
}

/// One `ZEND_HASH_FOREACH_KEY_VAL` step of `swoole_typed_array`'s fill.
unsafe extern "C" fn add(
    index: zend_ulong,
    key: *mut zend_string,
    value: *mut zval,
    context: *mut c_void,
) -> bool {
    // SAFETY: `fill`'s `Fill`.
    let fill = unsafe { &mut *context.cast::<Fill>() };
    // SAFETY: an all-zero zval is IS_UNDEF.
    let mut zkey: zval = unsafe { std::mem::zeroed() };
    if key.is_null() {
        // SAFETY: a local zval.
        unsafe { set_long(&raw mut zkey, index.cast_signed()) };
    } else {
        // SAFETY: a borrowed key; only its type is read.
        unsafe { set_str(&raw mut zkey, key) };
    }
    // SAFETY: a live typed array and zvals.
    if !unsafe { swr_v1_phpstdext_check(fill.array, &raw const zkey, value) } {
        fill.ok = false;
        return false;
    }
    // SAFETY: a live value; the array takes the new reference.
    unsafe { swr_zval_try_addref(value) };
    if key.is_null() {
        // SAFETY: a live table.
        unsafe { zend_hash_index_add(fill.array, index, value) };
    } else {
        // SAFETY: as above.
        unsafe { zend_hash_add(fill.array, key, value) };
    }
    true
}

/// `swoole_array_is_typed(array $array, string $typeDef = '')`.
fn array_is_typed(call: &Call) {
    let parsed = call
        .arity(1, 2)
        .and_then(|()| Ok((call.array(1)?, call.str(2)?)));
    let Ok((array, definition)) = parsed else {
        return;
    };
    // SAFETY: a live array argument.
    let ht = unsafe { table(array) };
    // SAFETY: as above.
    if !unsafe { is_typed(ht) } {
        call.set_bool(false);
        return;
    }
    let Some(definition) = definition else {
        call.set_bool(true);
        return;
    };
    // SAFETY: a live argument.
    let scratch = unsafe { scratch(definition) };
    // SAFETY: room for the definition.
    let parsed = unsafe { parse(scratch, definition) };
    // SAFETY: both parsed definitions, when it parsed.
    call.set_bool(parsed.is_ok() && unsafe { equals(info_of(ht), scratch) });
    // SAFETY: allocated above.
    unsafe { swr_zend_efree(scratch.cast()) };
    if let Err(Failure::Bailout(bailout)) = parsed {
        raise(&Err(bailout));
    }
}

/// The handlers the overrides wrap.
struct Originals {
    push: zif_handler,
    unshift: zif_handler,
    splice: zif_handler,
}

static ORIGINALS: OnceLock<Originals> = OnceLock::new();

/// Swap `function`'s handler for `handler`: the original.
/// # Safety
/// `function` is a core internal function; at MINIT.
unsafe fn replace(function: *mut zend_function, handler: zif_handler) -> zif_handler {
    // `internal_function` starts the `zend_function` union.
    let slot = function
        .wrapping_byte_add(offset_of!(zend_internal_function, handler))
        .cast::<zif_handler>();
    // SAFETY: per the caller.
    unsafe { ptr::replace(slot, handler) }
}

/// Replace `array_push`, `array_unshift` and `array_splice`'s handlers (MINIT).
/// # Safety
/// At MINIT.
pub(crate) unsafe fn install() {
    // SAFETY: a core function, at MINIT.
    let push = unsafe { replace(compiled_function(c"array_push"), Some(push)) };
    // SAFETY: as above.
    let unshift = unsafe { replace(compiled_function(c"array_unshift"), Some(unshift)) };
    // SAFETY: as above.
    let splice = unsafe { replace(compiled_function(c"array_splice"), Some(splice)) };
    let originals = Originals {
        push,
        unshift,
        splice,
    };
    if ORIGINALS.set(originals).is_err() {
        // MINIT runs once; nothing to do on a second call.
    }
}

/// # Safety
/// The original handler of the running frame.
unsafe fn run(
    handler: fn(&Originals) -> zif_handler,
    frame: *mut zend_execute_data,
    value: *mut zval,
) {
    if let Some(handler) = ORIGINALS.get().and_then(handler) {
        // SAFETY: per the caller.
        unsafe { handler(frame, value) };
    }
}

/// The typed array a builtin's first argument names, if it is one.
/// # Safety
/// A live call frame.
unsafe fn typed_target(
    frame: *mut zend_execute_data,
) -> Option<(*mut HashTable, *mut zval, usize)> {
    // SAFETY: per the caller.
    let (args, count) = unsafe { crate::invoke::arguments(frame) };
    if count == 0 {
        return None;
    }
    // SAFETY: a passed argument.
    let array = unsafe { deref(args) };
    // SAFETY: a live zval.
    if unsafe { type_of(array) } != IS_ARRAY {
        return None;
    }
    // SAFETY: an array zval.
    let ht = unsafe { table(array) };
    // SAFETY: a live table.
    unsafe { is_typed(ht) }.then_some((ht, args, count))
}

/// Check each argument after the first as an appended value.
/// # Safety
/// `ht` is typed; `args` holds `count` zvals.
unsafe fn check_appended(ht: *mut HashTable, args: *mut zval, count: usize) -> bool {
    (1..count).all(|index| {
        // SAFETY: Zend's shared undefined zval.
        let key = unsafe { swr_zend_uninitialized_zval() };
        // SAFETY: per the caller.
        unsafe { swr_v1_phpstdext_check(ht, key, args.wrapping_add(index)) }
    })
}

unsafe extern "C" fn push(frame: *mut zend_execute_data, value: *mut zval) {
    // SAFETY: Zend's handler contract.
    if let Some((ht, args, count)) = unsafe { typed_target(frame) }
        // SAFETY: as above.
        && !unsafe { check_appended(ht, args, count) }
    {
        return;
    }
    // SAFETY: the original handler.
    unsafe { run(|originals| originals.push, frame, value) };
}

unsafe extern "C" fn unshift(frame: *mut zend_execute_data, value: *mut zval) {
    // SAFETY: Zend's handler contract.
    let target = unsafe { typed_target(frame) };
    if let Some((ht, args, count)) = target
        // SAFETY: as above.
        && !unsafe { check_appended(ht, args, count) }
    {
        return;
    }
    // SAFETY: the original handler.
    unsafe { run(|originals| originals.unshift, frame, value) };
    if let Some((ht, ..)) = target {
        // SAFETY: the table the original handler rebuilt in place.
        unsafe { set_typed(ht) };
    }
}

unsafe extern "C" fn splice(frame: *mut zend_execute_data, value: *mut zval) {
    // SAFETY: Zend's handler contract.
    let target = unsafe { typed_target(frame) };
    if let Some((ht, args, count)) = target
        && count > 3
        // SAFETY: the passed fourth argument.
        && !unsafe { check_replacement(ht, args.wrapping_add(3)) }
    {
        return;
    }
    // SAFETY: the original handler.
    unsafe { run(|originals| originals.splice, frame, value) };
    if let Some((ht, ..)) = target {
        // SAFETY: the table the original handler rebuilt in place.
        unsafe { set_typed(ht) };
    }
}

/// `array_splice`'s `$replacement`: an array of values, or one value.
/// # Safety
/// `ht` is typed; `replacement` is a live argument.
unsafe fn check_replacement(ht: *mut HashTable, replacement: *mut zval) -> bool {
    // SAFETY: per the caller.
    let values = unsafe { deref(replacement) };
    // SAFETY: a live zval.
    if unsafe { type_of(values) } != IS_ARRAY {
        // SAFETY: Zend's shared undefined zval.
        let key = unsafe { swr_zend_uninitialized_zval() };
        // SAFETY: a typed array and live zvals.
        return unsafe { swr_v1_phpstdext_check(ht, key, values) };
    }
    let mut walk = Walk { ht, ok: true };
    // SAFETY: an array zval.
    let values = unsafe { table(values) };
    // SAFETY: a live table; `walk` outlives the walk.
    unsafe { swr_zend_hash_foreach(values, Some(check_value), (&raw mut walk).cast()) };
    walk.ok
}

struct Walk {
    ht: *mut HashTable,
    ok: bool,
}

unsafe extern "C" fn check_value(value: *mut zval, context: *mut c_void) -> bool {
    // SAFETY: `check_replacement`'s `Walk`.
    let walk = unsafe { &mut *context.cast::<Walk>() };
    // SAFETY: Zend's shared undefined zval.
    let key = unsafe { swr_zend_uninitialized_zval() };
    // SAFETY: a typed array and live zvals.
    walk.ok = unsafe { swr_v1_phpstdext_check(walk.ht, key, value) };
    walk.ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_matches_upstream() {
        assert_eq!(size_of::<TypeValue>(), 6);
        assert_eq!(offset_of!(TypeInfo, ce), 16);
        assert_eq!(offset_of!(TypeInfo, len), 24);
        assert_eq!(offset_of!(TypeInfo, text), 26);
        assert_eq!(size_of::<TypeInfo>(), 32);
    }

    #[test]
    fn names_have_types() {
        let kind = |name: &[u8]| u32::from(type_of_name(name));
        assert_eq!(kind(b"int"), IS_LONG);
        assert_eq!(kind(b"FLOAT"), IS_DOUBLE);
        assert_eq!(kind(b"string"), IS_STRING);
        assert_eq!(kind(b"bool"), IS_TRUE);
        assert_eq!(kind(b"<int>"), IS_ARRAY);
        assert_eq!(kind(b"resource"), IS_RESOURCE);
        assert_eq!(kind(b"null"), IS_NULL);
        assert_eq!(kind(b"stdclass"), IS_OBJECT);
        assert_eq!(kind(b""), IS_OBJECT);
        assert_eq!(kind(b"<"), IS_OBJECT);
    }

    fn parsed(definition: &[u8]) -> TypeValue {
        let mut text = definition.to_vec();
        text.push(0);
        // SAFETY: a NUL-terminated definition.
        unsafe { split_value(text.as_ptr(), definition.len()) }.unwrap()
    }

    #[test]
    fn definitions_split_key_and_value() {
        let list = parsed(b"<int>");
        assert_eq!(
            (list.key, list.value, list.offset, list.len),
            (0, code(IS_LONG), 1, 3)
        );
        let map = parsed(b"<string,<int>>");
        assert_eq!(
            (map.key, map.value, map.offset, map.len),
            (code(IS_STRING), code(IS_ARRAY), 8, 5)
        );
        let empty = parsed(b"<int,>");
        assert_eq!((empty.value, empty.len), (code(IS_OBJECT), 0));
        let nul = parsed(b"<int\0,string>");
        assert_eq!(nul.key, 0, "strchr stops at a NUL");
    }
}
