//! `php_swoole_set_global_option`: the process-wide settings that
//! `swoole_async_set()`, `Co::set()` and `Process\Pool::set()` take, applied in
//! upstream's order. And `sw_zval_is_serializable`.
use crate::{
    constants::DEBUG,
    module::{globals, size_error},
    ops,
};
use std::{
    ffi::{CStr, c_char, c_int},
    ptr,
    sync::atomic::Ordering,
};
use swoole_rs_corebase::{
    Switch, built_with, swr_v1_core_set_enable_kqueue, swr_v1_core_set_enable_signalfd,
    swr_v1_core_set_print_backtrace_on_error, swr_v1_core_set_trace_flags,
};
use swoole_rs_utilities::{
    Logger, LoggerSetting, process_logger, swoole_set_log_file, swoole_set_log_level,
    swr_v1_logger_set, swr_v1_logger_set_date_format,
};
use swoole_rs_zend::{
    bailout, each_value,
    string::bytes,
    sys::{
        IS_ARRAY, IS_NULL, IS_OBJECT, IS_REFERENCE, IS_STRING, ZEND_ACC_NOT_SERIALIZABLE,
        swr_zend_string_release, swr_zval_get_double, swr_zval_get_long, swr_zval_get_string,
        swr_zval_is_true, zend_array, zend_hash_str_find, zend_ini_parse_quantity, zend_string,
        zval,
    },
    throw_exception, type_of, value,
};

/// `swHttp2SettingId`.
const HTTP2_HEADER_TABLE_SIZE: u32 = 1;
const HTTP2_ENABLE_PUSH: u32 = 2;
const HTTP2_MAX_CONCURRENT_STREAMS: u32 = 3;
const HTTP2_INIT_WINDOW_SIZE: u32 = 4;
const HTTP2_MAX_FRAME_SIZE: u32 = 5;
const HTTP2_MAX_HEADER_LIST_SIZE: u32 = 6;

/// `php_swoole_array_get_value`: the value under `key`, unless absent or null.
pub(crate) fn get(options: *mut zend_array, key: &CStr) -> Option<*mut zval> {
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
pub(crate) fn size(value: *mut zval) -> Result<i64, *mut zend_string> {
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

/// The option under `key`, if set, as a NUL-terminated string (`zend::String`)
/// with its length, for `set`.
fn text_option(options: *mut zend_array, key: &CStr, set: impl FnOnce(*const c_char, usize)) {
    let Some(value) = get(options, key) else {
        return;
    };
    // SAFETY: a live zval; a new reference released below.
    let string = unsafe { swr_zval_get_string(value) };
    // SAFETY: a live string, NUL-terminated after its bytes.
    let text = unsafe { bytes(string) };
    set(text.as_ptr().cast(), text.len());
    // SAFETY: the reference taken above.
    unsafe { swr_zend_string_release(string) };
}

/// C's implicit `zend_long` to `int` conversion.
#[expect(
    clippy::cast_possible_truncation,
    reason = "upstream passes the zend_long as int"
)]
const fn int(value: i64) -> c_int {
    value as c_int
}

/// `sw_logger()`, which exists from MINIT on.
fn logger() -> *const Logger {
    process_logger().map_or(ptr::null(), ptr::from_ref)
}

/// `php_swoole_set_global_option(options)`. A size that does not parse stops
/// here with a fatal error.
/// # Safety
/// After MINIT, with a live options table; the caller is C or C++ (the fatal
/// error bails out from here).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn php_swoole_set_global_option(options: *mut zend_array) {
    set_global(options);
    bailout::finish();
}

/// `SW_ERROR_OPERATION_NOT_SUPPORT`.
const OPERATION_NOT_SUPPORT: i64 = 507;

/// `php_swoole_set_global_option`'s body: false after a size that does not
/// parse, whose fatal error is deferred.
pub fn set_global(options: *mut zend_array) -> bool {
    let Err((value, error)) = apply(options) else {
        return true;
    };
    // SAFETY: only a string fails to parse.
    let failed = unsafe { value::str_of(value) };
    // SAFETY: a live string, and Zend's error string.
    unsafe { size_error(failed, error) };
    false
}

fn apply(options: *mut zend_array) -> Result<(), (*mut zval, *mut zend_string)> {
    let ops = ops();
    let size_of = |value| size(value).map_err(|error| (value, error));
    let flag = |key, set: &dyn Fn(bool)| option(options, key, truthy, |on| set(on != 0));
    let number = |key, set: &dyn Fn(i64)| option(options, key, long, set);
    let defaults = &raw mut swoole_rs_reactor::swr_v1_socket_defaults;
    // SAFETY: the process-wide socket defaults, which only this main-thread
    // setter writes; native code reads them by name.
    let timeouts = unsafe { &mut (*defaults).timeouts };

    if crate::FLAGS.load(Ordering::Relaxed) & DEBUG != 0
        && get(options, c"debug_mode").is_some_and(|value| truthy(value) != 0)
    {
        swoole_set_log_level(0);
    }
    // [EventLoop]
    flag(c"enable_signalfd", &|on| {
        swr_v1_core_set_enable_signalfd(on);
    });
    flag(c"enable_kqueue", &|on| swr_v1_core_set_enable_kqueue(on));
    // [Logger]: the process logger, live from MINIT on.
    number(c"trace_flags", &|flags| swr_v1_core_set_trace_flags(flags));
    text_option(options, c"log_file", |file, _| {
        // SAFETY: a NUL-terminated path.
        unsafe { swoole_set_log_file(file) };
    });
    number(c"log_level", &|level| swoole_set_log_level(int(level)));
    text_option(options, c"log_date_format", |format, _| {
        // SAFETY: the live logger and a NUL-terminated format.
        unsafe { swr_v1_logger_set_date_format(logger(), format) };
    });
    flag(c"log_date_with_microseconds", &|on| {
        // SAFETY: the live logger.
        unsafe { swr_v1_logger_set(logger(), LoggerSetting::Microseconds, c_int::from(on)) };
    });
    number(c"log_rotation", &|rotation| {
        // SAFETY: the live logger.
        unsafe { swr_v1_logger_set(logger(), LoggerSetting::Rotation, int(rotation)) };
    });
    if let Some(value) = get(options, c"display_errors") {
        // SAFETY: this thread's live module globals.
        unsafe { (*globals()).display_errors = truthy(value) != 0 };
    }
    flag(c"print_backtrace_on_error", &|on| {
        if built_with(Switch::Stacktrace) || built_with(Switch::Execinfo) {
            swr_v1_core_set_print_backtrace_on_error(on);
            return;
        }
        // SAFETY: a handler's exception, of a class MINIT registered.
        unsafe {
            throw_exception(
                crate::ERROR.entry(),
                c"The `print_backtrace_on_error` option requires `boost stacktrace` or `execinfo.h` to be installed",
                OPERATION_NOT_SUPPORT,
            );
        }
    });
    // [DNS]
    text_option(options, c"dns_server", |server, length| {
        // SAFETY: C++'s setter, with a string that lives for the call.
        unsafe { (ops.set_dns_server)(server, length) };
    });
    // [Socket]
    if let Some(value) = get(options, c"socket_dns_timeout") {
        timeouts.dns = timeout(value);
    }
    if let Some(value) = get(options, c"socket_connect_timeout") {
        timeouts.connect = timeout(value);
    }
    if let Some(value) =
        get(options, c"socket_write_timeout").or_else(|| get(options, c"socket_send_timeout"))
    {
        timeouts.write = timeout(value);
    }
    if let Some(value) =
        get(options, c"socket_read_timeout").or_else(|| get(options, c"socket_recv_timeout"))
    {
        timeouts.read = timeout(value);
    }
    if let Some(value) = get(options, c"socket_buffer_size") {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "upstream assigns the size to a uint32_t"
        )]
        let size = size_of(value)? as u32;
        // SAFETY: as for the timeouts.
        unsafe { (*defaults).buffer_size = size };
    }
    if let Some(value) = get(options, c"socket_timeout") {
        timeouts.read = timeout(value);
        timeouts.write = timeouts.read;
    }
    // [HTTP2]: (key, setting, parsed as a size).
    let http2: [(&CStr, u32, bool); 6] = [
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
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "upstream passes the zend_long as uint32_t"
            )]
            swoole_rs_http2::put_default_setting(id, setting as u32);
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
