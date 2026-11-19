//! `curl_getinfo`: every field as an array, or one
//! `CURLINFO_*` value. libcurl's `curl_easy_getinfo` is variadic, so each result type has a
//! shim in `native/hooks/curl_handle.cc`.
use super::php_handle::{PhpHandle, handle_argument};
use super::setopt::{List, libcurl, swoole_curl_private, warn};
use std::ffi::c_int;
use swoole_rs_corebase::sys::Text;
use swoole_rs_zend::sys::PHP_VERSION_ID;
use swoole_rs_zend::{Call, E_WARNING, OwnedArray, ZStr};

/// A list libcurl owns: where [`swoole_curl_list_next`] reads next, null at the end.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct Cursor<'a>(*const std::ffi::c_void, std::marker::PhantomData<&'a ()>);

/// libcurl's certificate chain for a handle, which keeps it until its next transfer.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct Certs<'a>(*const std::ffi::c_void, std::marker::PhantomData<&'a ()>);

unsafe extern "C" {
    // Each writes its output only when it returns true (`CURLE_OK`). The handle keeps a
    // string until its next transfer.
    safe fn swoole_curl_info_string<'a>(
        handle: PhpHandle<'a>,
        info: c_int,
        value: &mut Option<Text<'a>>,
    ) -> bool;
    safe fn swoole_curl_info_long(handle: PhpHandle<'_>, info: c_int, value: &mut i64) -> bool;
    safe fn swoole_curl_info_double(handle: PhpHandle<'_>, info: c_int, value: &mut f64) -> bool;
    safe fn swoole_curl_info_off_t(handle: PhpHandle<'_>, info: c_int, value: &mut i64) -> bool;
    // The list is the caller's to free, as [`List`]'s drop does.
    safe fn swoole_curl_info_list(handle: PhpHandle<'_>, info: c_int, list: &mut List) -> bool;
    safe fn swoole_curl_list_next<'a>(cursor: &mut Cursor<'a>) -> Option<Text<'a>>;
    // The certificate count, or -1 when libcurl did not answer.
    safe fn swoole_curl_info_certs<'a>(handle: PhpHandle<'a>, certs: &mut Certs<'a>) -> c_int;
    // `index` is below the count `swoole_curl_info_certs` returned for `certs`.
    safe fn swoole_curl_cert_fields(certs: Certs<'_>, index: c_int) -> Cursor<'_>;
    safe fn swoole_curl_header_out(handle: PhpHandle<'_>) -> Option<&ZStr>;
}

swoole_rs_zend::handlers! {
    zif_swoole_native_curl_getinfo => getinfo;
}

const STRING: c_int = 0x10_0000;
const LONG: c_int = 0x20_0000;
const DOUBLE: c_int = 0x30_0000;
const SLIST: c_int = 0x40_0000;
const OFF_T: c_int = 0x60_0000;
const TYPE_MASK: c_int = 0xf0_0000;
/// PHP's own `CURLINFO_HEADER_OUT`.
const HEADER_OUT: c_int = 2;
const CERTINFO: c_int = SLIST + 34;
const PRIVATE: c_int = STRING + 21;
const CONTENT_TYPE: c_int = STRING + 18;

/// How a field of the full array is read and stored.
#[derive(Clone, Copy)]
enum Field {
    /// `CAAS`: the string, or the empty string for none.
    Text,
    /// `CAAL` of a `long`.
    Long,
    /// `CAAD`.
    Double,
    /// `CAAL` of a `curl_off_t`.
    Offset,
}

/// The full array's fields before the certificate chain, in upstream's order.
const HEAD: &[(&[u8], Field, c_int)] = &[
    (b"http_code", Field::Long, LONG + 2),
    (b"header_size", Field::Long, LONG + 11),
    (b"request_size", Field::Long, LONG + 12),
    (b"filetime", Field::Long, LONG + 14),
    (b"ssl_verify_result", Field::Long, LONG + 13),
    (b"redirect_count", Field::Long, LONG + 20),
    (b"total_time", Field::Double, DOUBLE + 3),
    (b"namelookup_time", Field::Double, DOUBLE + 4),
    (b"connect_time", Field::Double, DOUBLE + 5),
    (b"pretransfer_time", Field::Double, DOUBLE + 6),
    (b"size_upload", Field::Double, DOUBLE + 7),
    (b"size_download", Field::Double, DOUBLE + 8),
    (b"speed_download", Field::Double, DOUBLE + 9),
    (b"speed_upload", Field::Double, DOUBLE + 10),
    (b"download_content_length", Field::Double, DOUBLE + 15),
    (b"upload_content_length", Field::Double, DOUBLE + 16),
    (b"starttransfer_time", Field::Double, DOUBLE + 17),
    (b"redirect_time", Field::Double, DOUBLE + 19),
    (b"redirect_url", Field::Text, STRING + 31),
    (b"primary_ip", Field::Text, STRING + 32),
];

/// The fields after the certificate chain that every supported build has.
const TAIL: &[(&[u8], Field, c_int)] = &[
    (b"primary_port", Field::Long, LONG + 40),
    (b"local_ip", Field::Text, STRING + 41),
    (b"local_port", Field::Long, LONG + 42),
    (b"http_version", Field::Long, LONG + 46),
    (b"protocol", Field::Long, LONG + 48),
    (b"ssl_verifyresult", Field::Long, LONG + 47),
    (b"scheme", Field::Text, STRING + 49),
];

/// Whether libcurl answered, and the string it has, if any.
fn text(handle: PhpHandle<'_>, info: c_int) -> (bool, Option<Text<'_>>) {
    let mut value = None;
    (swoole_curl_info_string(handle, info, &mut value), value)
}

fn long(handle: PhpHandle<'_>, info: c_int) -> Option<i64> {
    let mut value = 0;
    swoole_curl_info_long(handle, info, &mut value).then_some(value)
}

fn double(handle: PhpHandle<'_>, info: c_int) -> Option<f64> {
    let mut value = 0.0;
    swoole_curl_info_double(handle, info, &mut value).then_some(value)
}

fn offset(handle: PhpHandle<'_>, info: c_int) -> Option<i64> {
    let mut value = 0;
    swoole_curl_info_off_t(handle, info, &mut value).then_some(value)
}

/// One field of the full array, left out when libcurl does not answer.
fn field(array: &mut OwnedArray, handle: PhpHandle<'_>, key: &[u8], kind: Field, info: c_int) {
    match kind {
        Field::Text => {
            if let (true, value) = text(handle, info) {
                array.insert_bytes(
                    key,
                    Some(value.map_or(&[][..], |text| text.get().to_bytes())),
                );
            }
        }
        Field::Long => {
            if let Some(value) = long(handle, info) {
                array.insert_long(key, value);
            }
        }
        Field::Double => {
            if let Some(value) = double(handle, info) {
                array.insert_double(key, value);
            }
        }
        Field::Offset => {
            if let Some(value) = offset(handle, info) {
                array.insert_long(key, value);
            }
        }
    }
}

/// `create_certinfo`: one array per certificate, each `name:value` field under its name.
/// The name is what precedes the first colon in the field's first 63 bytes.
fn certificates(handle: PhpHandle<'_>) -> Option<OwnedArray> {
    let mut certs = Certs(std::ptr::null(), std::marker::PhantomData);
    let count = swoole_curl_info_certs(handle, &mut certs);
    if count < 0 {
        return None;
    }
    let mut list = OwnedArray::default();
    for index in 0..count {
        let mut fields = OwnedArray::default();
        let mut cursor = swoole_curl_cert_fields(certs, index);
        while let Some(entry) = swoole_curl_list_next(&mut cursor) {
            let entry = entry.get().to_bytes();
            let head = entry.split_at(entry.len().min(63)).0;
            match head.iter().position(|&byte| byte == b':') {
                Some(colon) => {
                    let (name, rest) = entry.split_at(colon);
                    fields.insert_bytes(name, Some(rest.split_at(1).1));
                }
                None => {
                    warn(
                        E_WARNING,
                        c"Could not extract hash key from certificate info",
                    );
                }
            }
        }
        list.push(fields.into_owned());
    }
    Some(list)
}

/// Every field, in upstream's order for the PHP version and the libcurl headers built against.
fn all(handle: PhpHandle<'_>) -> OwnedArray {
    let mut array = OwnedArray::default();
    field(&mut array, handle, b"url", Field::Text, STRING + 1);
    if let (true, value) = text(handle, CONTENT_TYPE) {
        array.insert_bytes(b"content_type", value.map(|text| text.get().to_bytes()));
    }
    for &(key, kind, info) in HEAD {
        field(&mut array, handle, key, kind, info);
    }
    if let Some(certs) = certificates(handle) {
        array.insert(b"certinfo", certs.into_owned());
    }
    for &(key, kind, info) in TAIL {
        field(&mut array, handle, key, kind, info);
    }
    let php84 = PHP_VERSION_ID >= 80400;
    let php85 = PHP_VERSION_ID >= 80500;
    let mut timed = |key: &[u8], info: c_int, present: bool| {
        if present {
            field(&mut array, handle, key, Field::Offset, info);
        }
    };
    timed(b"appconnect_time_us", OFF_T + 56, true);
    timed(b"queue_time_us", OFF_T + 65, php85 && libcurl(8, 6, 0));
    timed(b"connect_time_us", OFF_T + 52, true);
    timed(b"namelookup_time_us", OFF_T + 51, true);
    timed(b"pretransfer_time_us", OFF_T + 53, true);
    timed(b"redirect_time_us", OFF_T + 55, true);
    timed(b"starttransfer_time_us", OFF_T + 54, true);
    timed(
        b"posttransfer_time_us",
        OFF_T + 67,
        php84 && libcurl(8, 10, 0),
    );
    timed(b"total_time_us", OFF_T + 50, true);
    if let Some(header) = swoole_curl_header_out(handle) {
        array.insert_bytes(b"request_header", Some(header.bytes()));
    }
    if libcurl(7, 72, 0) {
        field(
            &mut array,
            handle,
            b"effective_method",
            Field::Text,
            STRING + 58,
        );
    }
    if libcurl(7, 84, 0) && PHP_VERSION_ID >= 80300 {
        field(&mut array, handle, b"capath", Field::Text, STRING + 62);
        field(&mut array, handle, b"cainfo", Field::Text, STRING + 61);
    }
    if php85 {
        if libcurl(8, 7, 0) {
            field(&mut array, handle, b"used_proxy", Field::Long, LONG + 66);
        }
        if libcurl(8, 12, 0) {
            field(&mut array, handle, b"httpauth_used", Field::Long, LONG + 69);
            field(
                &mut array,
                handle,
                b"proxyauth_used",
                Field::Long,
                LONG + 70,
            );
        }
        if libcurl(8, 2, 0) {
            field(&mut array, handle, b"conn_id", Field::Offset, OFF_T + 64);
        }
    }
    array
}

/// One `CURLINFO_*` value, or false when libcurl has none.
fn one(call: &Call<'_>, handle: PhpHandle<'_>, info: c_int) {
    match info {
        HEADER_OUT => match swoole_curl_header_out(handle) {
            Some(header) => call.set_zstring(header.to_owned()),
            None => call.set_bool(false),
        },
        CERTINFO => match certificates(handle) {
            Some(certs) => call.set_owned(certs.into_owned()),
            None => call.set_bool(false),
        },
        PRIVATE => {
            let private = swoole_curl_private(handle);
            if private.kind() == swoole_rs_zend::sys::IS_UNDEF {
                call.set_bool(false);
            } else {
                call.set_owned(private.copy());
            }
        }
        _ => match info & TYPE_MASK {
            STRING => match text(handle, info) {
                (true, Some(value)) => call.set_bytes(value.get().to_bytes()),
                _ => call.set_bool(false),
            },
            LONG => match long(handle, info) {
                Some(value) => call.set_long(value),
                None => call.set_bool(false),
            },
            DOUBLE => match double(handle, info) {
                Some(value) => call.set_owned(swoole_rs_zend::Owned::double(value)),
                None => call.set_bool(false),
            },
            SLIST => {
                let mut list = List(None);
                if !swoole_curl_info_list(handle, info, &mut list) {
                    return call.set_bool(false);
                }
                let mut array = OwnedArray::default();
                let start = list
                    .0
                    .map_or(std::ptr::null(), |head| head.as_ptr().cast_const());
                let mut cursor = Cursor(start, std::marker::PhantomData);
                while let Some(entry) = swoole_curl_list_next(&mut cursor) {
                    array.push_bytes(entry.get().to_bytes());
                }
                call.set_owned(array.into_owned());
            }
            OFF_T => match offset(handle, info) {
                Some(value) => call.set_long(value),
                None => call.set_bool(false),
            },
            _ => call.set_bool(false),
        },
    }
}

fn getinfo(call: &Call<'_>) {
    if call.arity(1, 2).is_err() {
        return;
    }
    let Some(handle) = handle_argument(call) else {
        return;
    };
    let option = match call.value(2) {
        Some(value) if !value.is_null() => match call.long(2, 0) {
            Ok(option) => Some(option),
            Err(_) => return,
        },
        _ => None,
    };
    match option {
        None => call.set_owned(all(handle).into_owned()),
        // An option outside `int` names no information.
        Some(option) => match c_int::try_from(option) {
            Ok(info) => one(call, handle, info),
            Err(_) => call.set_bool(false),
        },
    }
}
