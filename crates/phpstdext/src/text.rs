//! The string and array predicates, `swoole_parse_str`, and the PCRE and JSON
//! functions, whose PCRE calls are src/stdext.c's and JSON call zend-sys's.
use std::{
    ffi::{c_char, c_void},
    io::IoSlice,
    marker::PhantomData,
    ptr::NonNull,
};
use swoole_rs_zend::{
    Call, OwnedArray, ReturnSlot, UNDEF, ZStr, bailout, contain_with, handlers,
    sys::{zend_long, zval},
};

unsafe extern "C" {
    /// `estrndup`: it reads the lent bytes (an `IoSlice` is an iovec) and returns
    /// an emalloc'd NUL-terminated copy.
    safe fn swr_bytes_estrndup(bytes: IoSlice<'_>) -> Copied;
    /// The SAPI's `treat_data(PARSE_STRING, text, array)`: it parses the query
    /// string into the array, then frees the copy, which it takes. Its
    /// `max_input_vars` and nesting warnings run the error handler, which may
    /// bail out.
    safe fn swr_zend_treat_string(text: Copied, array: &mut OwnedArray);
    /// `pcre_get_compiled_regex_cache` of a live string inside `zend_try`,
    /// retaining what it returns: nonzero for a bailout, else `result` holds the
    /// retained entry, or none after the warning of a pattern that failed.
    safe fn swr_phpstdext_pcre_cache(regex: &ZStr, result: &mut Option<Pattern>) -> u8;
    /// Gives up the reference the pattern held.
    safe fn php_pcre_pce_decref(entry: Pattern);
    /// `php_pcre_match_impl` of the lent pattern and live subject inside
    /// `zend_try`, as upstream calls it: the match count goes in `count`, which
    /// it writes without reading, and the matches in `result`. Nonzero for a
    /// bailout.
    safe fn swr_phpstdext_pcre_match(
        entry: Lent<'_>,
        subject: &ZStr,
        count: &mut zval,
        result: ReturnSlot<'_>,
        global: bool,
        flags_passed: bool,
        flags: i64,
        offset: i64,
    ) -> u8;
}

/// An emalloc'd NUL-terminated copy, which only `swr_bytes_estrndup` makes.
#[repr(transparent)]
struct Copied(*mut c_char);

/// A retained PHP PCRE cache entry, which only `swr_phpstdext_pcre_cache`
/// makes, holding one reference until `php_pcre_pce_decref` takes it: it is
/// neither `Copy` nor `Clone`, so the reference goes once. Release is native
/// refcount/memory work and cannot run PHP code.
#[repr(transparent)]
struct Pattern(NonNull<c_void>);

/// A [`Pattern`]'s entry, lent while the pattern holds its reference.
#[derive(Clone, Copy)]
#[repr(transparent)]
struct Lent<'a>(NonNull<c_void>, PhantomData<&'a Pattern>);

impl Pattern {
    fn lend(&self) -> Lent<'_> {
        Lent(self.0, PhantomData)
    }
}

/// Upstream's `php_do_pcre_match` once its arguments parsed.
fn match_pattern(
    call: &Call,
    regex: &ZStr,
    subject: &ZStr,
    global: bool,
    flags: zend_long,
    offset: zend_long,
) -> Result<(), swoole_rs_zend::bailout::Bailout> {
    let mut compiled = None;
    // Compilation warnings are trapped below Rust.
    swoole_rs_zend::bailout::trapped(i32::from(swr_phpstdext_pcre_cache(regex, &mut compiled)))?;
    let Some(compiled) = compiled else {
        call.set_bool(false);
        return Ok(());
    };
    let mut count = UNDEF;
    let bailed = swr_phpstdext_pcre_match(
        compiled.lend(),
        subject,
        &mut count,
        call.return_slot(),
        global,
        call.num_args() >= 3,
        flags,
        offset,
    );
    // The trap returned, bailout or not: the reference goes either way.
    php_pcre_pce_decref(compiled);
    swoole_rs_zend::bailout::trapped(i32::from(bailed))
}

/// `PHP_JSON_OBJECT_AS_ARRAY`.
const OBJECT_AS_ARRAY: zend_long = 1;
/// `PHP_JSON_PARSER_DEFAULT_DEPTH`.
const DEFAULT_DEPTH: zend_long = 512;

handlers! {
    zif_swoole_parse_str => parse_str;
    zif_swoole_str_is_empty => str_is_empty;
    zif_swoole_array_is_empty => array_is_empty;
    zif_swoole_str_match => match_one;
    zif_swoole_str_match_all => match_all;
    zif_swoole_str_json_decode => json_decode_assoc;
    zif_swoole_str_json_decode_to_object => json_decode_object;
}

fn parse_str(call: &Call) {
    let Ok(text) = call.arity(1, 1).and_then(|()| call.string(1, b"")) else {
        return;
    };
    let mut array = OwnedArray::default();
    bailout::defer(contain_with(|| {
        swr_zend_treat_string(swr_bytes_estrndup(IoSlice::new(text)), &mut array);
    }));
    call.set_owned(array.into_owned());
}

fn str_is_empty(call: &Call) {
    if let Ok(Some(string)) = call.arity(1, 1).and_then(|()| call.zstr(1)) {
        call.set_bool(string.bytes().is_empty());
    }
}

fn array_is_empty(call: &Call) {
    if let Ok(array) = call.arity(1, 1).and_then(|()| call.array_ref(1)) {
        call.set_bool(array.count() == 0);
    }
}

fn match_one(call: &Call) {
    pcre_match(call, false);
}

fn match_all(call: &Call) {
    pcre_match(call, true);
}

/// Upstream's `php_do_pcre_match`.
fn pcre_match(call: &Call, global: bool) {
    let parsed = call.arity(2, 4).and_then(|()| {
        Ok((
            call.zstr(1)?,
            call.zstr(2)?,
            call.long(3, 0)?,
            call.long(4, 0)?,
        ))
    });
    let Ok((Some(subject), Some(regex), flags, offset)) = parsed else {
        return;
    };
    let result = match_pattern(call, regex, subject, global, flags, offset);
    swoole_rs_zend::bailout::defer(result);
}

fn json_decode_assoc(call: &Call) {
    json_decode(call, true);
}

fn json_decode_object(call: &Call) {
    json_decode(call, false);
}

/// Upstream's `php_do_json_decode`.
fn json_decode(call: &Call, assoc: bool) {
    let parsed = call.arity(1, 3).and_then(|()| {
        Ok((
            call.zstr(1)?,
            call.long(2, DEFAULT_DEPTH)?,
            call.long(3, 0)?,
        ))
    });
    let Ok((Some(text), depth, options)) = parsed else {
        return;
    };
    let options = if assoc {
        options | OBJECT_AS_ARRAY
    } else {
        options & !OBJECT_AS_ARRAY
    };
    swoole_rs_zend::bailout::defer(call.json_decode(text, 0..usize::MAX, options, depth));
}
