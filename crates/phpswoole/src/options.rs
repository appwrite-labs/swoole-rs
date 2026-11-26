//! `php_swoole_set_global_option`: the process-wide settings that
//! `swoole_async_set()`, `Co::set()` and `Process\Pool::set()` take, applied in
//! upstream's order. And `sw_zval_is_serializable`.
use crate::{constants::DEBUG, module::globals};
use std::{
    ffi::{CStr, CString, c_int},
    sync::atomic::Ordering,
};
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::utilities::{process_logger, set_log_file, swoole_set_log_level};
use swoole_rs_corebase::{
    Switch, built_with, swr_v1_core_set_enable_kqueue, swr_v1_core_set_enable_signalfd,
    swr_v1_core_set_print_backtrace_on_error, swr_v1_core_set_trace_flags,
};
use swoole_rs_zend::{Array, Value, ZStr, bailout, sys::ZEND_ACC_NOT_SERIALIZABLE};

use swoole_rs_http2::setting_id;

// crates/clients' resolver address (`dns_server`).
use swoole_rs_clients::dns::set_dns_server;

/// `php_swoole_array_get_value`: the value under `key`, unless absent or null.
/// Stop reading options after a conversion raises.
pub(crate) fn get<'a>(options: &'a Array, key: &CStr) -> Option<&'a Value> {
    if !setting_ok() {
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
    let seconds = value.applied_double()?;
    setting_ok().then(|| clamp(seconds))
}

fn clamp(timeout: f64) -> f64 {
    if timeout <= 0.0 || timeout > f64::from(i32::MAX) {
        f64::from(i32::MAX)
    } else {
        timeout
    }
}

fn setting_ok() -> bool {
    !bailout::pending() && swoole_rs_zend::host::exception().is_null()
}

fn invalid_setting(message: &CStr) {
    if setting_ok() {
        crate::exception().throw(
            message,
            swoole_rs_error::ErrorCode::SW_ERROR_INVALID_PARAMS as i64,
        );
    }
}

/// Parse a PHP size setting, including ini quantities such as "64M".
pub fn php_swoole_parse_to_size(value: &Value) -> Option<i64> {
    if let Some(string) = value.str() {
        return string
            .parse_quantity()
            .map_err(|(_, error)| {
                let message = [
                    b"failed to parse '".as_slice(),
                    string.c_str().to_bytes(),
                    b"' to size, Error: ",
                    error.c_str().to_bytes(),
                ]
                .concat();
                invalid_setting(&CString::new(message).unwrap_or_default());
            })
            .ok();
    }
    let size = long(value);
    setting_ok().then_some(size)
}

/// The option under `key`, if set, as `convert` reads it, passed to `set`.
fn option<T>(options: &Array, key: &CStr, convert: impl FnOnce(&Value) -> T, set: impl FnOnce(T)) {
    if let Some(value) = get(options, key) {
        let converted = convert(value);
        if setting_ok() {
            set(converted);
        }
    }
}

/// The option under `key`, if set, as a NUL-terminated string (`zend::String`)
/// with its length, for `set`.
fn text_option(options: &Array, key: &CStr, set: impl FnOnce(&ZStr)) {
    if let Some(value) = get(options, key) {
        let text = value.string();
        if setting_ok() {
            set(&text);
        }
    }
}

/// `SW_ERROR_OPERATION_NOT_SUPPORT`.
const OPERATION_NOT_SUPPORT: i64 =
    swoole_rs_error::ErrorCode::SW_ERROR_OPERATION_NOT_SUPPORT as i64;

/// Convert a socket or HTTP/2 setting without wrapping its unsigned destination.
fn nonnegative_setting(key: &CStr, value: i64) -> Option<u32> {
    if !setting_ok() {
        return None;
    }
    if value < 0 {
        let message = swoole_rs_zend::message!(
            "{} must be greater than or equal to 0, got {value}",
            key.to_string_lossy()
        );
        invalid_setting(message.as_cstr());
        return None;
    }
    Some(u32::try_from(value).unwrap_or(u32::MAX))
}

/// Apply global settings in order, stopping on a conversion error.
pub fn set_global(options: &Array) -> bool {
    apply(options).is_some() && setting_ok()
}

fn apply(options: &Array) -> Option<()> {
    let flag = |key, set: &dyn Fn(bool)| option(options, key, truthy, |on| set(on != 0));
    let number = |key, set: &dyn Fn(i64)| option(options, key, long, set);
    let defaults = &swoole_rs_corebase::reactor::SOCKET_DEFAULTS;

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
    number(c"log_level", &|level| {
        swoole_set_log_level(level.c_cast::<c_int>());
    });
    text_option(options, c"log_date_format", |format| {
        if let Some(logger) = process_logger() {
            logger.set_date_format(format.c_str());
        }
    });
    flag(c"log_date_with_microseconds", &|on| {
        if let Some(logger) = process_logger() {
            logger.set_date_with_microseconds(on);
        }
    });
    number(c"log_rotation", &|rotation| {
        if let Some(logger) = process_logger() {
            logger.set_rotation(rotation.c_cast::<c_int>());
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
        defaults.set_buffer_size(nonnegative_setting(
            c"socket_buffer_size",
            php_swoole_parse_to_size(value)?,
        )?);
    }
    if let Some(both) = get(options, c"socket_timeout").and_then(timeout) {
        defaults.read.set(both);
        defaults.write.set(both);
    }
    // [HTTP2]: (key, setting, parsed as a size).
    let http2 = [
        (
            c"http2_header_table_size",
            setting_id::HEADER_TABLE_SIZE,
            true,
        ),
        (c"http2_enable_push", setting_id::ENABLE_PUSH, false),
        (
            c"http2_max_concurrent_streams",
            setting_id::MAX_CONCURRENT_STREAMS,
            false,
        ),
        (
            c"http2_init_window_size",
            setting_id::INIT_WINDOW_SIZE,
            true,
        ),
        (c"http2_max_frame_size", setting_id::MAX_FRAME_SIZE, true),
        (
            c"http2_max_header_list_size",
            setting_id::MAX_HEADER_LIST_SIZE,
            true,
        ),
    ];
    for (key, id, sized) in http2 {
        if let Some(value) = get(options, key) {
            let setting = if sized {
                php_swoole_parse_to_size(value)?
            } else {
                long(value)
            };
            swoole_rs_http2::put_default_setting(u32::from(id), nonnegative_setting(key, setting)?);
        }
    }
    if let Some(value) = get(options, c"http2_max_headers") {
        let limit = long(value);
        if !setting_ok() {
            return None;
        }
        swoole_rs_http2::set_max_headers(limit.clamp(0, i64::from(u32::MAX)).c_cast::<u32>());
    }
    Some(())
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
