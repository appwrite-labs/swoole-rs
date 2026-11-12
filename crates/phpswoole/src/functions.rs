//! The global functions of `php_swoole.cc`, each as upstream parses and answers.
use crate::{
    constants::VERSION,
    log_put,
    module::{CALL_USER_SHUTDOWNFUNC_BEGIN, RINIT_END},
};
use std::{
    ffi::{CStr, c_char, c_int},
    ptr,
};
use swoole_rs_corebase::log::{last_error as swoole_last_error, set_last_error};
use swoole_rs_framing::mime;
use swoole_rs_zend::{
    Array, Call, E_WARNING, Invalid, OwnedArray, ZStr, handlers, report,
    sys::{swr_zend_try_json_decode_ex, zend_array, zval},
};

unsafe extern "C" {
    /// PHP's `unserialize()`, as Swoole's `zend::unserialize` called it.
    fn php_unserialize_with_options(
        value: *mut zval,
        buf: *const c_char,
        length: usize,
        options: *mut zend_array,
        function: *const c_char,
    );
}

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
    /// `<netdb.h>`; not in the libc crate. Any code names a static message.
    safe fn hstrerror(error: c_int) -> *const c_char;
}

/// `ZEND_PARSE_PARAMETERS_END_EX(RETURN_FALSE)`.
fn or_false<T>(call: &Call, parsed: Result<T, Invalid>) -> Option<T> {
    parsed.map_err(|Invalid| call.set_bool(false)).ok()
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
    call.set_long(i64::from(swoole_rs_corebase::swr_v1_core_cpu_num()));
}

fn last_error(call: &Call) {
    call.set_long(i64::from(swoole_last_error()));
}

fn clear_error(_: &Call) {
    set_last_error(0);
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
        hstrerror(int(code))
    } else if kind == STRERROR_SWOOLE || (code > ERROR_BEGIN && code < ERROR_END) {
        // Swoole's `swoole_strerror`, with the system fallback C++ passes.
        swoole_rs_error::strerror_with(int(code), Some(system_error))
    } else {
        system_error(int(code))
    };
    // SAFETY: each table returns a NUL-terminated message.
    call.set_bytes(unsafe { CStr::from_ptr(message) }.to_bytes());
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
            let hash = swoole_rs_zend::string::hash(data, data.len());
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
        log_put(int(level), message);
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
        let (level, error) = (int(level), int(error));
        set_last_error(error);
        if level >= swoole_rs_utilities::swoole_get_log_level()
            && !swoole_rs_error_ffi::swoole_is_ignored_error(error)
        {
            log_put(level, &error_log_message(error, message));
        }
    }
}

fn ignore_error(call: &Call) {
    let parsed = call.arity(1, 1).and_then(|()| call.long(1, 0));
    if let Some(error) = or_false(call, parsed) {
        swoole_rs_error_ffi::swoole_ignore_error(int(error));
    }
}

/// `Z_PARAM_STR` of each of the first `N` arguments, all required, as
/// upstream passes them on: up to the first NUL.
fn strings<const N: usize>(call: &Call) -> Option<[&CStr; N]> {
    let mut out = [c""; N];
    #[expect(clippy::cast_possible_truncation, reason = "one or two arguments")]
    let count = N as u32;
    let parsed = call.arity(count, count).and_then(|()| {
        for (index, slot) in (1..).zip(out.iter_mut()) {
            *slot = call.zstr(index)?.map_or(c"", ZStr::c_str);
        }
        Ok(out)
    });
    or_false(call, parsed)
}

fn mime_type_add(call: &Call) {
    if let Some([suffix, kind]) = strings(call) {
        call.set_bool(mime::add(suffix.to_bytes(), kind));
    }
}

fn mime_type_set(call: &Call) {
    if let Some([suffix, kind]) = strings(call) {
        mime::set(suffix.to_bytes(), kind);
    }
}

fn mime_type_delete(call: &Call) {
    if let Some([suffix]) = strings(call) {
        call.set_bool(mime::del(suffix.to_bytes()));
    }
}

/// `mime_type::get(path)`.
#[must_use]
pub fn mime_type(path: &[u8]) -> Vec<u8> {
    mime::get(path)
}

fn mime_type_get(call: &Call) {
    if let Some([filename]) = strings(call) {
        call.set_bytes(&mime_type(filename.to_bytes()));
    }
}

fn mime_type_exists(call: &Call) {
    if let Some([filename]) = strings(call) {
        call.set_bool(mime::exists(filename.to_bytes()));
    }
}

fn mime_type_list(call: &Call) {
    let mut array = OwnedArray::default();
    mime::each(|kind| array.push_bytes(kind.to_bytes()));
    call.set_owned(array.into_owned());
}

fn shutdown_begin(call: &Call) {
    let status = &crate::module::globals().req_status;
    if status.get() == RINIT_END {
        status.set(CALL_USER_SHUTDOWNFUNC_BEGIN);
        call.set_bool(true);
    } else {
        swoole_rs_zend::bailout::defer(report(
            E_WARNING,
            c"can not call this function in user level",
        ));
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
            (call.num_args() >= 4)
                .then(|| call.array_ref(4))
                .transpose()?,
        ))
    });
    let Ok((buf, offset, length, options)) = parsed else {
        return;
    };
    let Window::At(offset, length) = window(buf.len(), offset, length) else {
        call.set_bool(false);
        return;
    };
    let options = options.map_or(ptr::null_mut(), Array::as_ptr);
    // SAFETY: PHP's decoder over the argument's live bytes.
    unsafe {
        php_unserialize_with_options(
            call.return_value(),
            buf.as_ptr().wrapping_add(offset).cast(),
            length,
            options,
            c"swoole_ext_unserialize".as_ptr(),
        );
    }
}

fn substr_json_decode(call: &Call) {
    let parsed = call.arity(2, 6).and_then(|()| {
        Ok((
            call.string(1, b"")?,
            call.long(2, 0)?,
            call.long(3, 0)?,
            call.bool_or_null(4)?,
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
            // SAFETY: PHP's decoder over the argument's live bytes, into the return slot.
            let bailed = unsafe {
                swr_zend_try_json_decode_ex(
                    call.return_value(),
                    text.as_ptr().wrapping_add(offset).cast(),
                    length,
                    options,
                    depth,
                )
            };
            swoole_rs_zend::bailout::defer(swoole_rs_zend::bailout::trapped(bailed));
            return;
        }
        Window::Empty => c"Non-empty string required",
        Window::Before => c"Offset must be not less than the negative length of the string",
        Window::Past => c"Offset must be less than the length of the string",
    };
    swoole_rs_zend::bailout::defer(report(E_WARNING, warning));
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
