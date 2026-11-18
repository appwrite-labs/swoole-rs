//! Typed arrays: a `HashTable` allocated with its type definition behind it
//! and flagged [`TYPED`]. Writes check keys and values against the definition:
//! `swoole_typed_array`, the `array_push`/`array_unshift`/`array_splice`
//! overrides, and the `ASSIGN_DIM`/`UNSET_DIM`/`FE_RESET_RW` opcode handlers.
use crate::{Frame, Next, Op};
use std::{
    cell::Cell,
    ffi::{CStr, CString, c_int},
    io::IoSlice,
    mem::offset_of,
    slice,
    sync::OnceLock,
};
use swoole_rs_zend::{
    Array, Call, ClassEntry, Owned, Php, Value, ZString,
    bailout::Bailout,
    handlers,
    opcode::Handler,
    sys::{
        HashTable, IS_ARRAY, IS_DOUBLE, IS_FALSE, IS_LONG, IS_NULL, IS_OBJECT, IS_RESOURCE,
        IS_STRING, IS_TRUE, IS_UNDEF, zend_long, zif_handler,
    },
    throw_error, type_error,
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
/// Its `ce` is the class of an object value type, which outlives the request's
/// arrays: borrowed for `'a`, as long as the array it describes.
#[repr(C)]
struct TypeInfo<'a> {
    this: TypeValue,
    element: TypeValue,
    ce: Option<ClassEntry<'a>>,
    len: u16,
    text: [u8; 0],
}

const _: () = assert!(
    size_of::<TypeInfo<'static>>() == 32 && offset_of!(TypeInfo, text) == 26,
    "stdext.c's swr_type_info"
);

/// Why a definition did not parse.
enum Failure {
    /// Zend has thrown.
    Thrown,
    Bailout(Bailout),
}

/// `get_type_info`: the definition behind a typed array's table.
fn info_of<'a>(ht: *mut HashTable) -> *mut TypeInfo<'a> {
    ht.wrapping_byte_add(size_of::<HashTable>()).cast()
}

/// Bytes a `TypeInfo` with `len` bytes of text takes.
fn info_size(len: usize) -> usize {
    size_of::<TypeInfo<'static>>() + len + 1
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
    /// The pointer the zval holds, which reaches the definition behind the table.
    ht: *mut HashTable,
    array: &'a Array,
}

impl<'a> Typed<'a> {
    /// The typed array `value` holds, if it holds one (not dereferenced).
    pub(crate) fn of(value: &'a Value) -> Option<Self> {
        let table = Self::held(value)?;
        (table.flags().get() & TYPED != 0).then_some(table)
    }

    /// The table `value` holds, typed or about to be.
    fn held(value: &'a Value) -> Option<Self> {
        Some(Self {
            ht: value.array_ptr()?,
            array: value.array()?,
        })
    }

    /// The table, for Zend's hash API.
    pub(crate) fn table(self) -> *mut HashTable {
        self.ht
    }

    /// `HT_FLAGS(ht)`.
    #[inline]
    fn flags(self) -> &'a Cell<u32> {
        self.array.flags()
    }

    /// Flag the table typed again, after a builtin rebuilt it in place.
    fn mark(self) {
        let flags = self.flags();
        flags.set(flags.get() | TYPED);
    }

    fn header(self) -> &'a TypeInfo<'a> {
        // SAFETY: a typed table carries a parsed definition right after it,
        // whose class (one `Php::lookup_class` found) outlives the table.
        unsafe { &*info_of(self.ht) }
    }

    fn this(self) -> TypeValue {
        self.header().this
    }

    /// The definition behind the table: its header, text and NUL.
    fn info(self) -> &'a [u8] {
        let length = info_size(usize::from(self.header().len));
        // SAFETY: a parsed definition, `info_size` of its text's length,
        // follows the table, which Zend never writes.
        unsafe { slice::from_raw_parts(info_of(self.ht).cast::<u8>(), length) }
    }

    /// The definition's text.
    fn text(self) -> &'a [u8] {
        let start = offset_of!(TypeInfo, text);
        let text = self
            .info()
            .get(start..start + usize::from(self.header().len));
        text.unwrap_or_default()
    }

    /// `get_value_type_str()` with its length.
    fn value_bytes(self) -> &'a [u8] {
        value_part(self.text(), self.this())
    }

    /// `zend_hash_num_elements`.
    fn count(self) -> u32 {
        self.array.elements()
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
struct Parsed<'a> {
    this: TypeValue,
    element: TypeValue,
    ce: Option<ClassEntry<'a>>,
    /// Lowercase, without spaces, then a NUL.
    text: Vec<u8>,
}

impl Parsed<'_> {
    /// The text without its NUL.
    fn text(&self) -> &[u8] {
        self.text.split_last().map_or(&[], |(_, text)| text)
    }

    /// `ArrayTypeInfo::equals`.
    fn equals(&self, typed: Typed) -> bool {
        let other = typed.this();
        self.this.key == other.key && self.this.value == other.value && self.text() == typed.text()
    }

    /// The header behind a table: what [`typed_new`] copies there, then the text.
    fn header(&self) -> TypeInfo<'_> {
        TypeInfo {
            this: self.this,
            element: self.element,
            ce: self.ce,
            // Shorter than 65535 bytes: `parse` checked.
            len: u16::try_from(self.text().len()).unwrap_or(u16::MAX),
            text: [],
        }
    }
}

unsafe extern "C" {
    /// libc's `isspace`, as upstream's `remove_all_spaces` classifies bytes.
    safe fn isspace(c: c_int) -> c_int;
    /// `SEPARATE_ARRAY` of an array zval (anything else is left alone): a
    /// shared table is copied for the slot, with the lent bytes (an `IoSlice`
    /// is an iovec) behind the copy, and its count only drops, so it stays
    /// live for whoever else holds it.
    #[link_name = "swr_phpstdext_separate"]
    safe fn separate_array(array: &Value, info: IoSlice<'_>);
    /// `ZVAL_ARR(made, ...)` of a new table of `size` elements with a copy of
    /// `header` behind it, then `header.len` bytes of the lent `text` (zeros past
    /// its end) and a NUL: the table. What `made` held is not released.
    #[link_name = "swr_phpstdext_typed_new"]
    safe fn typed_new(
        made: &Owned,
        size: u32,
        header: &TypeInfo<'_>,
        text: IoSlice<'_>,
    ) -> *mut HashTable;
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
fn parse<'a>(php: Php<'a>, source: &[u8]) -> Result<Parsed<'a>, Failure> {
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
        ce: None,
        text,
    };
    if u32::from(this.value) == IS_OBJECT {
        let name = ZString::new(value_part(parsed.text(), this));
        let Some(ce) = php.lookup_class(&name).map_err(Failure::Bailout)? else {
            throw_error(&message(&[b"Class '", name.bytes(), b"' not found"]));
            return Err(Failure::Thrown);
        };
        parsed.ce = Some(ce);
    }
    Ok(parsed)
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
    match (value.class_entry(), Typed::of(&value)) {
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
            let index = key.index.to_string();
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
fn check_object(typed: Typed, value: &Value, actual: ClassEntry<'_>) -> bool {
    // The class the definition's parse looked up for its object values.
    let expected = typed.header().ce;
    if expected.is_some_and(|expected| actual.instance_of(expected)) {
        return true;
    }
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

/// `SEPARATE_ARRAY(container)`, then `copy_array_type_info`: the typed array
/// `value` holds, unshared. A copy the separation made gets the definition, in
/// a fresh allocation that has room for it behind the table; the shared
/// original stays with its other holders. None unless `value` holds a typed
/// array.
pub(crate) fn separate(value: &Value) -> Option<Typed<'_>> {
    let source = Typed::of(value)?;
    separate_array(value, IoSlice::new(source.info()));
    // Separation leaves the container holding an array: `source`'s, or its
    // copy with the definition behind it.
    let own = Typed::held(value)?;
    if source.ht == own.ht {
        return Some(source);
    }
    // The fresh unshared copy, which `value` holds for `'a`.
    own.mark();
    Some(own)
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
    const OPCODE: u32 = swoole_rs_zend::sys::ZEND_UNSET_DIM;
    fn run(frame: Frame<'_>) -> Next {
        // `get_array_on_opline`: op1's slot.
        let Some(array) = Op::current(frame).and_then(Op::op1_container) else {
            return Next::Dispatch;
        };
        match Typed::of(array) {
            Some(typed) if typed.this().key == 0 => {
                throw_error(c"The typed array list do not support random deletion of elements");
                Next::Continue
            }
            _ => Next::Dispatch,
        }
    }
}

/// The `FE_RESET_RW` user opcode handler: no `foreach` by reference over a
/// typed array.
pub(crate) struct ForeachBegin;

impl Handler for ForeachBegin {
    const OPCODE: u32 = swoole_rs_zend::sys::ZEND_FE_RESET_RW;
    fn run(frame: Frame<'_>) -> Next {
        let Some(op) = Op::current(frame) else {
            return Next::Dispatch;
        };
        if op.op1_container().and_then(Typed::of).is_none() {
            return Next::Dispatch;
        }
        throw_error(
            c"The type array do not support using references for element value during iteration",
        );
        // The instruction's failure: an undefined result, if it is used.
        if let Some(result) = op.result() {
            result.undef_iteration();
        }
        Next::Continue
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
    let outcome = parse(call.php(), definition.bytes()).and_then(|parsed| match init {
        Some(init) => from_array(call, &parsed, init),
        None => fill(call, &parsed, None),
    });
    if let Err(failure) = outcome {
        call.set_null();
        if let Failure::Bailout(bailout) = failure {
            swoole_rs_zend::bailout::defer(Err(bailout));
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
    // `made` takes the new table's reference.
    let mut made = Owned::default();
    typed_new(
        &made,
        init.map_or(0, swoole_rs_zend::Array::count),
        &definition.header(),
        IoSlice::new(definition.text()),
    );
    // The table just made, with its definition, which `made` holds and this
    // frame keeps until it returns it.
    if let Some(typed) = Typed::held(&made) {
        typed.mark();
    }
    if let Some(init) = init
        && let Some(target) = made.array_mut()
    {
        let mut entries = init.entries();
        while let Some((name, index, element)) = entries.next_stored() {
            // Marked above; borrowed only until the entry is added.
            let Some(typed) = Typed::of(target) else {
                break;
            };
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
    match parse(call.php(), definition.bytes()) {
        Ok(parsed) => call.set_bool(parsed.equals(typed)),
        Err(failure) => {
            call.set_bool(false);
            if let Failure::Bailout(bailout) = failure {
                swoole_rs_zend::bailout::defer(Err(bailout));
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

/// Replace `array_push`, `array_unshift` and `array_splice`'s handlers, at
/// MINIT (the token): PHP's function table is complete and no request has started.
pub(crate) fn install(minit: &swoole_rs_zend::Minit) {
    let handlers: [(_, swoole_rs_zend::arginfo::Handler); 3] = [
        (c"array_push", zif_swoole_array_push),
        (c"array_unshift", zif_swoole_array_unshift),
        (c"array_splice", zif_swoole_array_splice),
    ];
    let [push, unshift, splice] =
        handlers.map(|(name, handler)| minit.replace_handler(name, handler));
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
fn typed_target<'a>(call: &'a Call<'_>) -> Option<Typed<'a>> {
    // The array the by-reference argument refers to, which the builtin works
    // on in place.
    Typed::of(call.value(1)?.referred())
}

/// Check each argument after the first as an appended value.
fn check_appended(typed: Typed, call: &Call) -> bool {
    (2..=call.num_args())
        .filter_map(|n| call.value(n))
        .all(|value| check(typed, KeyType::UNDEF, value))
}

/// An override: unless the first argument names a typed array that `allowed`
/// refuses (Zend has thrown), run the `original` handler, then, with `remark`,
/// flag the table it rebuilt in place typed again. A bailout in the original
/// is trapped and deferred to the handler's end, with no table flagged.
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
    if let Some(handler) = ORIGINALS.get().map(original)
        && handler.is_some()
        && let Err(bailout) = call.forward(handler)
    {
        bailout.defer();
        return;
    }
    if remark && let Some(typed) = target {
        typed.mark();
    }
}

// Upstream's `PHP_FUNCTION` names; `install` puts them in the core functions'
// slots. Each ends by raising the bailout `guarded` deferred.
swoole_rs_zend::handlers! {
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
        assert_eq!(offset_of!(TypeInfo<'static>, ce), 16);
        assert_eq!(offset_of!(TypeInfo<'static>, len), 24);
        assert_eq!(offset_of!(TypeInfo<'static>, text), 26);
        assert_eq!(size_of::<TypeInfo<'static>>(), 32);
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
    fn a_bad_key_reports_the_rest() {
        let text = b"<float,int>\0";
        assert_eq!(split_value(text, 0, 11), Err(7));
        let nested = b"<int,<x,int>>\0";
        assert_eq!(split_value(nested, 5, 7), Err(8));
    }
}
