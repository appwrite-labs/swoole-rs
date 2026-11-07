//! `Swoole\Redis\Server::format`: a PHP value as a RESP reply, over the
//! protocols crate's reply framing.
use std::ffi::{CStr, c_void};
use swoole_rs_protocols::reply_parts;
use swoole_rs_zend::{
    Call,
    string::bytes,
    sys::{
        IS_ARRAY, IS_FALSE, IS_LONG, IS_NULL, IS_TRUE, swr_zend_array_is_list,
        swr_zend_hash_foreach_kv, swr_zend_string_release, zend_long, zend_string, zend_ulong,
        zval,
    },
    throw_exception, type_of, value,
};

/// `swoole::redis::ReplyType`.
pub(crate) const ERROR: zend_long = 0;
pub(crate) const NIL: zend_long = 1;
pub(crate) const STATUS: zend_long = 2;
pub(crate) const INT: zend_long = 3;
pub(crate) const STRING: zend_long = 4;
pub(crate) const SET: zend_long = 5;
pub(crate) const MAP: zend_long = 6;
/// `reply_parts`' map keys: a string, and an integer.
const KEY_STRING: u8 = 8;
const KEY_INTEGER: u8 = 9;

/// `SW_REDIS_MAX_STRING_SIZE`.
const MAX_STRING_SIZE: usize = 536_870_912;
/// `SW_ERROR_INVALID_PARAMS`.
const INVALID_PARAMS: zend_long = 505;

/// `redis_response_append`: one reply's prefix, body and CRLF.
fn append(buffer: &mut Vec<u8>, kind: u8, present: bool, number: u64, data: &[u8]) {
    let Some(parts) = reply_parts(kind, present, number, data) else {
        return;
    };
    let prefix = parts.prefix.get(..parts.prefix_length as usize);
    buffer.extend_from_slice(prefix.unwrap_or_default());
    buffer.extend_from_slice(data.get(..parts.body_length).unwrap_or_default());
    let suffix = b"\r\n".get(..parts.suffix_length as usize);
    buffer.extend_from_slice(suffix.unwrap_or_default());
}

fn throw(message: &CStr) {
    // SAFETY: Swoole's exception class, registered at MINIT.
    unsafe { throw_exception(crate::exception(), message, INVALID_PARAMS) };
}

/// `zend::String(value)`: `zval_get_string`, whose bytes `each` reads.
fn with_string(value: *const zval, each: impl FnOnce(&[u8])) {
    // SAFETY: a live value; a bailout is deferred.
    let string = unsafe { value::get_string(value) };
    // SAFETY: a live string (or the empty one after a bailout).
    each(unsafe { bytes(string) });
    // SAFETY: the reference `get_string` gave.
    unsafe { swr_zend_string_release(string) };
}

/// `redis_response_format`: false after it threw.
fn format(buffer: &mut Vec<u8>, kind: zend_long, value: *const zval) -> bool {
    let present = !value.is_null();
    match kind {
        NIL => append(buffer, 1, true, 0, &[]),
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "ERROR or STATUS"
        )]
        ERROR | STATUS if present => {
            with_string(value, |text| append(buffer, kind as u8, true, 0, text));
        }
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "ERROR or STATUS"
        )]
        ERROR | STATUS => append(buffer, kind as u8, false, 0, &[]),
        INT | STRING | SET | MAP if !present => {
            throw(c"require more parameters");
            return false;
        }
        INT => {
            // SAFETY: a live value; a bailout is deferred.
            let integer = unsafe { value::get_long(value) };
            append(buffer, 3, true, integer.cast_unsigned(), &[]);
        }
        STRING => {
            let mut fits = true;
            with_string(value, |text| {
                fits = text.len() <= MAX_STRING_SIZE;
                if fits {
                    append(buffer, 4, true, 0, text);
                }
            });
            if !fits {
                throw(c"the length of given string exceeds the maximum allowed value");
                return false;
            }
        }
        SET | MAP => {
            // SAFETY: a live value.
            if unsafe { type_of(value) } != IS_ARRAY {
                // The exception ends the reply.
                throw(c"the second parameter should be an array");
                return false;
            }
            // SAFETY: a live array.
            let count = unsafe { value::count(value) };
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "SET or MAP"
            )]
            append(buffer, kind as u8, true, u64::from(count), &[]);
            // SAFETY: a live array.
            let table = unsafe { value::table(value) };
            let mut walk = Walk {
                buffer,
                map: kind == MAP,
            };
            // SAFETY: a live array; the callback only reads it.
            unsafe { swr_zend_hash_foreach_kv(table, Some(item), (&raw mut walk).cast()) };
        }
        _ => {
            #[expect(clippy::cast_possible_truncation, reason = "upstream's `(int) type`")]
            let message = format!("Unknown type[{}]", kind as i32);
            let message = std::ffi::CString::new(message).unwrap_or_default();
            throw(&message);
            return false;
        }
    }
    true
}

/// `Z_ARRVAL_P`.
fn table(array: *const zval) -> *mut swoole_rs_zend::sys::HashTable {
    // SAFETY: the callers pass a live array.
    unsafe { value::table(array) }
}

/// A `SET` or `MAP` walk.
struct Walk<'a> {
    buffer: &'a mut Vec<u8>,
    map: bool,
}

/// One entry: a map's key, then `redis_response_format_array_item`.
unsafe extern "C" fn item(
    key: *mut zend_string,
    index: zend_ulong,
    entry: *mut zval,
    walk: *mut c_void,
) {
    // SAFETY: `format`'s walk, borrowed for the loop.
    let walk = unsafe { &mut *walk.cast::<Walk>() };
    if walk.map {
        if key.is_null() {
            append(walk.buffer, KEY_INTEGER, true, index, &[]);
        } else {
            // SAFETY: a live key.
            append(walk.buffer, KEY_STRING, true, 0, unsafe { bytes(key) });
        }
    }
    // SAFETY: a live entry.
    let kind = match unsafe { type_of(entry) } {
        IS_LONG | IS_FALSE | IS_TRUE => INT,
        // SAFETY: a live array.
        IS_ARRAY if unsafe { swr_zend_array_is_list(table(entry)) } => SET,
        IS_ARRAY => MAP,
        IS_NULL => NIL,
        _ => STRING,
    };
    format(walk.buffer, kind, entry);
}

/// `format(int $type, mixed $value = null): string|false`.
pub(crate) fn method(call: &Call) {
    let Ok(kind) = call.arity(1, 2).and_then(|()| call.long(1, 0)) else {
        call.set_bool(false);
        return;
    };
    let value = call.zval(2);
    let mut buffer = Vec::with_capacity(1024);
    if !format(&mut buffer, kind, value) {
        call.set_bool(false);
        return;
    }
    // SAFETY: a fresh string whose reference the return slot takes.
    unsafe { call.set_string(crate::zend_string(&buffer)) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalars_frame_as_upstream() {
        let mut buffer = Vec::new();
        append(&mut buffer, 1, true, 0, &[]);
        append(&mut buffer, 0, false, 0, &[]);
        append(&mut buffer, 2, true, 0, b"SUCCESS");
        append(&mut buffer, 3, true, 1000, &[]);
        append(&mut buffer, 4, true, 0, b"hello swoole");
        assert_eq!(
            buffer,
            b"$-1\r\n-ERR\r\n+SUCCESS\r\n:1000\r\n$12\r\nhello swoole\r\n"
        );
    }

    #[test]
    fn map_keys_frame_as_bulk_strings() {
        let mut buffer = Vec::new();
        append(&mut buffer, 6, true, 2, &[]);
        append(&mut buffer, KEY_STRING, true, 0, b"php");
        append(&mut buffer, KEY_INTEGER, true, 9999, &[]);
        assert_eq!(buffer, b"*4\r\n$3\r\nphp\r\n$4\r\n9999\r\n");
    }
}
