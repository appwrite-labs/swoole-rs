//! Typed arrays: a `HashTable` allocated with its type definition behind it
//! and flagged [`TYPED`]. Writes check keys and values against the definition:
//! `swoole_typed_array`, the `array_push`/`array_unshift`/`array_splice`
//! overrides, and the `ASSIGN_DIM`/`UNSET_DIM`/`FE_RESET_RW` opcode handlers.
use crate::{CONTINUE, DISPATCH, Frame, Op, invoke::compiled_function};
use std::{
    ffi::{CStr, CString, c_int, c_long},
    marker::PhantomData,
    mem::offset_of,
    ptr, slice,
    sync::OnceLock,
};
use swoole_rs_zend::{
    Call, Owned, UNDEF, Value, ZString,
    bailout::Bailout,
    handlers,
    opcode::Handler,
    sys::{
        _zend_hash_init, _zval_struct__bindgen_ty_2, HashTable, IS_ARRAY, IS_ARRAY_EX, IS_DOUBLE,
        IS_FALSE, IS_LONG, IS_NULL, IS_OBJECT, IS_RESOURCE, IS_STRING, IS_TRUE, IS_UNDEF,
        swr_zend_efree, swr_zend_emalloc, swr_zend_instanceof, swr_zend_separate_array,
        swr_zend_try_lookup_class, zend_class_entry, zend_execute_data, zend_internal_function,
        zend_long, zend_value, zif_handler, zval, zval_ptr_dtor,
    },
    throw_error, type_error,
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

    /// The table, for Zend's hash API.
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
            throw_error(&message(&[
                b"The key type of array must be string or int, but got ",
                rest,
            ]));
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
        throw_error(c"The type definition string is too long (must be less than 65535 characters)");
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
        throw_error(c"The type definition of typed array must start with '<' and end with '>'");
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
        throw_error(&message(&[b"Class '", name.bytes(), b"' not found"]));
        return Err(Failure::Thrown);
    }
    Ok(ce)
}

/// The parts in a row, each as printf's `%s` (or `%.*s`) prints it: up to its
/// first NUL.
pub(crate) fn message(parts: &[&[u8]]) -> CString {
    let text = parts
        .iter()
        .flat_map(|part| part.split(|&byte| byte == 0).next().unwrap_or_default());
    CString::new(text.copied().collect::<Vec<u8>>()).unwrap_or_default()
}

/// `zend_get_type_by_const`, as printf's `%s` shows it.
fn type_name(kind: u32) -> &'static [u8] {
    // "(null)" is what Zend's printf shows for a null string.
    swoole_rs_zend::type_name(kind).map_or(b"(null)", CStr::to_bytes)
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
        type_error(&message(&[
            b"Array value type mismatch, expected `",
            type_name(expected),
            b"`, got `",
            type_name(value_type),
            b"`",
        ]));
        return false;
    }
    match (value.class(), Typed::of(&value)) {
        (Some(actual), _) => check_object(typed, &value, actual),
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
        type_error(&message(&[
            b"Array key type mismatch, expected `",
            type_name(this.key.into()),
            b"`, got `",
            type_name(key.kind),
            b"`",
        ]));
        return false;
    }
    if key.kind == IS_LONG {
        if key.index > i64::from(typed.count()) {
            // `%ld` of the key.
            let index = (key.index as c_long).to_string();
            throw_error(&message(&[
                b"Incorrect array key `",
                index.as_bytes(),
                b"`, out of the permitted range",
            ]));
            return false;
        }
    } else if !matches!(key.kind, IS_UNDEF | IS_NULL) {
        throw_error(c"Incorrect array key, must be undef or int");
        return false;
    }
    true
}

/// Whether `value`, an object of class `actual`, is of the class `typed`'s
/// values have.
fn check_object(typed: Typed, value: &Value, actual: *mut zend_class_entry) -> bool {
    let ce = typed.header().ce;
    // SAFETY: live classes: `value`'s own, and the one the definition's parse
    // looked up for its object values.
    if unsafe { swr_zend_instanceof(actual, ce) } {
        return true;
    }
    // SAFETY: that class, which the definition's parse found and so is never
    // null; classes outlive the request's arrays.
    let expected = unsafe { swoole_rs_zend::ClassEntry::from_ptr(ce) };
    type_error(&message(&[
        b"Array value type mismatch, expected `",
        expected.map_or(&[][..], |class| class.name().bytes()),
        b"`, got `",
        // An object's value name is its class's.
        value.value_name().to_bytes(),
        b"`",
    ]));
    false
}

/// Whether `element`, an array value, is of the value type of `typed`, an
/// array of arrays.
fn check_array(typed: Typed, element: Option<Typed>) -> bool {
    let expected = typed.value_bytes();
    let Some(element) = element else {
        type_error(&message(&[
            b"Array value type mismatch, expected `",
            expected,
            b"`, got `array`",
        ]));
        return false;
    };
    if typed.element_equals(element) {
        return true;
    }
    let actual = element.definition();
    type_error(&message(&[
        b"Array value type mismatch, expected `",
        expected,
        b"`, got `",
        actual.bytes(),
        b"`",
    ]));
    false
}

/// `SEPARATE_ARRAY(container)`, then `copy_array_type_info`: a copy the
/// separation made gets `source`'s definition, in a fresh allocation that has
/// room for it behind the table. The typed table `container` holds afterwards.
/// # Safety
/// `container` holds `source`, which nothing uses afterwards.
pub(crate) unsafe fn separate<'a>(container: &'a Value, source: Typed<'a>) -> Typed<'a> {
    let container = container.as_ptr();
    // SAFETY: Zend separates this exact live array slot.
    unsafe { swr_zend_separate_array(container) };
    // SAFETY: successful separation leaves the container holding an initialized array.
    let ht = unsafe { table(container) };
    if source.ht == ht {
        return source;
    }
    // `source` remains shared or immutable; `ht` is the fresh unshared copy.
    let source_info = info_of(source.ht);
    let extra = info_size(usize::from(source.header().len));
    // SAFETY: allocate the real table header and trailing type metadata.
    let copy = unsafe { swr_zend_emalloc(size_of::<HashTable>() + extra) }.cast::<HashTable>();
    // SAFETY: both hold a table.
    unsafe { ptr::copy_nonoverlapping(ht, copy, 1) };
    // SAFETY: the source holds `extra` bytes of definition; the copy has room.
    unsafe { ptr::copy_nonoverlapping(source_info.cast::<u8>(), info_of(copy).cast(), extra) };
    // SAFETY: the table just made, with the definition copied behind it.
    let own = unsafe { Typed::from_table(copy) };
    own.mark();
    // SAFETY: an array zval, which now holds the copy.
    unsafe { (*container).value = zend_value { arr: copy } };
    // SAFETY: the old allocation, which nothing references now.
    unsafe { swr_zend_efree(ht.cast()) };
    own
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
        type_error(message);
    }
    allowed
}

/// The `UNSET_DIM` user opcode handler: a typed list keeps its keys.
pub(crate) struct Unset;

impl Handler for Unset {
    fn run(frame: Frame<'_>) -> c_int {
        // `get_array_on_opline`: op1's slot.
        let Some(array) = Op::current(frame).op1_container() else {
            return DISPATCH;
        };
        match Typed::of(array) {
            Some(typed) if typed.this().key == 0 => {
                throw_error(c"The typed array list do not support random deletion of elements");
                CONTINUE
            }
            _ => DISPATCH,
        }
    }
}

/// The `FE_RESET_RW` user opcode handler: no `foreach` by reference over a
/// typed array.
pub(crate) struct ForeachBegin;

impl Handler for ForeachBegin {
    fn run(frame: Frame<'_>) -> c_int {
        let op = Op::current(frame);
        if op.op1_container().and_then(Typed::of).is_none() {
            return DISPATCH;
        }
        throw_error(
            c"The type array do not support using references for element value during iteration",
        );
        if let Some(result) = op.result() {
            let mut undefined = UNDEF;
            undefined.u2 = _zval_struct__bindgen_ty_2 {
                fe_iter_idx: u32::MAX,
            };
            // SAFETY: the result slot: `ZVAL_UNDEF`, then `Z_FE_ITER_P(result) = -1`.
            unsafe { result.as_ptr().write(undefined) };
        }
        CONTINUE
    }
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
        throw_error(c"The type definition of the typed array does not match the initial values");
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
    let mut made = Owned::default();
    // SAFETY: `ZVAL_ARR(made, array)`: `made` takes the new table's reference.
    unsafe { value::set(made.as_ptr(), zend_value { arr: array }, IS_ARRAY_EX) };
    if let Some(init) = init
        && let Some(target) = made.array_mut()
    {
        let mut entries = init.entries();
        while let Some((name, index, element)) = entries.next_stored() {
            let key_type = KeyType {
                kind: if name.is_some() { IS_STRING } else { IS_LONG },
                index: if name.is_some() {
                    0
                } else {
                    index.cast_signed()
                },
            };
            if !check(typed, key_type, element) {
                // `made` releases the array.
                return Err(Failure::Thrown);
            }
            // The array takes the new reference; init's keys are distinct.
            match name {
                Some(name) => target.add_new(name, element.copy()),
                None => target.insert_index(index, element.copy()),
            }
        }
    }
    call.set_owned(made);
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

/// Replace `array_push`, `array_unshift` and `array_splice`'s handlers (MINIT).
/// # Safety
/// At MINIT.
pub(crate) unsafe fn install() {
    type Handler = unsafe extern "C" fn(*mut zend_execute_data, *mut zval);
    let [push, unshift, splice] = [
        (c"array_push", zif_swoole_array_push as Handler),
        (c"array_unshift", zif_swoole_array_unshift),
        (c"array_splice", zif_swoole_array_splice),
    ]
    .map(|(name, handler)| {
        // `internal_function` starts the `zend_function` union.
        let slot = compiled_function(name)
            .wrapping_byte_add(offset_of!(zend_internal_function, handler))
            .cast::<zif_handler>();
        // SAFETY: a core internal function's handler slot, at MINIT.
        unsafe { ptr::replace(slot, Some(handler)) }
    });
    let originals = Originals {
        push,
        unshift,
        splice,
    };
    if ORIGINALS.set(originals).is_err() {
        // MINIT runs once; nothing to do on a second call.
    }
}

/// The typed array a builtin's first argument names, if it is one.
fn typed_target(call: &Call) -> Option<Typed<'_>> {
    let array = call.value(1)?.referent();
    // SAFETY: the array the by-reference argument refers to, which the builtin
    // works on in place.
    Typed::of(unsafe { Value::from_ptr(array) }?)
}

/// Check each argument after the first as an appended value.
fn check_appended(typed: Typed, call: &Call) -> bool {
    (2..=call.num_args())
        .filter_map(|n| call.value(n))
        .all(|value| check(typed, KeyType::UNDEF, value))
}

/// An override: unless the first argument names a typed array that `allowed`
/// refuses (Zend has thrown), run the `original` handler, then, with `remark`,
/// flag the table it rebuilt in place typed again. The original may bail out
/// through this frame and its handler's, which own nothing to drop.
fn guarded(
    call: &Call,
    original: fn(&Originals) -> zif_handler,
    remark: bool,
    allowed: fn(Typed, &Call) -> bool,
) {
    let target = typed_target(call);
    if let Some(typed) = target
        && !allowed(typed, call)
    {
        return;
    }
    if let Some(handler) = ORIGINALS.get().and_then(original) {
        // SAFETY: a core function's handler, whose whole contract is a live
        // handler frame and return slot: `call`'s. It parses its own arguments.
        unsafe { handler(call.frame(), call.return_value()) };
    }
    if remark && let Some(typed) = target {
        typed.mark();
    }
}

// Upstream's `PHP_FUNCTION` names; `install` puts them in the core functions' slots.
swoole_rs_zend::handlers! {
    leaf:
    zif_swoole_array_push => push;
    zif_swoole_array_unshift => unshift;
    zif_swoole_array_splice => splice;
}

fn push(call: &Call) {
    guarded(call, |originals| originals.push, false, check_appended);
}

fn unshift(call: &Call) {
    guarded(call, |originals| originals.unshift, true, check_appended);
}

fn splice(call: &Call) {
    guarded(call, |originals| originals.splice, true, spliced);
}

/// `array_splice`'s `$replacement`, if passed, may go in `typed`.
fn spliced(typed: Typed, call: &Call) -> bool {
    call.value(4)
        .is_none_or(|replacement| check_replacement(typed, replacement))
}

/// `array_splice`'s `$replacement`: an array of values, or one value.
fn check_replacement(typed: Typed, replacement: &Value) -> bool {
    let values = replacement.dereferenced();
    let Some(values) = values.array() else {
        return check(typed, KeyType::UNDEF, &values);
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
