//! `Swoole\Redis\Server::format`: a PHP value as a RESP reply, over the
//! protocols crate's reply framing.
use std::ffi::CStr;
use swoole_rs_corebase::CCast;
use swoole_rs_protocols::reply_parts;
use swoole_rs_zend::{
    Call, Key, Value,
    sys::{IS_ARRAY, IS_FALSE, IS_LONG, IS_NULL, IS_TRUE, zend_long},
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
    swoole_rs_phpswoole::exception().throw(message, INVALID_PARAMS);
}

/// `redis_response_format`: false after it threw.
fn format(buffer: &mut Vec<u8>, kind: zend_long, value: Option<&Value>) -> bool {
    match (kind, value) {
        (NIL, _) => append(buffer, 1, true, 0, &[]),
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "ERROR or STATUS"
        )]
        (ERROR | STATUS, Some(value)) => {
            append(buffer, kind as u8, true, 0, value.string().bytes());
        }
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "ERROR or STATUS"
        )]
        (ERROR | STATUS, None) => append(buffer, kind as u8, false, 0, &[]),
        (INT | STRING | SET | MAP, None) => {
            throw(c"require more parameters");
            return false;
        }
        (INT, Some(value)) => append(buffer, 3, true, value.long().cast_unsigned(), &[]),
        (STRING, Some(value)) => {
            let text = value.string();
            if text.bytes().len() > MAX_STRING_SIZE {
                throw(c"the length of given string exceeds the maximum allowed value");
                return false;
            }
            append(buffer, 4, true, 0, text.bytes());
        }
        (SET | MAP, Some(value)) => {
            let Some(array) = value.array() else {
                // The exception ends the reply.
                throw(c"the second parameter should be an array");
                return false;
            };
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "SET or MAP"
            )]
            append(buffer, kind as u8, true, u64::from(array.count()), &[]);
            array.for_each(|key, entry| item(buffer, kind == MAP, key, entry));
        }
        (_, _) => {
            let message = format!("Unknown type[{}]", kind.c_cast::<i32>());
            let message = std::ffi::CString::new(message).unwrap_or_default();
            throw(&message);
            return false;
        }
    }
    true
}

/// One entry: a map's key, then `redis_response_format_array_item`.
fn item(buffer: &mut Vec<u8>, map: bool, key: Key, entry: &Value) {
    match key {
        Key::Index(index) if map => append(buffer, KEY_INTEGER, true, index, &[]),
        Key::Name(name) if map => append(buffer, KEY_STRING, true, 0, name),
        _ => {}
    }
    let kind = match entry.kind() {
        IS_LONG | IS_FALSE | IS_TRUE => INT,
        IS_ARRAY if entry.array().is_some_and(swoole_rs_zend::Array::is_list) => SET,
        IS_ARRAY => MAP,
        IS_NULL => NIL,
        _ => STRING,
    };
    format(buffer, kind, Some(entry));
}

/// `format(int $type, mixed $value = null): string|false`.
pub(crate) fn method(call: &Call) {
    let Ok(kind) = call.arity(1, 2).and_then(|()| call.long(1, 0)) else {
        call.set_bool(false);
        return;
    };
    let value = call.value(2);
    let mut buffer = Vec::with_capacity(1024);
    if !format(&mut buffer, kind, value) {
        call.set_bool(false);
        return;
    }
    call.set_zstring(swoole_rs_zend::ZString::new(&buffer));
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
