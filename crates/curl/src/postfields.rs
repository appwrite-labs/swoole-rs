//! `CURLOPT_POSTFIELDS` (#5131): a string is the body as given; an array becomes a
//! multipart body, one part per field, a `CURLFile` read from its file and a
//! `CURLStringFile` sent as a file from its data. `curl_copy_handle` builds the
//! parts again for the copy from the fields a file upload keeps. libcurl's mime
//! calls and the file parts' stream callbacks are in `native/hooks/curl_handle.cc`.
use super::php_handle::PhpHandle;
use super::setopt::{
    swoole_curl_allowed_path, swoole_curl_save_error, swoole_curl_setopt_long,
    swoole_curl_setopt_string, warn,
};
use std::ffi::{CString, c_int, c_long};
use swoole_rs_zend::{ClassEntry, E_WARNING, Key, Owned, Value, ZStr, ZString, bailout};

unsafe extern "C" {
    safe fn swoole_curl_file_class() -> ClassEntry<'static>;
    safe fn swoole_curl_string_file_class() -> ClassEntry<'static>;
    safe fn swoole_curl_mime_new(handle: PhpHandle<'_>) -> Mime;
    safe fn swoole_curl_mime_free(mime: Mime);
    // Each part's builder returns its `CURLcode`, or -1 when libcurl gave no part.
    safe fn swoole_curl_mime_field(mime: Parts<'_>, name: &ZStr, data: &ZStr) -> c_int;
    safe fn swoole_curl_mime_string_file(
        mime: Parts<'_>,
        name: &ZStr,
        data: &ZStr,
        filename: &ZStr,
        mime_type: &ZStr,
    ) -> c_int;
    safe fn swoole_curl_mime_file(
        handle: PhpHandle<'_>,
        mime: Parts<'_>,
        name: &ZStr,
        path: &ZStr,
        filename: Option<&ZStr>,
        mime_type: Option<&ZStr>,
    ) -> c_int;
    // Sets the multipart body and gives it to the handle, which frees it.
    safe fn swoole_curl_setopt_mime(handle: PhpHandle<'_>, mime: Mime) -> c_int;
    safe fn swoole_curl_set_empty_post(handle: PhpHandle<'_>) -> c_int;
    safe fn swoole_curl_postfields<'a>(handle: PhpHandle<'a>) -> &'a mut Owned;
}

/// `CURLOPT_POSTFIELDSIZE` and `CURLOPT_COPYPOSTFIELDS`.
const CURLOPT_POSTFIELDSIZE: c_int = 60;
const CURLOPT_COPYPOSTFIELDS: c_int = 10165;

/// A `curl_mime` being built, freed unless [`swoole_curl_setopt_mime`] takes it.
#[repr(transparent)]
struct Mime(Option<std::ptr::NonNull<std::ffi::c_void>>);

impl Mime {
    /// The `curl_mime *` to add parts to, while this owns it.
    fn parts(&self) -> Option<Parts<'_>> {
        self.0.map(|mime| Parts(mime, std::marker::PhantomData))
    }
}

/// A [`Mime`]'s `curl_mime *`, borrowed to add parts.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct Parts<'a>(
    std::ptr::NonNull<std::ffi::c_void>,
    std::marker::PhantomData<&'a Mime>,
);

impl Drop for Mime {
    fn drop(&mut self) {
        if self.0.is_some() {
            swoole_curl_mime_free(Self(self.0.take()));
        }
    }
}

/// `CURLOPT_POSTFIELDS`: whether it was set.
pub(super) fn set(handle: PhpHandle<'_>, value: &Value) -> bool {
    if let Some(fields) = value.array() {
        return if fields.count() == 0 {
            swoole_curl_set_empty_post(handle) == 0
        } else {
            build(handle, value)
        };
    }
    let Some(body) = value.applied_string() else {
        return false;
    };
    // libcurl copies the body, of the size set first.
    swoole_curl_setopt_long(
        handle,
        CURLOPT_POSTFIELDSIZE,
        c_long::try_from(body.bytes().len()).unwrap_or(c_long::MAX),
    );
    swoole_curl_setopt_string(handle, CURLOPT_COPYPOSTFIELDS, Some(&body)) == 0
}

/// The copy `curl_copy_handle` made gets its own parts, built from the fields the
/// source kept for a file upload: whether that worked.
pub(super) fn copy(clone: PhpHandle<'_>, source: PhpHandle<'_>) -> bool {
    let kept = swoole_curl_postfields(source);
    kept.kind() == swoole_rs_zend::sys::IS_UNDEF || build(clone, kept)
}

/// `build_mime_structure_from_hash`: the multipart body from an array of fields.
fn build(handle: PhpHandle<'_>, fields: &Value) -> bool {
    // A field's conversion or a property read can run PHP code, which must not free
    // the table.
    let Some(held) = fields.hold() else {
        return false;
    };
    let Some(table) = held.array() else {
        return false;
    };
    if table.count() == 0 {
        // No parts: the body is unset.
        swoole_curl_save_error(handle, 0);
        return swoole_curl_setopt_mime(handle, Mime(None)) == 0;
    }
    let mime = swoole_curl_mime_new(handle);
    let Some(parts) = mime.parts() else {
        return false;
    };
    let mut error = 0;
    let mut entries = table.entries();
    while let Some((key, value)) = entries.next_keyed() {
        let name = match key {
            Key::Name(name) => ZString::new(name),
            Key::Index(index) => ZString::new(zend_long_text(index).as_bytes()),
        };
        let value = value.referred();
        let class = value.class_entry();
        let code = if class.is_some_and(|class| class.instance_of(swoole_curl_file_class())) {
            match file_part(handle, parts, &name, value, fields) {
                Some(code) => code,
                None => return false,
            }
        } else if class.is_some_and(|class| class.instance_of(swoole_curl_string_file_class())) {
            match string_file_part(handle, parts, &name, value, fields) {
                Some(code) => code,
                None => return false,
            }
        } else if let Some(values) = value.array() {
            // Each element is a field of the same name; their errors are ignored, as
            // upstream ignores them.
            let mut elements = values.entries();
            while let Some((_, element)) = elements.next_keyed() {
                let Some(data) = element.referred().applied_string() else {
                    return false;
                };
                swoole_curl_mime_field(parts, &name, &data);
            }
            0
        } else {
            let Some(data) = value.applied_string() else {
                return false;
            };
            swoole_curl_mime_field(parts, &name, &data);
            0
        };
        if code != 0 {
            error = code;
        }
    }
    swoole_curl_save_error(handle, error);
    error == 0 && swoole_curl_setopt_mime(handle, mime) == 0
}

/// A `CURLFile` field: the part's `CURLcode`, 0 after the warning of a filename
/// that isn't a string, or None to give up on the body.
fn file_part(
    handle: PhpHandle<'_>,
    mime: Parts<'_>,
    name: &ZString,
    file: &Value,
    fields: &Value,
) -> Option<c_int> {
    let class = swoole_curl_file_class();
    let read = |property: &std::ffi::CStr| match file.read_property_of(class, property) {
        Ok(read) => Some(read),
        Err(bailout) => {
            bailout::defer(Err(bailout));
            None
        }
    };
    let path = read(c"name")?;
    let Some(path) = path.as_ref().and_then(|path| path.referred().str()) else {
        let message = [&b"Invalid filename for key "[..], name.bytes()].concat();
        if !warn(E_WARNING, &CString::new(message).unwrap_or_default()) {
            return None;
        }
        return Some(0);
    };
    let mut allowed = false;
    bailout::defer(swoole_rs_zend::contain_with(|| {
        allowed = swoole_curl_allowed_path(path);
    }));
    if !allowed {
        return None;
    }
    let mime_type = read(c"mime")?;
    let filename = read(c"postname")?;
    keep(handle, fields);
    let code = swoole_curl_mime_file(
        handle,
        mime,
        name,
        path,
        non_empty(filename.as_ref()),
        non_empty(mime_type.as_ref()),
    );
    (code >= 0).then_some(code)
}

/// A `CURLStringFile` field, as [`file_part`]: its properties are typed strings.
fn string_file_part(
    handle: PhpHandle<'_>,
    mime: Parts<'_>,
    name: &ZString,
    file: &Value,
    fields: &Value,
) -> Option<c_int> {
    let class = swoole_curl_string_file_class();
    let read = |property: &std::ffi::CStr| {
        let read = file.read_property_of(class, property);
        let thrown = !swoole_rs_zend::host::exception().is_null();
        match read {
            Ok(Some(value)) if !thrown => Some(value),
            Ok(_) => None,
            Err(bailout) => {
                bailout::defer(Err(bailout));
                None
            }
        }
    };
    let filename = read(c"postname")?;
    let mime_type = read(c"mime")?;
    let data = read(c"data")?;
    let (Some(filename), Some(mime_type), Some(data)) = (
        filename.referred().str(),
        mime_type.referred().str(),
        data.referred().str(),
    ) else {
        return None;
    };
    keep(handle, fields);
    let code = swoole_curl_mime_string_file(mime, name, data, filename, mime_type);
    (code >= 0).then_some(code)
}

/// The handle keeps the fields of a file upload for `curl_copy_handle`; the ones it
/// kept before are released once the slot is no longer borrowed.
fn keep(handle: PhpHandle<'_>, fields: &Value) {
    let previous = std::mem::replace(swoole_curl_postfields(handle), fields.copy());
    drop(previous);
}

/// A property that is a non-empty string.
fn non_empty(property: Option<&Owned>) -> Option<&ZStr> {
    property
        .and_then(|property| property.referred().str())
        .filter(|text| !text.bytes().is_empty())
}

/// `zend_long_to_str` of an integer key, which Zend stores as unsigned.
fn zend_long_text(index: swoole_rs_zend::sys::zend_ulong) -> String {
    i64::from_ne_bytes(index.to_ne_bytes()).to_string()
}
