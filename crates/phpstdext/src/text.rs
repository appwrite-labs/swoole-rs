//! The string and array predicates, `swoole_parse_str`, and the PCRE and JSON
//! functions, whose PCRE calls are src/stdext.c's and JSON call zend-sys's.
use std::{ffi::c_void, ptr};
use swoole_rs_zend::{
    Call, handlers,
    sys::{
        swr_zend_array_init, swr_zend_emalloc, swr_zend_treat_string, swr_zend_try_json_decode_ex,
        zend_long, zend_string, zval,
    },
};

unsafe extern "C" {
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

unsafe fn match_pattern(
    regex: *mut zend_string,
    subject: *mut zend_string,
    value: *mut zval,
    global: u8,
    num_args: u32,
    flags: zend_long,
    offset: zend_long,
) -> Result<(), swoole_rs_zend::bailout::Bailout> {
    let mut compiled = ptr::null_mut();
    // SAFETY: both argument strings stay live; compilation warnings are trapped below Rust.
    swoole_rs_zend::bailout::trapped(i32::from(unsafe {
        swr_phpstdext_pcre_cache(regex, &raw mut compiled)
    }))?;
    if compiled.is_null() {
        // SAFETY: like RETVAL_FALSE, write the caller's fresh return slot type.
        unsafe {
            (*value).u1 = swoole_rs_zend::sys::_zval_struct__bindgen_ty_1 {
                type_info: swoole_rs_zend::sys::IS_FALSE,
            }
        };
        return Ok(());
    }
    // SAFETY: PHP's live cache entry is retained before any matching effect.
    unsafe { swr_phpstdext_pcre_retain(compiled) };
    let compiled = Pattern(compiled);
    // SAFETY: all-zero initializes the native function's count zval to undefined.
    let mut count = unsafe { std::mem::zeroed::<zval>() };
    // SAFETY: PHP borrows the retained pattern, subject and writable result slots
    // inside a native trap, below the Pattern owner and all caller frames.
    swoole_rs_zend::bailout::trapped(i32::from(unsafe {
        swr_phpstdext_pcre_match(
            compiled.0,
            subject,
            &raw mut count,
            value,
            global == 1,
            num_args >= 3,
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
    // SAFETY: the return slot.
    unsafe { swr_zend_array_init(call.return_value()) };
    // `estrndup`: the SAPI frees it.
    // SAFETY: Zend's request allocator.
    let copy = unsafe { swr_zend_emalloc(text.len() + 1) }.cast::<u8>();
    // SAFETY: room for the bytes and a NUL.
    unsafe { ptr::copy_nonoverlapping(text.as_ptr(), copy, text.len()) };
    // SAFETY: as above.
    unsafe { copy.wrapping_add(text.len()).write(0) };
    // SAFETY: an emalloc'd NUL-terminated copy and the array just made.
    unsafe { swr_zend_treat_string(copy.cast(), call.return_value()) };
}

fn str_is_empty(call: &Call) {
    if let Ok(Some(string)) = call.arity(1, 1).and_then(|()| call.str(1)) {
        // SAFETY: a live argument string.
        call.set_bool(unsafe { (*string).len } == 0);
    }
}

fn array_is_empty(call: &Call) {
    if let Ok(array) = call.arity(1, 1).and_then(|()| call.array_ref(1)) {
        call.set_bool(array.count() == 0);
    }
}

fn match_one(call: &Call) {
    pcre_match(call, 0);
}

fn match_all(call: &Call) {
    pcre_match(call, 1);
}

/// Upstream's `php_do_pcre_match`.
fn pcre_match(call: &Call, global: u8) {
    let parsed = call.arity(2, 4).and_then(|()| {
        Ok((
            call.str(1)?,
            call.str(2)?,
            call.long(3, 0)?,
            call.long(4, 0)?,
        ))
    });
    let Ok((Some(subject), Some(regex), flags, offset)) = parsed else {
        return;
    };
    // SAFETY: the original argument strings and return slot remain live while
    // the Rust owner retains, matches and releases PHP's real cached pattern.
    let result = unsafe {
        match_pattern(
            regex,
            subject,
            call.return_value(),
            global,
            call.num_args(),
            flags,
            offset,
        )
    };
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
            call.string(1, b"")?,
            call.long(2, DEFAULT_DEPTH)?,
            call.long(3, 0)?,
        ))
    });
    let Ok((text, depth, options)) = parsed else {
        return;
    };
    let options = if assoc {
        options | OBJECT_AS_ARRAY
    } else {
        options & !OBJECT_AS_ARRAY
    };
    // SAFETY: the argument's bytes live for the call; the return slot.
    let bailed = unsafe {
        swr_zend_try_json_decode_ex(
            call.return_value(),
            text.as_ptr().cast(),
            text.len(),
            options,
            depth,
        )
    };
    swoole_rs_zend::bailout::defer(swoole_rs_zend::bailout::trapped(bailed));
}
