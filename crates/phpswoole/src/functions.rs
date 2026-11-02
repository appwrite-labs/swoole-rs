//! The global functions of `php_swoole.cc`, each as upstream parses and answers.
use crate::{constants::VERSION, ops};
use std::{
    ffi::{CStr, c_char, c_int},
    ptr,
};
use swoole_rs_zend::{
    Call, E_WARNING, Invalid, handlers, php_error,
    string::val,
    sys::{
        _zend_expected_type_Z_EXPECTED_BOOL_OR_NULL, IS_FALSE, IS_NULL, IS_TRUE,
        ZPP_ERROR_WRONG_ARG, zend_hash_func, zend_parse_arg_bool_slow, zend_string,
        zend_wrong_parameter_error,
    },
    type_of, value,
};

handlers! {
    zif_swoole_version => version;
    zif_swoole_cpu_num => cpu_num;
    zif_swoole_last_error => last_error;
    zif_swoole_clear_error => clear_error;
    zif_swoole_errno => errno;
    zif_swoole_strerror => strerror;
    zif_swoole_hashcode => hashcode;
    zif_swoole_error_log => error_log;
    zif_swoole_error_log_ex => error_log_ex;
    zif_swoole_ignore_error => ignore_error;
    zif_swoole_mime_type_add => mime_type_add;
    zif_swoole_mime_type_set => mime_type_set;
    zif_swoole_mime_type_delete => mime_type_delete;
    zif_swoole_mime_type_get => mime_type_get;
    zif_swoole_mime_type_exists => mime_type_exists;
    zif_swoole_mime_type_list => mime_type_list;
    zif_swoole_internal_call_user_shutdown_begin => shutdown_begin;
    zif_swoole_substr_unserialize => substr_unserialize;
    zif_swoole_substr_json_decode => substr_json_decode;
}

/// `SW_STRERROR_*`: which table `swoole_strerror()` reads.
const STRERROR_GAI: i64 = 1;
const STRERROR_DNS: i64 = 2;
const STRERROR_SWOOLE: i64 = 9;
/// `SW_ERROR_BEGIN` and `SW_ERROR_END`.
const ERROR_BEGIN: i64 = 500;
const ERROR_END: i64 = 10020;
/// `SW_ERROR_MSG_SIZE`: `swoole_error_log` formats into a buffer this big.
const ERROR_MSG_SIZE: usize = 16384;
/// `PHP_JSON_PARSER_DEFAULT_DEPTH` and `PHP_JSON_OBJECT_AS_ARRAY`.
const JSON_DEFAULT_DEPTH: i64 = 512;
const JSON_OBJECT_AS_ARRAY: i64 = 1;

unsafe extern "C" {
    /// `<netdb.h>`; not in the libc crate.
    fn hstrerror(error: c_int) -> *const c_char;
}

/// `ZEND_PARSE_PARAMETERS_END_EX(RETURN_FALSE)`.
fn or_false<T>(call: &Call, parsed: Result<T, Invalid>) -> Option<T> {
    parsed.map_err(|Invalid| call.set_bool(false)).ok()
}

/// `RETURN_STRING` of a C string.
/// # Safety
/// `text` is a live NUL-terminated string.
unsafe fn set_c_string(call: &Call, text: *const c_char) {
    // SAFETY: per the caller.
    call.set_bytes(unsafe { CStr::from_ptr(text) }.to_bytes());
}

/// `ZSTR_VAL`.
fn text(string: *mut zend_string) -> *const c_char {
    val(string).cast_const().cast()
}

/// `zend_long` to the `int` upstream passes on, as C converts it.
#[expect(clippy::cast_possible_truncation, reason = "C's (int) conversion")]
fn int(value: i64) -> i32 {
    value as i32
}

fn version(call: &Call) {
    call.set_bytes(VERSION.to_bytes());
}

fn cpu_num(call: &Call) {
    // SAFETY: C++'s getter.
    call.set_long(i64::from(unsafe { (ops().cpu_num)() }));
}

fn last_error(call: &Call) {
    // SAFETY: C++'s getter.
    call.set_long(i64::from(unsafe { (ops().last_error)() }));
}

fn clear_error(_: &Call) {
    // SAFETY: C++'s setter.
    unsafe { (ops().set_last_error)(0) };
}

fn errno(call: &Call) {
    call.set_long(i64::from(
        std::io::Error::last_os_error().raw_os_error().unwrap_or(0),
    ));
}

extern "C" fn system_error(code: i32) -> *const c_char {
    // SAFETY: libc's message table.
    unsafe { libc::strerror(code) }
}

fn strerror(call: &Call) {
    let parsed = call
        .arity(1, 2)
        .and_then(|()| Ok((call.long(1, 0)?, call.long(2, 0)?)));
    let Some((code, kind)) = or_false(call, parsed) else {
        return;
    };
    let message = if kind == STRERROR_GAI {
        // SAFETY: libc's message table.
        unsafe { libc::gai_strerror(int(code)) }
    } else if kind == STRERROR_DNS {
        // SAFETY: as above.
        unsafe { hstrerror(int(code)) }
    } else if kind == STRERROR_SWOOLE || (code > ERROR_BEGIN && code < ERROR_END) {
        // SAFETY: Swoole's `swoole_strerror`, with the system fallback C++ passes.
        unsafe { swoole_rs_error::swr_v1_error_strerror(int(code), Some(system_error)) }
    } else {
        system_error(int(code))
    };
    // SAFETY: each table returns a NUL-terminated message.
    unsafe { set_c_string(call, message) };
}

/// Bob Jenkins' one-at-a-time hash over C `char`s (sign-extended where `char`
/// is signed), as upstream's `hashkit_one_at_a_time`.
fn one_at_a_time(key: &[u8]) -> u32 {
    let mut value: u32 = 0;
    for &byte in key {
        // C's (uint32_t) of a plain char.
        let byte = if c_char::MIN == 0 {
            u32::from(byte)
        } else {
            i32::from(byte.cast_signed()).cast_unsigned()
        };
        value = value.wrapping_add(byte);
        value = value.wrapping_add(value << 10);
        value ^= value >> 6;
    }
    value = value.wrapping_add(value << 3);
    value ^= value >> 11;
    value.wrapping_add(value << 15)
}

fn hashcode(call: &Call) {
    let parsed = call
        .arity(1, 2)
        .and_then(|()| Ok((call.string(1, b"")?, call.long(2, 0)?)));
    let Some((data, kind)) = or_false(call, parsed) else {
        return;
    };
    match kind {
        0 => {
            // SAFETY: a live string argument.
            let hash = unsafe { zend_hash_func(data.as_ptr().cast(), data.len()) };
            #[expect(clippy::cast_possible_wrap, reason = "RETURN_LONG of a zend_ulong")]
            call.set_long(hash as i64);
        }
        1 => call.set_long(i64::from(one_at_a_time(data))),
        _ => call.set_bool(false),
    }
}

fn error_log(call: &Call) {
    let parsed = call
        .arity(2, 2)
        .and_then(|()| Ok((call.long(1, 0)?, call.string(2, b"")?)));
    if let Some((level, message)) = or_false(call, parsed) {
        // SAFETY: C++'s logger, with the argument's bytes.
        unsafe { (ops().log_put)(int(level), message.as_ptr().cast(), message.len()) };
    }
}

/// `swoole_error_log(level, error, "%.*s", msg)` from `zif_swoole_error_log_ex`:
/// `"<function>() (ERRNO <error>): <msg up to a NUL>"`, cut to the buffer.
fn error_log_message(error: i32, message: &[u8]) -> Vec<u8> {
    let text = message.split(|&byte| byte == 0).next().unwrap_or_default();
    let mut out = format!("zif_swoole_error_log_ex() (ERRNO {error}): ").into_bytes();
    out.extend_from_slice(text);
    out.truncate(ERROR_MSG_SIZE - 1);
    out
}

fn error_log_ex(call: &Call) {
    let parsed = call
        .arity(3, 3)
        .and_then(|()| Ok((call.long(1, 0)?, call.long(2, 0)?, call.string(3, b"")?)));
    if let Some((level, error, message)) = or_false(call, parsed) {
        let message = error_log_message(int(error), message);
        // SAFETY: C++'s logger, with a live message.
        unsafe {
            (ops().error_log)(
                int(level),
                int(error),
                message.as_ptr().cast(),
                message.len(),
            );
        }
    }
}

fn ignore_error(call: &Call) {
    let parsed = call.arity(1, 1).and_then(|()| call.long(1, 0));
    if let Some(error) = or_false(call, parsed) {
        // SAFETY: C++'s ignore list.
        unsafe { (ops().ignore_error)(int(error)) };
    }
}

/// `Z_PARAM_STR` of each of the first `N` arguments, all required.
fn strings<const N: usize>(call: &Call) -> Option<[*mut zend_string; N]> {
    let mut out = [ptr::null_mut(); N];
    #[expect(clippy::cast_possible_truncation, reason = "one or two arguments")]
    let count = N as u32;
    let parsed = call.arity(count, count).and_then(|()| {
        for (index, slot) in (1..).zip(out.iter_mut()) {
            *slot = call.str(index)?.unwrap_or(ptr::null_mut());
        }
        Ok(out)
    });
    or_false(call, parsed)
}

fn mime_type_add(call: &Call) {
    if let Some([suffix, kind]) = strings(call) {
        // SAFETY: C++'s table, with live argument strings.
        call.set_bool(unsafe { (ops().mime_add)(text(suffix), text(kind)) } != 0);
    }
}

fn mime_type_set(call: &Call) {
    if let Some([suffix, kind]) = strings(call) {
        // SAFETY: as above.
        unsafe { (ops().mime_set)(text(suffix), text(kind)) };
    }
}

fn mime_type_delete(call: &Call) {
    if let Some([suffix]) = strings(call) {
        // SAFETY: as above.
        call.set_bool(unsafe { (ops().mime_del)(text(suffix)) } != 0);
    }
}

fn mime_type_get(call: &Call) {
    if let Some([filename]) = strings(call) {
        // SAFETY: C++'s table, with a live argument string.
        let kind = unsafe { (ops().mime_get)(text(filename)) };
        // SAFETY: the table's entry outlives the copy.
        unsafe { set_c_string(call, kind) };
    }
}

fn mime_type_exists(call: &Call) {
    if let Some([filename]) = strings(call) {
        // SAFETY: as above.
        call.set_bool(unsafe { (ops().mime_exists)(text(filename)) } != 0);
    }
}

fn mime_type_list(call: &Call) {
    call.set_array();
    // SAFETY: the return slot holds an array.
    unsafe { (ops().mime_list)(call.return_value()) };
}

fn shutdown_begin(call: &Call) {
    // SAFETY: C++'s module globals.
    if unsafe { (ops().shutdown_begin)() } != 0 {
        call.set_bool(true);
    } else {
        // SAFETY: a handler, with a literal message.
        unsafe { php_error(E_WARNING, c"can not call this function in user level") };
        call.set_bool(false);
    }
}

/// The `[offset, offset + length)` window of a `size`-byte string that
/// `swoole_substr_*` decode, or why there is none.
#[derive(Debug, PartialEq, Eq)]
enum Window {
    Empty,
    Before,
    Past,
    At(usize, usize),
}

fn window(size: usize, offset: i64, length: i64) -> Window {
    let size = i64::try_from(size).unwrap_or(i64::MAX);
    if size == 0 {
        return Window::Empty;
    }
    let offset = if offset < 0 { size + offset } else { offset };
    if offset < 0 {
        return Window::Before;
    }
    if size <= offset {
        return Window::Past;
    }
    let length = if length <= 0 || length > size - offset {
        size - offset
    } else {
        length
    };
    // Both are checked non-negative above.
    Window::At(
        usize::try_from(offset).unwrap_or(0),
        usize::try_from(length).unwrap_or(0),
    )
}

fn substr_unserialize(call: &Call) {
    let parsed = call.arity(2, 4).and_then(|()| {
        Ok((
            call.string(1, b"")?,
            call.long(2, 0)?,
            call.long(3, 0)?,
            call.array_opt(4)?,
        ))
    });
    let Ok((buf, offset, length, options)) = parsed else {
        return;
    };
    let Window::At(offset, length) = window(buf.len(), offset, length) else {
        call.set_bool(false);
        return;
    };
    // SAFETY: an array argument.
    let options = options.map_or(ptr::null_mut(), |array| unsafe { value::table(array) });
    // SAFETY: C++'s decoder over the argument's live bytes.
    unsafe {
        (ops().unserialize)(
            call.return_value(),
            buf[offset..].as_ptr().cast(),
            length,
            options,
        );
    }
}

/// `Z_PARAM_BOOL_EX(dest, is_null, 1, 0)` of argument `n`: `None` when null or
/// not passed.
fn bool_or_null(call: &Call, n: u32) -> Result<Option<bool>, Invalid> {
    let Some(arg) = call.passed(n) else {
        return Ok(None);
    };
    // SAFETY: a passed, live argument.
    match unsafe { type_of(arg) } {
        IS_TRUE => return Ok(Some(true)),
        IS_FALSE => return Ok(Some(false)),
        IS_NULL => return Ok(None),
        _ => {}
    }
    let mut out = false;
    // SAFETY: a live argument and a writable local.
    if unsafe { zend_parse_arg_bool_slow(arg, &raw mut out, n) } {
        return Ok(Some(out));
    }
    #[expect(clippy::cast_possible_wrap, reason = "ZPP error codes are small")]
    // SAFETY: as Zend's macro reports it, with the argument that failed.
    unsafe {
        zend_wrong_parameter_error(
            ZPP_ERROR_WRONG_ARG as i32,
            n,
            ptr::null_mut(),
            _zend_expected_type_Z_EXPECTED_BOOL_OR_NULL,
            arg,
        );
    }
    Err(Invalid)
}

fn substr_json_decode(call: &Call) {
    let parsed = call.arity(2, 6).and_then(|()| {
        Ok((
            call.string(1, b"")?,
            call.long(2, 0)?,
            call.long(3, 0)?,
            bool_or_null(call, 4)?,
            call.long(5, JSON_DEFAULT_DEPTH)?,
            call.long(6, 0)?,
        ))
    });
    let Ok((text, offset, length, assoc, depth, options)) = parsed else {
        return;
    };
    let warning = match window(text.len(), offset, length) {
        Window::At(offset, length) => {
            // For BC, the bool $assoc overrides the options' PHP_JSON_OBJECT_AS_ARRAY.
            let options = match assoc {
                Some(true) => options | JSON_OBJECT_AS_ARRAY,
                Some(false) => options & !JSON_OBJECT_AS_ARRAY,
                None => options,
            };
            // SAFETY: C++'s decoder over the argument's live bytes.
            unsafe {
                (ops().json_decode)(
                    call.return_value(),
                    text[offset..].as_ptr().cast(),
                    length,
                    options,
                    depth,
                );
            }
            return;
        }
        Window::Empty => c"Non-empty string required",
        Window::Before => c"Offset must be not less than the negative length of the string",
        Window::Past => c"Offset must be less than the length of the string",
    };
    // SAFETY: a handler, with a literal message.
    unsafe { php_error(E_WARNING, warning) };
    call.set_null();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_at_a_time_matches_hashkit() {
        assert_eq!(one_at_a_time(b""), 0);
        // Reference values of libmemcached's hashkit_one_at_a_time.
        assert_eq!(one_at_a_time(b"a"), 0xca2e_9442);
        assert_eq!(one_at_a_time(b"hello"), 0xc8fd_181b);
        if c_char::MIN != 0 {
            // Signed `char` sign-extends high bytes, as the C loop does.
            assert_eq!(one_at_a_time(b"\xff\x80z"), 0xa7e6_bb0a);
        }
    }

    #[test]
    fn windows_follow_upstream() {
        assert_eq!(window(0, 0, 0), Window::Empty);
        assert_eq!(window(5, -6, 0), Window::Before);
        assert_eq!(window(5, 5, 0), Window::Past);
        assert_eq!(window(5, -2, 0), Window::At(3, 2));
        assert_eq!(window(5, 1, 2), Window::At(1, 2));
        assert_eq!(window(5, 1, 9), Window::At(1, 4));
        assert_eq!(window(5, 1, -1), Window::At(1, 4));
    }

    #[test]
    fn error_log_ex_formats_like_swoole_error_log() {
        assert_eq!(
            error_log_message(888_888, b"hello\0hidden"),
            b"zif_swoole_error_log_ex() (ERRNO 888888): hello"
        );
        assert_eq!(
            error_log_message(1, &[b'x'; ERROR_MSG_SIZE]).len(),
            ERROR_MSG_SIZE - 1
        );
    }
}
