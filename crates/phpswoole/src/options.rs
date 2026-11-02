//! `php_swoole_set_global_option`: the process-wide settings that
//! `swoole_async_set()`, `Co::set()` and `Process\Pool::set()` take, applied in
//! upstream's order. And `sw_zval_is_serializable`.
use crate::{constants::DEBUG, ops};
use std::{
    ffi::{CStr, c_char},
    ptr,
    sync::atomic::Ordering,
};
use swoole_rs_zend::{
    each_value,
    string::bytes,
    sys::{
        IS_ARRAY, IS_NULL, IS_OBJECT, IS_REFERENCE, IS_STRING, ZEND_ACC_NOT_SERIALIZABLE,
        swr_zend_string_release, swr_zval_get_double, swr_zval_get_long, swr_zval_get_string,
        swr_zval_is_true, zend_array, zend_hash_str_find, zend_ini_parse_quantity, zend_string,
        zval,
    },
    type_of, value,
};

/// `swHttp2SettingId`.
const HTTP2_HEADER_TABLE_SIZE: i32 = 1;
const HTTP2_ENABLE_PUSH: i32 = 2;
const HTTP2_MAX_CONCURRENT_STREAMS: i32 = 3;
const HTTP2_INIT_WINDOW_SIZE: i32 = 4;
const HTTP2_MAX_FRAME_SIZE: i32 = 5;
const HTTP2_MAX_HEADER_LIST_SIZE: i32 = 6;
/// `swr_v1_phpswoole_ops.set_socket_timeout` kinds.
const DNS: i32 = 0;
const CONNECT: i32 = 1;
const READ: i32 = 2;
const WRITE: i32 = 3;

/// `php_swoole_array_get_value`: the value under `key`, unless absent or null.
fn get(options: *mut zend_array, key: &CStr) -> Option<*mut zval> {
    // SAFETY: a live table; Zend returns a slot or null.
    let found = unsafe { zend_hash_str_find(options, key.as_ptr(), key.count_bytes()) };
    // SAFETY: a found slot is a live zval.
    (!found.is_null() && unsafe { type_of(found) } != IS_NULL).then_some(found)
}

fn truthy(value: *mut zval) -> u8 {
    // SAFETY: a live zval; Zend's own conversion.
    u8::from(unsafe { swr_zval_is_true(value) })
}

fn long(value: *mut zval) -> i64 {
    // SAFETY: as above.
    unsafe { swr_zval_get_long(value) }
}

/// `timeout_format`: non-positive or oversized timeouts mean `INT_MAX` seconds.
fn timeout(value: *mut zval) -> f64 {
    // SAFETY: as above.
    clamp(unsafe { swr_zval_get_double(value) })
}

fn clamp(timeout: f64) -> f64 {
    if timeout <= 0.0 || timeout > f64::from(i32::MAX) {
        f64::from(i32::MAX)
    } else {
        timeout
    }
}

/// `php_swoole_parse_to_size`, or the error string to report fatally.
fn size(value: *mut zval) -> Result<i64, *mut zend_string> {
    // SAFETY: a live zval.
    if unsafe { type_of(value) } != IS_STRING {
        return Ok(long(value));
    }
    let mut error = ptr::null_mut();
    // SAFETY: a string zval holds a live string.
    let string = unsafe { value::str_of(value) };
    // SAFETY: a live string; Zend writes an error string or leaves null.
    let size = unsafe { zend_ini_parse_quantity(string, &raw mut error) };
    if error.is_null() {
        Ok(size)
    } else {
        Err(error)
    }
}

/// The option under `key`, if set, as `convert` reads it, passed to `set`.
fn option<T>(
    options: *mut zend_array,
    key: &CStr,
    convert: impl FnOnce(*mut zval) -> T,
    set: impl FnOnce(T),
) {
    if let Some(value) = get(options, key) {
        set(convert(value));
    }
}

/// The option under `key`, if set, as a string (`zend::String`) for C++'s `set`.
fn text_option(
    options: *mut zend_array,
    key: &CStr,
    set: unsafe extern "C" fn(*const c_char, usize),
) {
    let Some(value) = get(options, key) else {
        return;
    };
    // SAFETY: a live zval; a new reference released below.
    let string = unsafe { swr_zval_get_string(value) };
    // SAFETY: a live string, NUL-terminated after its bytes.
    let text = unsafe { bytes(string) };
    // SAFETY: C++'s setter, with a string that lives for the call.
    unsafe { set(text.as_ptr().cast(), text.len()) };
    // SAFETY: the reference taken above.
    unsafe { swr_zend_string_release(string) };
}

/// `php_swoole_set_global_option(options)`. A size that does not parse stops
/// here: its zval is written to `failed` and its error string returned, for
/// C++ to raise the fatal error once this frame is gone. Null otherwise.
/// # Safety
/// After MINIT, with a live options table and a writable `failed`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpswoole_set_global_option(
    options: *mut zend_array,
    failed: *mut *mut zval,
) -> *mut zend_string {
    match apply(options) {
        Ok(()) => ptr::null_mut(),
        Err((value, error)) => {
            // SAFETY: per the caller.
            unsafe { failed.write(value) };
            error
        }
    }
}

fn apply(options: *mut zend_array) -> Result<(), (*mut zval, *mut zend_string)> {
    let ops = ops();
    let size_of = |value| size(value).map_err(|error| (value, error));
    // Every setter below is C++'s, taking a plain value.
    let flag = |key, set: unsafe extern "C" fn(u8)| {
        // SAFETY: as above.
        option(options, key, truthy, |on| unsafe { set(on) });
    };
    let number = |key, set: unsafe extern "C" fn(i64)| {
        // SAFETY: as above.
        option(options, key, long, |n| unsafe { set(n) });
    };
    let seconds = |kind, value| {
        // SAFETY: as above.
        unsafe { (ops.set_socket_timeout)(kind, timeout(value)) };
    };

    if crate::FLAGS.load(Ordering::Relaxed) & DEBUG != 0
        && get(options, c"debug_mode").is_some_and(|value| truthy(value) != 0)
    {
        // SAFETY: as above.
        unsafe { (ops.set_log_level)(0) };
    }
    // [EventLoop]
    flag(c"enable_signalfd", ops.set_signalfd);
    flag(c"enable_kqueue", ops.set_kqueue);
    // [Logger]
    number(c"trace_flags", ops.set_trace_flags);
    text_option(options, c"log_file", ops.set_log_file);
    number(c"log_level", ops.set_log_level);
    text_option(options, c"log_date_format", ops.set_log_date_format);
    flag(
        c"log_date_with_microseconds",
        ops.set_log_date_with_microseconds,
    );
    number(c"log_rotation", ops.set_log_rotation);
    flag(c"display_errors", ops.set_display_errors);
    flag(c"print_backtrace_on_error", ops.set_print_backtrace);
    // [DNS]
    text_option(options, c"dns_server", ops.set_dns_server);
    // [Socket]
    if let Some(value) = get(options, c"socket_dns_timeout") {
        seconds(DNS, value);
    }
    if let Some(value) = get(options, c"socket_connect_timeout") {
        seconds(CONNECT, value);
    }
    if let Some(value) =
        get(options, c"socket_write_timeout").or_else(|| get(options, c"socket_send_timeout"))
    {
        seconds(WRITE, value);
    }
    if let Some(value) =
        get(options, c"socket_read_timeout").or_else(|| get(options, c"socket_recv_timeout"))
    {
        seconds(READ, value);
    }
    if let Some(value) = get(options, c"socket_buffer_size") {
        let size = size_of(value)?;
        // SAFETY: as above.
        unsafe { (ops.set_socket_buffer_size)(size) };
    }
    if let Some(value) = get(options, c"socket_timeout") {
        seconds(READ, value);
        seconds(WRITE, value);
    }
    // [HTTP2]: (key, setting, parsed as a size).
    let http2: [(&CStr, i32, bool); 6] = [
        (c"http2_header_table_size", HTTP2_HEADER_TABLE_SIZE, true),
        (c"http2_enable_push", HTTP2_ENABLE_PUSH, false),
        (
            c"http2_max_concurrent_streams",
            HTTP2_MAX_CONCURRENT_STREAMS,
            false,
        ),
        (c"http2_init_window_size", HTTP2_INIT_WINDOW_SIZE, true),
        (c"http2_max_frame_size", HTTP2_MAX_FRAME_SIZE, true),
        (
            c"http2_max_header_list_size",
            HTTP2_MAX_HEADER_LIST_SIZE,
            true,
        ),
    ];
    for (key, id, sized) in http2 {
        if let Some(value) = get(options, key) {
            let setting = if sized { size_of(value)? } else { long(value) };
            // SAFETY: as above.
            unsafe { (ops.set_http2_default)(id, setting) };
        }
    }
    Ok(())
}

/// `sw_zval_is_serializable`: false if `value` is, or holds in nested arrays,
/// an object of a class marked not serializable.
/// # Safety
/// `value` is a live zval.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sw_zval_is_serializable(value: *const zval) -> bool {
    let mut value = value;
    loop {
        // SAFETY: per the caller.
        match unsafe { type_of(value) } {
            IS_OBJECT => {
                // SAFETY: an object zval.
                let object = unsafe { value::obj_of(value) };
                // SAFETY: a live object's class lives for the process.
                let class = unsafe { (*object).ce };
                // SAFETY: as above.
                let flags = unsafe { (*class).ce_flags };
                return flags & ZEND_ACC_NOT_SERIALIZABLE == 0;
            }
            IS_ARRAY => {
                let mut serializable = true;
                let check = |element: *mut zval| {
                    // SAFETY: a live element.
                    serializable = unsafe { sw_zval_is_serializable(element) };
                    serializable
                };
                // SAFETY: an array zval.
                let table = unsafe { value::table(value) };
                // SAFETY: a live table, which the walk does not modify.
                unsafe { each_value(table, check) };
                return serializable;
            }
            IS_REFERENCE => {
                // SAFETY: a live zval.
                let inner = unsafe { (*value).value };
                // SAFETY: a reference zval holds a live reference.
                let reference = unsafe { inner.ref_ };
                // SAFETY: as above.
                value = unsafe { &raw const (*reference).val };
            }
            _ => return true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::clamp;

    #[test]
    fn timeouts_clamp_like_timeout_format() {
        let forever = f64::from(i32::MAX);
        assert!((clamp(1.5) - 1.5).abs() < f64::EPSILON);
        assert!(
            (clamp(0.0) - forever).abs() < f64::EPSILON,
            "zero waits forever"
        );
        assert!(
            (clamp(-1.0) - forever).abs() < f64::EPSILON,
            "negative waits forever"
        );
        assert!(
            (clamp(1e12) - forever).abs() < f64::EPSILON,
            "past INT_MAX caps"
        );
    }
}
