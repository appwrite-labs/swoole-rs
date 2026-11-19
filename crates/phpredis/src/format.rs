//! `Swoole\Redis\Server::format`: a PHP value as a RESP reply, over the
//! protocols crate's reply framing.
use std::ffi::CStr;
use swoole_rs_corebase::CCast;
use swoole_rs_protocols::Reply;
use swoole_rs_zend::{
    Call, Key, Value, ZString,
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
/// `SW_REDIS_MAX_STRING_SIZE`.
const MAX_STRING_SIZE: usize = 536_870_912;
/// `SW_ERROR_INVALID_PARAMS`.
const INVALID_PARAMS: zend_long = swoole_rs_error::ErrorCode::SW_ERROR_INVALID_PARAMS as zend_long;

fn throw(message: &CStr) {
    swoole_rs_phpswoole::exception().throw(message, INVALID_PARAMS);
}

/// `redis_response_format`: false after it threw.
fn format(buffer: &mut Vec<u8>, kind: zend_long, value: Option<&Value>) -> bool {
    match (kind, value) {
        (NIL, _) => Reply::Nil.append_to(buffer),
        (ERROR | STATUS, value) => {
            let text = value.map(Value::string);
            let bytes = text.as_ref().map(ZString::bytes);
            let reply = if kind == ERROR {
                Reply::Error(bytes)
            } else {
                Reply::Status(bytes)
            };
            reply.append_to(buffer);
        }
        (INT | STRING | SET | MAP, None) => {
            throw(c"require more parameters");
            return false;
        }
        (INT, Some(value)) => Reply::Integer(value.long()).append_to(buffer),
        (STRING, Some(value)) => {
            let text = value.string();
            if text.bytes().len() > MAX_STRING_SIZE {
                throw(c"the length of given string exceeds the maximum allowed value");
                return false;
            }
            Reply::Bulk(text.bytes()).append_to(buffer);
        }
        (SET | MAP, Some(value)) => {
            let Some(array) = value.array() else {
                // The exception ends the reply.
                throw(c"the second parameter should be an array");
                return false;
            };
            let count = array.count();
            let reply = if kind == SET {
                Reply::Set(count)
            } else {
                Reply::Map(count)
            };
            reply.append_to(buffer);
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
        Key::Index(index) if map => Reply::IntegerKey(index).append_to(buffer),
        Key::Name(name) if map => Reply::StringKey(name).append_to(buffer),
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
    call.set_zstring(ZString::new(&buffer));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalars_frame_as_upstream() {
        let mut buffer = Vec::new();
        Reply::Nil.append_to(&mut buffer);
        Reply::Error(None).append_to(&mut buffer);
        Reply::Status(Some(b"SUCCESS")).append_to(&mut buffer);
        Reply::Integer(1000).append_to(&mut buffer);
        Reply::Bulk(b"hello swoole").append_to(&mut buffer);
        assert_eq!(
            buffer,
            b"$-1\r\n-ERR\r\n+SUCCESS\r\n:1000\r\n$12\r\nhello swoole\r\n"
        );
    }

    #[test]
    fn map_keys_frame_as_bulk_strings() {
        let mut buffer = Vec::new();
        Reply::Map(2).append_to(&mut buffer);
        Reply::StringKey(b"php").append_to(&mut buffer);
        Reply::IntegerKey(9999).append_to(&mut buffer);
        assert_eq!(buffer, b"*4\r\n$3\r\nphp\r\n$4\r\n9999\r\n");
    }
}
