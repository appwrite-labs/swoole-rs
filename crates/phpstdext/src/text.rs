//! The string and array predicates, `swoole_parse_str`, and the PCRE and JSON
//! functions, whose PCRE calls are src/stdext.c's and JSON call zend-sys's.
use std::{
    ffi::{c_char, c_void},
    io::IoSlice,
    ptr,
};
use swoole_rs_zend::{
    Call, UNDEF, ZStr, handlers,
    sys::{swr_zend_treat_string, zend_long, zend_string, zval},
};

unsafe extern "C" {
    /// `estrndup`: it reads the lent bytes (an `IoSlice` is an iovec) and returns
    /// an emalloc'd NUL-terminated copy.
    safe fn swr_bytes_estrndup(bytes: IoSlice<'_>) -> *mut c_char;
    fn swr_phpstdext_pcre_cache(regex: *mut zend_string, result: *mut *mut c_void) -> u8;
    fn swr_phpstdext_pcre_retain(entry: *mut c_void);
    fn swr_phpstdext_pcre_release(entry: *mut c_void);
    fn swr_phpstdext_pcre_match(
        entry: *mut c_void,
        subject: *mut zend_string,
        count: *mut zval,
        result: *mut zval,
        global: bool,
        flags_passed: bool,
        flags: i64,
        offset: i64,
    ) -> u8;
}

/// A retained PHP PCRE cache entry. Release is native refcount/memory work and
/// cannot run PHP code, so a contained matching bailout still drops this owner.
struct Pattern(*mut c_void);
impl Drop for Pattern {
    fn drop(&mut self) {
        // SAFETY: this owner retains exactly one live compiled-pattern reference.
        unsafe { swr_phpstdext_pcre_release(self.0) };
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
    let mut compiled = ptr::null_mut();
    // SAFETY: both argument strings stay live; compilation warnings are trapped below Rust.
    swoole_rs_zend::bailout::trapped(i32::from(unsafe {
        swr_phpstdext_pcre_cache(regex.as_ptr(), &raw mut compiled)
    }))?;
    if compiled.is_null() {
        call.set_bool(false);
        return Ok(());
    }
    // SAFETY: PHP's live cache entry is retained before any matching effect.
    unsafe { swr_phpstdext_pcre_retain(compiled) };
    let compiled = Pattern(compiled);
    let mut count = UNDEF;
    // SAFETY: PHP borrows the retained pattern, subject and writable result slots
    // inside a native trap, below the Pattern owner and all caller frames.
    swoole_rs_zend::bailout::trapped(i32::from(unsafe {
        swr_phpstdext_pcre_match(
            compiled.0,
            subject.as_ptr(),
            &raw mut count,
            call.return_value(),
            global,
            call.num_args() >= 3,
            flags,
            offset,
        )
    }))
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
    call.set_array();
    // `estrndup`: the SAPI frees it.
    let copy = swr_bytes_estrndup(IoSlice::new(text));
    // SAFETY: an emalloc'd NUL-terminated copy and the array just made.
    unsafe { swr_zend_treat_string(copy, call.return_value()) };
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
