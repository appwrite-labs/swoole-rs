//! `php_swoole_set_global_option`: the process-wide settings that
//! `swoole_async_set()`, `Co::set()` and `Process\Pool::set()` take, applied in
//! upstream's order. And `sw_zval_is_serializable`.
use crate::{
    constants::DEBUG,
    module::{PHP_FATAL_ERROR, globals},
};
use std::{
    ffi::{CStr, CString, c_int},
    sync::atomic::Ordering,
};
use swoole_rs_corebase::{
    Switch, built_with, log::set_last_error, swr_v1_core_set_enable_kqueue,
    swr_v1_core_set_enable_signalfd, swr_v1_core_set_print_backtrace_on_error,
    swr_v1_core_set_trace_flags,
};
use swoole_rs_utilities::{
    LoggerSetting, process_logger, set_log_file, swoole_set_log_level, swr_v1_logger_set,
    swr_v1_logger_set_date_format,
};
use swoole_rs_zend::{
    Array, E_ERROR, Value, ZStr, ZString, bailout, report, sys::ZEND_ACC_NOT_SERIALIZABLE,
};

/// `swHttp2SettingId`.
const HTTP2_HEADER_TABLE_SIZE: u32 = 1;
const HTTP2_ENABLE_PUSH: u32 = 2;
const HTTP2_MAX_CONCURRENT_STREAMS: u32 = 3;
const HTTP2_INIT_WINDOW_SIZE: u32 = 4;
const HTTP2_MAX_FRAME_SIZE: u32 = 5;
const HTTP2_MAX_HEADER_LIST_SIZE: u32 = 6;

// crates/clients' resolver address (`dns_server`).
use swoole_rs_clients::dns::set_dns_server;

/// `php_swoole_array_get_value`: the value under `key`, unless absent or null.
/// Nothing is found once a bailout is pending: upstream's setters stopped at
/// the conversion that bailed out, so no later option is read or applied.
pub(crate) fn get<'a>(options: &'a Array, key: &CStr) -> Option<&'a Value> {
    if bailout::pending() {
        return None;
    }
    options.get(key.to_bytes())
}

fn truthy(value: &Value) -> u8 {
    u8::from(value.truthy())
}

fn long(value: &Value) -> i64 {
    value.long()
}

/// `timeout_format`: non-positive or oversized timeouts mean `INT_MAX` seconds;
/// `None` after a bailout, as [`Value::applied_double`].
fn timeout(value: &Value) -> Option<f64> {
    value.applied_double().map(clamp)
}

fn clamp(timeout: f64) -> f64 {
    if timeout <= 0.0 || timeout > f64::from(i32::MAX) {
        f64::from(i32::MAX)
    } else {
        timeout
    }
}

/// `php_swoole_parse_to_size`, or the string that did not parse and Zend's
/// reason.
fn size(value: &Value) -> Result<i64, (&ZStr, ZString)> {
    match value.str() {
        None => Ok(long(value)),
        Some(string) => string.parse_quantity().map_err(|error| (string, error)),
    }
}

/// `php_swoole_fatal_error(E_ERROR, "failed to parse '%s' to size, Error: %s")`:
/// a bailout, deferred for the caller's [`bailout::finish`].
fn size_error(failed: &ZStr, error: &ZStr) {
    set_last_error(PHP_FATAL_ERROR);
    let message = [
        b"failed to parse '",
        failed.c_str().to_bytes(),
        b"' to size, Error: ",
        error.c_str().to_bytes(),
    ]
    .concat();
    let message = CString::new(message).unwrap_or_default();
    bailout::defer(report(E_ERROR, &message));
}

/// `php_swoole_parse_to_size(zv)`: a size string as an ini quantity
/// ("64M"), anything else as `zval_get_long`. A string that does not parse
/// is a fatal error, raised from here.
pub fn php_swoole_parse_to_size(value: &Value) -> i64 {
    let size = size(value).unwrap_or_else(|(failed, error)| {
        size_error(failed, &error);
        0
    });
    bailout::finish();
    size
}

/// The option under `key`, if set, as `convert` reads it, passed to `set`.
fn option<T>(options: &Array, key: &CStr, convert: impl FnOnce(&Value) -> T, set: impl FnOnce(T)) {
    if let Some(value) = get(options, key) {
        let converted = convert(value);
        if !bailout::pending() {
            set(converted);
        }
    }
}

/// The option under `key`, if set, as a NUL-terminated string (`zend::String`)
/// with its length, for `set`.
fn text_option(options: &Array, key: &CStr, set: impl FnOnce(&ZStr)) {
    if let Some(value) = get(options, key) {
        set(&value.string());
    }
}

/// C's implicit `zend_long` to `int` conversion.
#[expect(
    clippy::cast_possible_truncation,
    reason = "upstream passes the zend_long as int"
)]
const fn int(value: i64) -> c_int {
    value as c_int
}

/// `php_swoole_set_global_option(options)`, after MINIT. A size that does not
/// parse stops here with a fatal error.
pub fn php_swoole_set_global_option(options: &Array) {
    set_global(options);
    bailout::finish();
}

/// `SW_ERROR_OPERATION_NOT_SUPPORT`.
const OPERATION_NOT_SUPPORT: i64 = 507;

/// `php_swoole_set_global_option`'s body: false after a size that does not
/// parse, whose fatal error is deferred, or a conversion's deferred bailout.
pub fn set_global(options: &Array) -> bool {
    let Err((failed, error)) = apply(options) else {
        return !bailout::pending();
    };
    size_error(failed, &error);
    false
}

fn apply(options: &Array) -> Result<(), (&ZStr, ZString)> {
    let size_of = size;
    let flag = |key, set: &dyn Fn(bool)| option(options, key, truthy, |on| set(on != 0));
    let number = |key, set: &dyn Fn(i64)| option(options, key, long, set);
    let defaults = &swoole_rs_reactor::SOCKET_DEFAULTS;

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
    text_option(options, c"log_file", |file| set_log_file(file.c_str()));
    number(c"log_level", &|level| swoole_set_log_level(int(level)));
    text_option(options, c"log_date_format", |format| {
        if let Some(logger) = process_logger() {
            // SAFETY: a NUL-terminated format (a Zend string's bytes).
            unsafe { swr_v1_logger_set_date_format(logger, format.c_str().as_ptr()) };
        }
    });
    flag(c"log_date_with_microseconds", &|on| {
        if let Some(logger) = process_logger() {
            swr_v1_logger_set(logger, LoggerSetting::Microseconds, c_int::from(on));
        }
    });
    number(c"log_rotation", &|rotation| {
        if let Some(logger) = process_logger() {
            swr_v1_logger_set(logger, LoggerSetting::Rotation, int(rotation));
        }
    });
    if let Some(value) = get(options, c"display_errors") {
        globals().display_errors.set(truthy(value) != 0);
    }
    flag(c"print_backtrace_on_error", &|on| {
        if built_with(Switch::Stacktrace) || built_with(Switch::Execinfo) {
            swr_v1_core_set_print_backtrace_on_error(on);
            return;
        }
        crate::ERROR.throw(
            c"The `print_backtrace_on_error` option requires `boost stacktrace` or `execinfo.h` to be installed",
            OPERATION_NOT_SUPPORT,
        );
    });
    // [DNS]
    text_option(options, c"dns_server", |server| {
        set_dns_server(server.bytes());
    });
    // [Socket]
    if let Some(dns) = get(options, c"socket_dns_timeout").and_then(timeout) {
        defaults.dns.set(dns);
    }
    if let Some(connect) = get(options, c"socket_connect_timeout").and_then(timeout) {
        defaults.connect.set(connect);
    }
    if let Some(write) = get(options, c"socket_write_timeout")
        .or_else(|| get(options, c"socket_send_timeout"))
        .and_then(timeout)
    {
        defaults.write.set(write);
    }
    if let Some(read) = get(options, c"socket_read_timeout")
        .or_else(|| get(options, c"socket_recv_timeout"))
        .and_then(timeout)
    {
        defaults.read.set(read);
    }
    if let Some(value) = get(options, c"socket_buffer_size") {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "upstream assigns the size to a uint32_t"
        )]
        let size = size_of(value)? as u32;
        if bailout::pending() {
            return Ok(());
        }
        defaults.set_buffer_size(size);
    }
    if let Some(both) = get(options, c"socket_timeout").and_then(timeout) {
        defaults.read.set(both);
        defaults.write.set(both);
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
            if bailout::pending() {
                return Ok(());
            }
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
#[must_use]
pub fn sw_zval_is_serializable(value: &Value) -> bool {
    let value = value.dereferenced();
    if let Some(flags) = value.class_flags() {
        return flags & ZEND_ACC_NOT_SERIALIZABLE == 0;
    }
    value.array().is_none_or(|array| {
        array
            .entries()
            .all(|(_, element)| sw_zval_is_serializable(element))
    })
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
