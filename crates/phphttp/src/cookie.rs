//! PHP `Swoole\Http\Cookie`: class registration, objects and every method, and
//! the `Set-Cookie` line `Swoole\Http\Response::cookie()` sends. The line is
//! built by `swr_v1_httpbindings_cookie`'s phases; this file runs their effects.
use std::ffi::{CStr, c_int};
use swoole_rs_httpbindings::{
    APPEND, APPEND_AGE, APPEND_DATE, APPEND_ENCODED, Action, CookieInput, DONE, ENCODE,
    FORMAT_DATE, FREE_DATE, FREE_ENCODED, READ_AGE, Text, WARN, swr_v1_httpbindings_cookie,
};
use swoole_rs_phpstubs::http_cookie;
use swoole_rs_zend::{
    Call, Class, Object, Owned, OwnedArray, ZStr, ZString,
    sys::{
        ZEND_ACC_NOT_SERIALIZABLE, php_format_date, php_time, php_url_encode, zend_class_entry,
        zend_object, zend_object_handlers, zval,
    },
    throw_constructed_twice, unset_property_deny,
};
use swoole_rs_zend::{method_table, sys as z};

/// `SW_ERROR_HTTP_COOKIE_UNAVAILABLE`.
const COOKIE_UNAVAILABLE: c_int = 7107;

static COOKIE: Class = Class::new();

/// `swoole::http::Cookie`. String fields are absent or non-empty.
struct Cookie {
    /// name, value, path, domain, sameSite, priority (the phases' field order).
    fields: [Option<ZString>; 6],
    expires: i64,
    encode: bool,
    secure: bool,
    http_only: bool,
    partitioned: bool,
}

const NAME: usize = 0;
const VALUE: usize = 1;
const PATH: usize = 2;
const DOMAIN: usize = 3;
const SAME_SITE: usize = 4;
const PRIORITY: usize = 5;
const KEYS: [&CStr; 6] = [
    c"name",
    c"value",
    c"path",
    c"domain",
    c"sameSite",
    c"priority",
];

impl Cookie {
    const fn new(encode: bool) -> Self {
        Self {
            fields: [const { None }; 6],
            expires: 0,
            encode,
            secure: false,
            http_only: false,
            partitioned: false,
        }
    }

    /// `HTTP_COOKIE_WITH_STR`: keep a reference to a non-empty string, else none.
    fn set(&mut self, field: usize, string: Option<&ZStr>) {
        if let Some(slot) = self.fields.get_mut(field) {
            *slot = string
                .filter(|string| !string.bytes().is_empty())
                .map(ZStr::to_owned);
        }
    }

    /// `Cookie::reset`, which also turns encoding back on.
    fn reset(&mut self) {
        for field in 0..self.fields.len() {
            self.set(field, None);
        }
        self.expires = 0;
        self.encode = true;
        self.secure = false;
        self.http_only = false;
        self.partitioned = false;
    }

    /// `Cookie::toString`: the line, or None after a warning.
    fn line(&self) -> Option<ZString> {
        let mut line = Vec::new();
        let mut encoded: Option<ZString> = None;
        let mut date: Option<ZString> = None;
        let mut age = 0.0;
        let mut phase = 0;
        loop {
            let mut input = CookieInput {
                fields: [Text::default(); 6],
                expires: self.expires,
                flags: u32::from(self.encode)
                    | u32::from(self.secure) << 1
                    | u32::from(self.http_only) << 2
                    | u32::from(self.partitioned) << 3,
                age,
            };
            for (text, field) in input.fields.iter_mut().zip(&self.fields) {
                if let Some(field) = field {
                    *text = Text {
                        data: field.bytes().as_ptr(),
                        length: field.bytes().len(),
                    };
                }
            }
            let mut action = Action::default();
            // SAFETY: live, disjoint locals; the fields outlive the call.
            if unsafe {
                swr_v1_httpbindings_cookie(&raw const input, &raw mut phase, &raw mut action)
            } != 0
            {
                return None;
            }
            match action.kind {
                APPEND => {
                    // SAFETY: the phases append bytes of a field or a static literal.
                    line.extend_from_slice(unsafe { text(&action.text) });
                }
                WARN => {
                    // SAFETY: a NUL-terminated static message.
                    let message = unsafe { CStr::from_ptr(action.text.data.cast()) };
                    crate::host::notice(&message.to_string_lossy());
                    return None;
                }
                ENCODE => {
                    // The phases encode only a present value.
                    let value = self.fields[VALUE]
                        .as_ref()
                        .map_or(&[][..], |value| value.bytes());
                    // SAFETY: Zend reads the bytes and returns a new string.
                    let string = unsafe { php_url_encode(value.as_ptr().cast(), value.len()) };
                    // SAFETY: that new string's reference.
                    encoded = Some(unsafe { ZString::from_raw(string) });
                }
                APPEND_ENCODED => {
                    line.extend_from_slice(encoded.as_ref().map_or(&[][..], |text| text.bytes()));
                }
                FREE_ENCODED => encoded = None,
                FORMAT_DATE => {
                    let format = c"D, d-M-Y H:i:s T";
                    // SAFETY: a literal format and a timestamp; Zend returns a new string.
                    let string = unsafe {
                        php_format_date(format.as_ptr(), format.count_bytes(), action.value, false)
                    };
                    // SAFETY: that new string's reference.
                    date = Some(unsafe { ZString::from_raw(string) });
                }
                APPEND_DATE => {
                    line.extend_from_slice(date.as_ref().map_or(&[][..], |text| text.bytes()));
                }
                FREE_DATE => date = None,
                READ_AGE => {
                    // SAFETY: PHP's clock.
                    let now = unsafe { php_time() };
                    age = difftime(self.expires, now);
                }
                APPEND_AGE => {
                    line.extend_from_slice(action.value.to_string().as_bytes());
                }
                DONE => return Some(ZString::new(&line)),
                _ => return None,
            }
        }
    }

    /// `Cookie::toArray`, each string field with its own reference.
    fn to_array(&self) -> OwnedArray {
        let mut array = OwnedArray::default();
        for (key, field) in KEYS.iter().zip(&self.fields) {
            match field {
                Some(field) => array.insert(key.to_bytes(), Owned::string(ZStr::to_owned(field))),
                None => array.insert_bytes(key.to_bytes(), Some(b"")),
            }
        }
        array.insert_bool(b"encode", self.encode);
        array.insert_long(b"expires", self.expires);
        array.insert_bool(b"secure", self.secure);
        array.insert_bool(b"httpOnly", self.http_only);
        array.insert_bool(b"partitioned", self.partitioned);
        array
    }
}

/// `difftime(end, start)`.
#[expect(clippy::cast_precision_loss, reason = "difftime returns a double")]
fn difftime(end: i64, start: i64) -> f64 {
    end.wrapping_sub(start) as f64
}

/// The bytes a phase action names.
/// # Safety
/// `text` spans live bytes.
unsafe fn text<'a>(text: &Text) -> &'a [u8] {
    if text.length == 0 {
        return &[];
    }
    // SAFETY: per the caller.
    unsafe { std::slice::from_raw_parts(text.data, text.length) }
}

/// `struct HttpCookieObject { Cookie *cookie; zend_object std; }`.
type CookieObject = Object<Option<Box<Cookie>>>;

unsafe extern "C" fn create(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: Zend calls `create_object` with this class or a subclass of it;
    // zeroed data is no cookie.
    unsafe { CookieObject::create(&COOKIE, ce) }
}

unsafe extern "C" fn free(std: *mut zend_object) {
    // SAFETY: Zend frees an object this class created.
    drop(unsafe { CookieObject::data(std) }.take());
    // SAFETY: as above.
    unsafe { CookieObject::dtor(std) };
}

method_table! {
    static METHODS = [
        c"__construct" => zim_swoole_http_cookie___construct, &http_cookie::arginfo_class_Swoole_Http_Cookie___construct, z::ZEND_ACC_PUBLIC;
        c"withName" => zim_swoole_http_cookie_withName, &http_cookie::arginfo_class_Swoole_Http_Cookie_withName, z::ZEND_ACC_PUBLIC;
        c"withValue" => zim_swoole_http_cookie_withValue, &http_cookie::arginfo_class_Swoole_Http_Cookie_withValue, z::ZEND_ACC_PUBLIC;
        c"withExpires" => zim_swoole_http_cookie_withExpires, &http_cookie::arginfo_class_Swoole_Http_Cookie_withExpires, z::ZEND_ACC_PUBLIC;
        c"withPath" => zim_swoole_http_cookie_withPath, &http_cookie::arginfo_class_Swoole_Http_Cookie_withPath, z::ZEND_ACC_PUBLIC;
        c"withDomain" => zim_swoole_http_cookie_withDomain, &http_cookie::arginfo_class_Swoole_Http_Cookie_withDomain, z::ZEND_ACC_PUBLIC;
        c"withSecure" => zim_swoole_http_cookie_withSecure, &http_cookie::arginfo_class_Swoole_Http_Cookie_withSecure, z::ZEND_ACC_PUBLIC;
        c"withHttpOnly" => zim_swoole_http_cookie_withHttpOnly, &http_cookie::arginfo_class_Swoole_Http_Cookie_withHttpOnly, z::ZEND_ACC_PUBLIC;
        c"withSameSite" => zim_swoole_http_cookie_withSameSite, &http_cookie::arginfo_class_Swoole_Http_Cookie_withSameSite, z::ZEND_ACC_PUBLIC;
        c"withPriority" => zim_swoole_http_cookie_withPriority, &http_cookie::arginfo_class_Swoole_Http_Cookie_withPriority, z::ZEND_ACC_PUBLIC;
        c"withPartitioned" => zim_swoole_http_cookie_withPartitioned, &http_cookie::arginfo_class_Swoole_Http_Cookie_withPartitioned, z::ZEND_ACC_PUBLIC;
        c"toString" => zim_swoole_http_cookie_toString, &http_cookie::arginfo_class_Swoole_Http_Cookie_toString, z::ZEND_ACC_PUBLIC;
        c"toArray" => zim_swoole_http_cookie_toArray, &http_cookie::arginfo_class_Swoole_Http_Cookie_toArray, z::ZEND_ACC_PUBLIC;
        c"reset" => zim_swoole_http_cookie_reset, &http_cookie::arginfo_class_Swoole_Http_Cookie_reset, z::ZEND_ACC_PUBLIC;
    ];
}

/// `php_swoole_http_cookie_minit`: register the class.
/// # Safety
/// Called once, at MINIT.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn php_swoole_http_cookie_minit(_module_number: c_int) {
    // SAFETY: MINIT, with a static table.
    unsafe { COOKIE.register(c"Swoole\\Http\\Cookie", METHODS.as_ptr(), configure) };
}

/// Not serializable, clone and unset denied, custom object.
fn configure(entry: &mut zend_class_entry, handlers: &mut zend_object_handlers) {
    entry.ce_flags |= ZEND_ACC_NOT_SERIALIZABLE;
    handlers.clone_obj = None;
    handlers.unset_property = Some(unset_property_deny);
    CookieObject::install(entry, handlers, create, free);
}

/// `php_swoole_http_get_cooke_safety`: the object's cookie, or Swoole's
/// `SW_ERROR_HTTP_COOKIE_UNAVAILABLE` (upstream then dereferences null).
fn cookie<'a>(object: *mut zend_object) -> Option<&'a mut Cookie> {
    // SAFETY: a Cookie object (methods' `$this`, or Response's typed argument).
    let cookie = unsafe { CookieObject::data(object) }.as_deref_mut();
    if cookie.is_none() {
        crate::host::set_last_error(COOKIE_UNAVAILABLE);
    }
    cookie
}

/// `__construct(bool $encode = true)`.
fn construct(call: &Call) {
    let Ok(encode) = call.arity(0, 1).and_then(|()| call.bool(1, true)) else {
        return;
    };
    // SAFETY: Cookie methods are non-static, so `$this` is one of our objects.
    let slot = unsafe { CookieObject::data(call.this()) };
    if slot.is_some() {
        // SAFETY: in a handler, with `$this` live.
        unsafe { throw_constructed_twice(call.this()) };
        call.set_bool(false);
        return;
    }
    *slot = Some(Box::new(Cookie::new(encode)));
}

/// `with<Field>(string $field)`: set a string field and return `$this`.
fn with_string(call: &Call, field: usize) {
    let cookie = cookie(call.this());
    let Ok(string) = call.arity(1, 1).and_then(|()| call.zstr(1)) else {
        return;
    };
    if let Some(cookie) = cookie {
        cookie.set(field, string);
    }
    call.set_this();
}

/// `with<Flag>(bool $flag = false)`: set a flag and return `$this`.
fn with_flag(call: &Call, flag: fn(&mut Cookie) -> &mut bool) {
    let cookie = cookie(call.this());
    let Ok(value) = call.arity(0, 1).and_then(|()| call.bool(1, false)) else {
        return;
    };
    if let Some(cookie) = cookie {
        *flag(cookie) = value;
    }
    call.set_this();
}

fn with_name(call: &Call) {
    with_string(call, NAME);
}
fn with_value(call: &Call) {
    with_string(call, VALUE);
}
fn with_path(call: &Call) {
    with_string(call, PATH);
}
fn with_domain(call: &Call) {
    with_string(call, DOMAIN);
}
fn with_same_site(call: &Call) {
    with_string(call, SAME_SITE);
}
fn with_priority(call: &Call) {
    with_string(call, PRIORITY);
}
fn with_secure(call: &Call) {
    with_flag(call, |cookie| &mut cookie.secure);
}
fn with_http_only(call: &Call) {
    with_flag(call, |cookie| &mut cookie.http_only);
}
fn with_partitioned(call: &Call) {
    with_flag(call, |cookie| &mut cookie.partitioned);
}

/// `withExpires(int $expires = 0)`.
fn with_expires(call: &Call) {
    let cookie = cookie(call.this());
    let Ok(expires) = call.arity(0, 1).and_then(|()| call.long(1, 0)) else {
        return;
    };
    if let Some(cookie) = cookie {
        cookie.expires = expires;
    }
    call.set_this();
}

/// `toString(): string|false`; a cookie that cannot be sent is reset.
fn to_string(call: &Call) {
    let Some(cookie) = cookie(call.this()) else {
        return;
    };
    if let Some(line) = cookie.line() {
        call.set_zstring(line);
    } else {
        cookie.reset();
        call.set_bool(false);
    }
}

/// `toArray(): array`.
fn to_array(call: &Call) {
    if let Some(cookie) = cookie(call.this()) {
        call.set_owned(cookie.to_array().into_owned());
    }
}

/// `reset(): void`.
fn reset(call: &Call) {
    if let Some(cookie) = cookie(call.this()) {
        cookie.reset();
    }
}

swoole_rs_zend::handlers! {
    zim_swoole_http_cookie___construct => construct;
    zim_swoole_http_cookie_withName => with_name;
    zim_swoole_http_cookie_withValue => with_value;
    zim_swoole_http_cookie_withExpires => with_expires;
    zim_swoole_http_cookie_withPath => with_path;
    zim_swoole_http_cookie_withDomain => with_domain;
    zim_swoole_http_cookie_withSecure => with_secure;
    zim_swoole_http_cookie_withHttpOnly => with_http_only;
    zim_swoole_http_cookie_withSameSite => with_same_site;
    zim_swoole_http_cookie_withPriority => with_priority;
    zim_swoole_http_cookie_withPartitioned => with_partitioned;
    zim_swoole_http_cookie_toArray => to_array;
    zim_swoole_http_cookie_toString => to_string;
    zim_swoole_http_cookie_reset => reset;
}

/// `Response::cookie()`/`rawcookie()` with a name: the line of a temporary
/// cookie built from the arguments, or None after a warning. `strings` are the
/// name, value, path, domain, `SameSite` and priority; `flags` are secure,
/// `HttpOnly` and partitioned.
pub(crate) fn header_line(
    encode: bool,
    strings: [Option<&ZStr>; 6],
    expires: i64,
    (secure, http_only, partitioned): (bool, bool, bool),
) -> Option<ZString> {
    let mut cookie = Cookie::new(encode);
    for (field, string) in strings.into_iter().enumerate() {
        cookie.set(field, string);
    }
    cookie.expires = expires;
    cookie.secure = secure;
    cookie.http_only = http_only;
    cookie.partitioned = partitioned;
    cookie.line()
}

/// `Response::cookie()` with a `Swoole\Http\Cookie`: its line, or None (after a
/// warning, which also resets the cookie, or for an unconstructed cookie).
/// # Safety
/// `object` is a live `Swoole\Http\Cookie` zval.
pub(crate) unsafe fn object_header_line(object: *const zval) -> Option<ZString> {
    // SAFETY: per the caller, a live zval.
    let value = unsafe { (*object).value };
    // SAFETY: an object zval holds `obj`.
    let std = unsafe { value.obj };
    let cookie = cookie(std)?;
    let line = cookie.line();
    if line.is_none() {
        cookie.reset();
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn difftime_is_the_exact_difference() {
        assert!(
            (difftime(100, 40) - 60.0).abs() < f64::EPSILON,
            "seconds left"
        );
        assert!((difftime(0, 5) + 5.0).abs() < f64::EPSILON, "expired");
    }

    #[test]
    fn fields_follow_the_phase_order() {
        assert_eq!(
            KEYS.map(CStr::to_bytes),
            [
                &b"name"[..],
                b"value",
                b"path",
                b"domain",
                b"sameSite",
                b"priority"
            ],
            "toArray keys match the phases' field order"
        );
    }
}
