//! Typed arrays: a `HashTable` allocated with its type definition behind it
//! and flagged [`TYPED`]. Writes check keys and values against the definition:
//! `swoole_typed_array`, the `array_push`/`array_unshift`/`array_splice`
//! overrides, and the `ASSIGN_DIM`/`UNSET_DIM`/`FE_RESET_RW` opcode handlers.
use crate::{CONTINUE, DISPATCH, Frame, invoke::compiled_function};
use std::{
    ffi::{CStr, c_char, c_int, c_long},
    marker::PhantomData,
    mem::offset_of,
    ptr, slice,
    sync::OnceLock,
};
use swoole_rs_zend::{
    Call, Key, Value, ZString,
    bailout::Bailout,
    handlers,
    sys::{
        _zend_hash_init, _zval_struct__bindgen_ty_1, _zval_struct__bindgen_ty_2, HashTable,
        IS_ARRAY, IS_ARRAY_EX, IS_DOUBLE, IS_FALSE, IS_LONG, IS_NULL, IS_OBJECT, IS_RESOURCE,
        IS_STRING, IS_TRUE, IS_UNDEF, swr_zend_efree, swr_zend_emalloc, swr_zend_instanceof,
        swr_zend_try_lookup_class, zend_class_entry, zend_execute_data, zend_function,
        zend_get_type_by_const, zend_hash_index_add, zend_hash_str_add, zend_internal_function,
        zend_long, zend_throw_error, zend_type_error, zend_value, zif_handler, zval, zval_ptr_dtor,
    },
    value::{self, table},
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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

/// `get_type_info`: the definition behind a typed array's table.
fn info_of(ht: *mut HashTable) -> *mut TypeInfo {
    ht.wrapping_byte_add(size_of::<HashTable>()).cast()
}

/// Bytes a `TypeInfo` with `len` bytes of text takes.
fn info_size(len: usize) -> usize {
    size_of::<TypeInfo>() + len + 1
}

/// The value part of a definition's text.
fn value_part(text: &[u8], this: TypeValue) -> &[u8] {
    let start = usize::from(this.offset);
    text.get(start..start + usize::from(this.len))
        .unwrap_or_default()
}

/// A typed array's table, live for `'a`, with its parsed definition behind it.
#[derive(Clone, Copy)]
pub(crate) struct Typed<'a> {
    ht: *mut HashTable,
    live: PhantomData<&'a HashTable>,
}

impl<'a> Typed<'a> {
    /// The typed array `value` holds, if it holds one (not dereferenced).
    pub(crate) fn of(value: &'a Value) -> Option<Self> {
        if value.kind() != IS_ARRAY {
            return None;
        }
        // SAFETY: a live array zval. The table pointer is read from it rather
        // than borrowed, so it reaches the definition behind the table.
        let ht = unsafe { table(value.as_ptr()) };
        // SAFETY: the zval holds the table while borrowed.
        let flags = unsafe { flags_of(ht).read() };
        (flags & TYPED != 0).then_some(Self {
            ht,
            live: PhantomData,
        })
    }

    /// # Safety
    /// `ht` is a typed array live for `'a`.
    pub(crate) unsafe fn from_table(ht: *mut HashTable) -> Self {
        Self {
            ht,
            live: PhantomData,
        }
    }

    pub(crate) fn table(self) -> *mut HashTable {
        self.ht
    }

    /// Flag the table typed again, after a builtin rebuilt it in place.
    fn mark(self) {
        // SAFETY: a live table; only its flags change.
        unsafe { *flags_of(self.ht) |= TYPED };
    }

    fn header(self) -> &'a TypeInfo {
        // SAFETY: a typed table carries a parsed definition right after it.
        unsafe { &*info_of(self.ht) }
    }

    fn this(self) -> TypeValue {
        self.header().this
    }

    /// The definition's text.
    fn text(self) -> &'a [u8] {
        let start = info_of(self.ht).wrapping_byte_add(offset_of!(TypeInfo, text));
        // SAFETY: a parsed definition's `len` bytes of text follow its header.
        unsafe { slice::from_raw_parts(start.cast::<u8>(), usize::from(self.header().len)) }
    }

    /// `get_value_type_str()` with its length.
    fn value_bytes(self) -> &'a [u8] {
        value_part(self.text(), self.this())
    }

    /// `zend_hash_num_elements`.
    fn count(self) -> u32 {
        // SAFETY: a live table.
        unsafe { (*self.ht).nNumOfElements }
    }

    /// `ArrayTypeInfo::element_type_equals`: whether `element` is the type of
    /// this array's values.
    fn element_equals(self, element: Self) -> bool {
        let (expected, actual) = (self.header().element, element.this());
        actual.key == expected.key
            && actual.value == expected.value
            && element.text() == self.value_bytes()
    }

    /// `get_array_type_def`: `<string,value>`, `<int,value>` or, for a list, `value>`.
    fn definition(self) -> ZString {
        let prefix: &[u8] = match u32::from(self.this().key) {
            IS_STRING => b"<string,",
            IS_LONG => b"<int,",
            // Upstream writes the `<` but only steps past it for a keyed type.
            _ => b"",
        };
        ZString::new(&[prefix, self.value_bytes(), b">"].concat())
    }
}

/// A definition parsed from a `swoole_typed_array` or `swoole_array_is_typed`
/// argument.
struct Parsed {
    this: TypeValue,
    element: TypeValue,
    ce: *mut zend_class_entry,
    /// Lowercase, without spaces, then a NUL.
    text: Vec<u8>,
}

impl Parsed {
    /// The text without its NUL.
    fn text(&self) -> &[u8] {
        self.text.split_last().map_or(&[], |(_, text)| text)
    }

    /// `ArrayTypeInfo::equals`.
    fn equals(&self, typed: Typed) -> bool {
        let other = typed.this();
        self.this.key == other.key && self.this.value == other.value && self.text() == typed.text()
    }

    /// Write the definition behind a table.
    /// # Safety
    /// `info` has `info_size(self.text().len())` writable bytes.
    unsafe fn write(&self, info: *mut TypeInfo) {
        let header = TypeInfo {
            this: self.this,
            element: self.element,
            ce: self.ce,
            // Shorter than 65535 bytes: `parse` checked.
            len: u16::try_from(self.text().len()).unwrap_or(u16::MAX),
            text: [],
        };
        // SAFETY: per the caller, room for the header.
        unsafe { info.write(header) };
        let text = info
            .wrapping_byte_add(offset_of!(TypeInfo, text))
            .cast::<u8>();
        // SAFETY: per the caller, room for the text and its NUL.
        unsafe { ptr::copy_nonoverlapping(self.text.as_ptr(), text, self.text.len()) };
    }
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

/// `ArrayTypeValue::parse` of the `len` bytes at `start` of `text`, a
/// NUL-terminated definition whose `strchr` runs to that NUL.
fn parse_value(text: &[u8], start: usize, len: usize) -> Option<TypeValue> {
    match split_value(text, start, len) {
        Ok(value) => Some(value),
        Err(rest) => {
            let rest = text.get(rest..).unwrap_or_default();
            // SAFETY: a printf format with the NUL-terminated rest.
            unsafe {
                zend_throw_error(
                    ptr::null_mut(),
                    c"The key type of array must be string or int, but got %s".as_ptr(),
                    rest.as_ptr(),
                );
            }
            None
        }
    }
}

/// [`parse_value`]'s split: the value, or where in `text` the text after a key
/// that is not `string` or `int` begins.
fn split_value(text: &[u8], start: usize, len: usize) -> Result<TypeValue, usize> {
    let from_start = text.get(start..).unwrap_or_default();
    let rest = from_start
        .split(|&byte| byte == 0)
        .next()
        .unwrap_or_default();
    let bytes = |from: usize, count: usize| {
        from_start
            .get(from..from.saturating_add(count))
            .unwrap_or_default()
    };
    let mut target = TypeValue::default();
    match rest.iter().position(|&byte| byte == b',') {
        None => target.offset = 1,
        Some(comma) => {
            target.key = type_of_name(bytes(1, comma.saturating_sub(1)));
            if !matches!(u32::from(target.key), IS_STRING | IS_LONG) {
                return Err(start + comma + 1);
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

/// `ArrayTypeInfo::parse`.
fn parse(source: &[u8]) -> Result<Parsed, Failure> {
    if u16::try_from(source.len()).is_err() || source.len() == usize::from(u16::MAX) {
        throw(c"The type definition string is too long (must be less than 65535 characters)");
        return Err(Failure::Thrown);
    }
    // `zend_string_tolower`, then `remove_all_spaces`.
    let mut text: Vec<u8> = source
        .iter()
        .map(u8::to_ascii_lowercase)
        .filter(|&byte| isspace(c_int::from(byte)) == 0)
        .collect();
    let len = text.len();
    if text.first() != Some(&b'<') || text.last() != Some(&b'>') {
        throw(c"The type definition of typed array must start with '<' and end with '>'");
        return Err(Failure::Thrown);
    }
    text.push(0);
    let this = parse_value(&text, 0, len).ok_or(Failure::Thrown)?;
    let mut parsed = Parsed {
        this,
        element: TypeValue::default(),
        ce: ptr::null_mut(),
        text,
    };
    if u32::from(this.value) == IS_OBJECT {
        parsed.ce = lookup_class(&ZString::new(value_part(parsed.text(), this)))?;
    }
    Ok(parsed)
}

/// `zend_lookup_class(name)`, throwing "Class '%s' not found".
fn lookup_class(name: &ZString) -> Result<*mut zend_class_entry, Failure> {
    let mut ce = ptr::null_mut();
    // SAFETY: a live name; autoloaders run under `zend_try`.
    if unsafe { swr_zend_try_lookup_class(name.as_ptr(), &raw mut ce) } != 0 {
        return Err(Failure::Bailout(Bailout));
    }
    if ce.is_null() {
        // SAFETY: a printf format with the NUL-terminated name.
        unsafe {
            zend_throw_error(
                ptr::null_mut(),
                c"Class '%s' not found".as_ptr(),
                name.c_str().as_ptr(),
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

/// A key as [`check`] reads it: its type and, for an int, its value.
#[derive(Clone, Copy)]
pub(crate) struct KeyType {
    kind: u32,
    index: zend_long,
}

impl KeyType {
    /// No key: an append.
    pub(crate) const UNDEF: Self = Self {
        kind: IS_UNDEF,
        index: 0,
    };

    pub(crate) fn of(key: &Value) -> Self {
        Self {
            kind: key.kind(),
            index: key.as_long().unwrap_or(0),
        }
    }
}

/// `ArrayTypeInfo::check`: whether `key => value` may go in `typed`; if not,
/// Zend has thrown.
pub(crate) fn check(typed: Typed, key: KeyType, value: &Value) -> bool {
    let this = typed.this();
    if !check_key(typed, this, key) {
        return false;
    }
    let value = value.dereferenced();
    let value_type = value.kind();
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
    match (value.class(), Typed::of(value)) {
        (Some(actual), _) => check_object(typed.header().ce, actual),
        (None, element) if expected == IS_ARRAY => check_array(typed, element),
        _ => true,
    }
}

/// The key half of [`check`].
fn check_key(typed: Typed, this: TypeValue, key: KeyType) -> bool {
    if this.key > 0 {
        if key.kind == u32::from(this.key) {
            return true;
        }
        // SAFETY: printf arguments: two strings.
        unsafe {
            zend_type_error(
                c"Array key type mismatch, expected `%s`, got `%s`".as_ptr(),
                type_name(this.key.into()),
                type_name(key.kind),
            );
        }
        return false;
    }
    if key.kind == IS_LONG {
        if key.index > i64::from(typed.count()) {
            // SAFETY: printf arguments: a long.
            unsafe {
                zend_throw_error(
                    ptr::null_mut(),
                    c"Incorrect array key `%ld`, out of the permitted range".as_ptr(),
                    key.index as c_long,
                );
            }
            return false;
        }
    } else if !matches!(key.kind, IS_UNDEF | IS_NULL) {
        throw(c"Incorrect array key, must be undef or int");
        return false;
    }
    true
}

/// Whether an object of class `actual` is a `ce`.
fn check_object(ce: *mut zend_class_entry, actual: *mut zend_class_entry) -> bool {
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
            swoole_rs_zend::string::val(expected_name),
            swoole_rs_zend::string::val(actual_name),
        );
    }
    false
}

/// Whether `element`, an array value, is of the value type of `typed`, an
/// array of arrays.
fn check_array(typed: Typed, element: Option<Typed>) -> bool {
    let expected = typed.value_bytes();
    // Shorter than 65535 bytes.
    let expected_len = c_int::try_from(expected.len()).unwrap_or(c_int::MAX);
    let Some(element) = element else {
        // SAFETY: printf arguments: a length and its bytes.
        unsafe {
            zend_type_error(
                c"Array value type mismatch, expected `%.*s`, got `array`".as_ptr(),
                expected_len,
                expected.as_ptr(),
            );
        }
        return false;
    };
    if typed.element_equals(element) {
        return true;
    }
    let actual = element.definition();
    // SAFETY: printf arguments: two lengths and their bytes.
    unsafe {
        zend_type_error(
            c"Array value type mismatch, expected `%.*s`, got `%.*s`".as_ptr(),
            expected_len,
            expected.as_ptr(),
            c_int::try_from(actual.bytes().len()).unwrap_or(c_int::MAX),
            actual.bytes().as_ptr(),
        );
    }
    false
}

/// `copy_array_type_info`: after `SEPARATE_ARRAY(container)` made a copy of
/// the typed array `source`, give the copy `source`'s definition, without
/// resuming a contained bailout.
/// # Safety
/// `container` holds the fresh separated array.
pub(crate) unsafe fn copy_type(container: *mut zval, source: Typed) -> Result<(), Bailout> {
    let source_info = info_of(source.ht);
    let extra = info_size(usize::from(source.header().len));
    // SAFETY: an array zval.
    let ht = unsafe { table(container) };
    let mut memory = ptr::null_mut();
    // SAFETY: allocate the real table header and trailing type metadata below a native trap.
    swoole_rs_zend::bailout::trapped(unsafe {
        swoole_rs_zend::sys::swr_zend_try_facade_alloc(
            size_of::<HashTable>() + extra,
            &raw mut memory,
        )
    })?;
    let copy = memory.cast::<HashTable>();
    // SAFETY: both hold a table.
    unsafe { ptr::copy_nonoverlapping(ht, copy, 1) };
    // SAFETY: the source holds `extra` bytes of definition; the copy has room.
    unsafe { ptr::copy_nonoverlapping(source_info.cast::<u8>(), info_of(copy).cast(), extra) };
    // SAFETY: the table just made, with the definition copied behind it.
    unsafe { Typed::from_table(copy) }.mark();
    // SAFETY: an array zval, which now holds the copy.
    unsafe { (*container).value = zend_value { arr: copy } };
    // SAFETY: the old allocation, which nothing references now.
    unsafe { swr_zend_efree(ht.cast()) };
    Ok(())
}

/// The `ASSIGN_DIM_OP` restriction: concatenation needs string values,
/// arithmetic int or float values. If not, Zend has thrown.
pub(crate) fn operation(typed: Typed, concat: bool) -> bool {
    let value = u32::from(typed.this().value);
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
pub unsafe extern "C" fn swr_v1_phpstdext_unset(frame: *mut zend_execute_data) -> c_int {
    // SAFETY: per Zend's contract.
    let frame = unsafe { Frame::new(frame) };
    // `get_array_on_opline`: op1's slot.
    let Some(array) = frame.op().op1_indirect() else {
        return DISPATCH;
    };
    match Typed::of(array.dereferenced()) {
        Some(typed) if typed.this().key == 0 => {
            throw(c"The typed array list do not support random deletion of elements");
            CONTINUE
        }
        _ => DISPATCH,
    }
}

/// The `FE_RESET_RW` user opcode handler: no `foreach` by reference over a
/// typed array.
/// # Safety
/// Zend's user opcode handler contract.
pub unsafe extern "C" fn swr_v1_phpstdext_foreach_begin(frame: *mut zend_execute_data) -> c_int {
    // SAFETY: per Zend's contract.
    let frame = unsafe { Frame::new(frame) };
    let op = frame.op();
    if op
        .op1_indirect()
        .and_then(|array| Typed::of(array.dereferenced()))
        .is_none()
    {
        return DISPATCH;
    }
    throw(c"The type array do not support using references for element value during iteration");
    if let Some(result) = op.result() {
        let result = result.as_ptr();
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
    }
    CONTINUE
}

handlers! {
    zif_swoole_typed_array => typed_array;
    zif_swoole_array_is_typed => array_is_typed;
}

/// `swoole_typed_array(string $typeDef, ?array $initArray = null)`.
fn typed_array(call: &Call) {
    let parsed = call.arity(1, 2).and_then(|()| {
        let definition = call.zstr(1)?;
        if call.num_args() >= 2 {
            call.array_ref(2)?;
        }
        Ok(definition)
    });
    let Ok(Some(definition)) = parsed else {
        return;
    };
    let init = call.value(2);
    let outcome = parse(definition.bytes()).and_then(|parsed| match init {
        Some(init) => from_array(call, &parsed, init),
        None => fill(call, &parsed, None),
    });
    if let Err(failure) = outcome {
        call.set_null();
        if let Failure::Bailout(bailout) = failure {
            swoole_rs_zend::bailout::raise(Err(bailout));
        }
    }
}

/// `swoole_typed_array` with initial values: a typed array of the same
/// definition is returned as is.
fn from_array(call: &Call, parsed: &Parsed, init: &Value) -> Result<(), Failure> {
    let Some(typed) = Typed::of(init) else {
        return fill(call, parsed, Some(init));
    };
    if parsed.equals(typed) {
        call.set_owned(init.copy());
    } else {
        throw(c"The type definition of the typed array does not match the initial values");
    }
    Ok(())
}

/// `make_typed_array`: a table with room for `len` bytes of definition.
fn make_typed_array(size: u32, len: usize) -> *mut HashTable {
    // SAFETY: Zend's request allocator.
    let ht = unsafe { swr_zend_emalloc(size_of::<HashTable>() + info_size(len)) }.cast();
    // SAFETY: room for a table.
    unsafe { _zend_hash_init(ht, size, Some(zval_ptr_dtor), false) };
    ht
}

/// A new typed array of `parsed`'s definition, holding `init`'s entries.
fn fill(call: &Call, parsed: &Parsed, init: Option<&Value>) -> Result<(), Failure> {
    let init = init.and_then(Value::array);
    let mut definition = Parsed {
        this: parsed.this,
        element: TypeValue::default(),
        ce: parsed.ce,
        text: parsed.text.clone(),
    };
    let this = parsed.this;
    if u32::from(this.value) == IS_ARRAY {
        let start = usize::from(this.offset);
        definition.element =
            parse_value(&parsed.text, start, usize::from(this.len)).ok_or(Failure::Thrown)?;
    }
    let array = make_typed_array(
        init.map_or(0, swoole_rs_zend::Array::count),
        parsed.text().len(),
    );
    // SAFETY: room for the definition behind the table just made.
    unsafe { definition.write(info_of(array)) };
    // SAFETY: the table just made, with its definition.
    let typed = unsafe { Typed::from_table(array) };
    typed.mark();
    let value = call.return_value();
    // SAFETY: `ZVAL_ARR(return_value, array)`.
    unsafe { value::set(value, zend_value { arr: array }, IS_ARRAY_EX) };
    let Some(init) = init else {
        return Ok(());
    };
    let mut entries = init.entries();
    while let Some((key, element)) = entries.next_keyed() {
        let key_type = match key {
            Key::Index(index) => KeyType {
                kind: IS_LONG,
                index: index.cast_signed(),
            },
            Key::Name(_) => KeyType {
                kind: IS_STRING,
                index: 0,
            },
        };
        if !check(typed, key_type, element) {
            // SAFETY: the array made above.
            unsafe { zval_ptr_dtor(value) };
            return Err(Failure::Thrown);
        }
        // The array takes the new reference.
        let mut copy = element.copy().into_zval();
        match key {
            // SAFETY: a live table.
            Key::Index(index) => unsafe { zend_hash_index_add(array, index, &raw mut copy) },
            // SAFETY: as above; Zend copies the key.
            Key::Name(name) => unsafe {
                zend_hash_str_add(array, name.as_ptr().cast(), name.len(), &raw mut copy)
            },
        };
    }
    Ok(())
}

/// `swoole_array_is_typed(array $array, string $typeDef = '')`.
fn array_is_typed(call: &Call) {
    let parsed = call
        .arity(1, 2)
        .and_then(|()| Ok((call.array_ref(1)?, call.zstr(2)?)));
    let Ok((_, definition)) = parsed else {
        return;
    };
    let Some(typed) = call.value(1).and_then(Typed::of) else {
        call.set_bool(false);
        return;
    };
    let Some(definition) = definition else {
        call.set_bool(true);
        return;
    };
    match parse(definition.bytes()) {
        Ok(parsed) => call.set_bool(parsed.equals(typed)),
        Err(failure) => {
            call.set_bool(false);
            if let Failure::Bailout(bailout) = failure {
                swoole_rs_zend::bailout::raise(Err(bailout));
            }
        }
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
fn typed_target(call: &Call) -> Option<Typed<'_>> {
    Typed::of(call.value(1)?.dereferenced())
}

/// Check each argument after the first as an appended value.
fn check_appended(typed: Typed, call: &Call) -> bool {
    (2..=call.num_args())
        .filter_map(|n| call.value(n))
        .all(|value| check(typed, KeyType::UNDEF, value))
}

unsafe extern "C" fn push(frame: *mut zend_execute_data, value: *mut zval) {
    // SAFETY: Zend's handler contract.
    let call = unsafe { Call::new(frame, value) };
    if let Some(typed) = typed_target(&call)
        && !check_appended(typed, &call)
    {
        return;
    }
    // SAFETY: the original handler.
    unsafe { run(|originals| originals.push, frame, value) };
}

unsafe extern "C" fn unshift(frame: *mut zend_execute_data, value: *mut zval) {
    // SAFETY: Zend's handler contract.
    let call = unsafe { Call::new(frame, value) };
    let target = typed_target(&call);
    if let Some(typed) = target
        && !check_appended(typed, &call)
    {
        return;
    }
    // SAFETY: the original handler.
    unsafe { run(|originals| originals.unshift, frame, value) };
    // The table the original handler rebuilt in place.
    if let Some(typed) = target {
        typed.mark();
    }
}

unsafe extern "C" fn splice(frame: *mut zend_execute_data, value: *mut zval) {
    // SAFETY: Zend's handler contract.
    let call = unsafe { Call::new(frame, value) };
    let target = typed_target(&call);
    if let Some(typed) = target
        && let Some(replacement) = call.value(4)
        && !check_replacement(typed, replacement)
    {
        return;
    }
    // SAFETY: the original handler.
    unsafe { run(|originals| originals.splice, frame, value) };
    // The table the original handler rebuilt in place.
    if let Some(typed) = target {
        typed.mark();
    }
}

/// `array_splice`'s `$replacement`: an array of values, or one value.
fn check_replacement(typed: Typed, replacement: &Value) -> bool {
    let values = replacement.dereferenced();
    let Some(values) = values.array() else {
        return check(typed, KeyType::UNDEF, values);
    };
    values
        .entries()
        .all(|(_, value)| check(typed, KeyType::UNDEF, value))
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
        split_value(&text, 0, definition.len()).unwrap_or_default()
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

    #[test]
    fn the_text_follows_the_header_intact() {
        let parsed = Parsed {
            this: TypeValue::default(),
            element: TypeValue::default(),
            ce: ptr::null_mut(),
            text: b"<string,int>\0".to_vec(),
        };
        let mut buffer = vec![u64::MAX; info_size(12).div_ceil(8)];
        let info = buffer.as_mut_ptr().cast::<TypeInfo>();
        unsafe { parsed.write(info) };
        let text = info
            .wrapping_byte_add(offset_of!(TypeInfo, text))
            .cast::<u8>();
        assert_eq!(
            unsafe { slice::from_raw_parts(text, 13) },
            b"<string,int>\0"
        );
        assert_eq!(unsafe { (*info).len }, 12);
    }

    #[test]
    fn a_bad_key_reports_the_rest() {
        let text = b"<float,int>\0";
        assert_eq!(split_value(text, 0, 11), Err(7));
        let nested = b"<int,<x,int>>\0";
        assert_eq!(split_value(nested, 5, 7), Err(8));
    }
}
