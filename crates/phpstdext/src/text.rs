//! The string and array predicates, `swoole_parse_str`, and the PCRE and JSON
//! functions, whose library calls C++ lends as [`Ops`].
use std::{
    ffi::c_char,
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
};
use swoole_rs_zend::{
    Call, handlers,
    sys::{
        swr_zend_array_init, swr_zend_emalloc, swr_zend_treat_string, zend_array_count, zend_long,
        zend_string, zval,
    },
    value::table,
};

/// The PHP library calls C++ makes for stdext (`swr_v1_phpstdext_minit`).
#[repr(C)]
pub struct Ops {
    /// Swoole's `zend::json_decode`.
    json_decode: unsafe extern "C" fn(
        value: *mut zval,
        text: *const c_char,
        length: usize,
        options: zend_long,
        depth: zend_long,
    ),
    /// `php_pcre_match_impl` of a cached compiled `regex` (`RETVAL_FALSE` if it
    /// does not compile); `flags` counts as passed when `num_args >= 3`.
    pcre_match: unsafe extern "C" fn(
        regex: *mut zend_string,
        subject: *mut zend_string,
        value: *mut zval,
        global: u8,
        num_args: u32,
        flags: zend_long,
        offset: zend_long,
    ),
}

static OPS: AtomicPtr<Ops> = AtomicPtr::new(ptr::null_mut());

/// # Safety
/// At MINIT; `ops` lives for the process.
pub(crate) unsafe fn install(ops: *const Ops) {
    OPS.store(ops.cast_mut(), Ordering::Relaxed);
}

fn ops() -> &'static Ops {
    // SAFETY: set at MINIT, before any PHP code runs, and lives for the process.
    unsafe { &*OPS.load(Ordering::Relaxed) }
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
    if let Ok(array) = call.arity(1, 1).and_then(|()| call.array(1)) {
        // SAFETY: a live array argument.
        // SAFETY: as above.
        let ht = unsafe { table(array) };
        // SAFETY: a live table.
        call.set_bool(unsafe { zend_array_count(ht) } == 0);
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
    // SAFETY: live argument strings and the return slot.
    unsafe {
        (ops().pcre_match)(
            regex,
            subject,
            call.return_value(),
            global,
            call.num_args(),
            flags,
            offset,
        );
    }
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
    unsafe {
        (ops().json_decode)(
            call.return_value(),
            text.as_ptr().cast(),
            text.len(),
            options,
            depth,
        );
    }
}
