//! PHP `Swoole\Http\Cookie`: class registration, objects and every method, and
//! the `Set-Cookie` line `Swoole\Http\Response::cookie()` sends.
use std::{cell::RefCell, ffi::CStr};
use swoole_rs_phpstubs::http_cookie;
use swoole_rs_phpswoole::MinitArgs;
use swoole_rs_zend::{Call, Custom, CustomClass, Owned, OwnedArray, Value, ZStr, ZString};
use swoole_rs_zend::{method_table, sys as z};

/// `SW_ERROR_HTTP_COOKIE_UNAVAILABLE`.
const COOKIE_UNAVAILABLE: std::ffi::c_int = 7107;

static COOKIE: CustomClass<Held> = CustomClass::new();

/// `struct HttpCookieObject { Cookie *cookie; zend_object std; }`: no cookie
/// until `__construct` runs. Borrows never span PHP code.
struct Held(RefCell<Option<Cookie>>);

impl Custom for Held {
    fn class() -> &'static CustomClass<Self> {
        &COOKIE
    }

    fn new() -> Self {
        Self(RefCell::new(None))
    }
}

impl Held {
    /// `f` on the constructed cookie.
    fn with<R>(&self, f: impl FnOnce(&mut Cookie) -> R) -> Option<R> {
        self.0.borrow_mut().as_mut().map(f)
    }

    /// `Cookie::toString`: the line, or None after a warning, which resets the
    /// cookie. The warning may run PHP code, so it formats a copy.
    fn line(&self) -> Option<ZString> {
        let line = self.with(|cookie| cookie.clone())?.line();
        if line.is_none() {
            self.with(Cookie::reset);
        }
        line
    }
}

/// `swoole::http::Cookie`. String fields are absent or non-empty.
#[derive(Clone)]
struct Cookie {
    /// name, value, path, domain, sameSite, priority.
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

/// The bytes no name, value, path or domain may hold.
const ILLEGAL: &[u8] = b",; \t\r\n\x0b\x0c";
/// How the warnings list them.
const LISTED: &str = r#"",", ";", " ", "\t", "\r", "\n", "\013", or "\014""#;

/// Whether `bytes` up to its first NUL (upstream's `strpbrk`) hold an illegal
/// byte, or `=` in a `name`.
fn illegal(bytes: &[u8], name: bool) -> bool {
    bytes
        .iter()
        .take_while(|&&c| c != 0)
        .any(|c| ILLEGAL.contains(c) || (name && *c == b'='))
}

/// Warn and give no line.
fn warn(message: &str) -> Option<ZString> {
    crate::host::notice(message);
    None
}

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
        *self = Self::new(true);
    }

    /// `Cookie::toString`: the line, or None after a warning.
    fn line(&self) -> Option<ZString> {
        const DATE: &CStr = c"D, d-M-Y H:i:s T";
        let [name, value, path, domain, same_site, priority] = self
            .fields
            .each_ref()
            .map(|field| field.as_ref().map_or(&[][..], |field| field.bytes()));
        if name.is_empty() {
            return warn("The name cannot be empty");
        }
        if illegal(name, true) {
            return warn(&format!("The name cannot contain \"=\", {LISTED}"));
        }
        let mut line = name.to_vec();
        if value.is_empty() {
            line.extend_from_slice(b"=deleted; expires=");
            line.extend_from_slice(ZString::format_date(DATE, 1, false).bytes());
            line.extend_from_slice(b"; Max-Age=0");
        } else {
            if !self.encode && illegal(value, false) {
                return warn(&format!("The value cannot contain {LISTED}"));
            }
            line.push(b'=');
            if self.encode {
                line.extend_from_slice(ZString::url_encode(value).bytes());
            } else {
                line.extend_from_slice(value);
            }
            if self.expires > 0 {
                if self.expires >= 253_402_300_800 {
                    return warn("The expires cannot have a year greater than 9999");
                }
                line.extend_from_slice(b"; expires=");
                line.extend_from_slice(ZString::format_date(DATE, self.expires, false).bytes());
                let age = self.expires.saturating_sub(swoole_rs_zend::time()).max(0);
                line.extend_from_slice(format!("; Max-Age={age}").as_bytes());
            }
        }
        for (option, bytes) in [("path", path), ("domain", domain)] {
            if !bytes.is_empty() {
                if illegal(bytes, false) {
                    return warn(&format!("The {option} option cannot contain {LISTED}"));
                }
                line.extend_from_slice(format!("; {option}=").as_bytes());
                line.extend_from_slice(bytes);
            }
        }
        for (on, attribute) in [(self.secure, "secure"), (self.http_only, "HttpOnly")] {
            if on {
                line.extend_from_slice(format!("; {attribute}").as_bytes());
            }
        }
        for (attribute, bytes) in [("SameSite", same_site), ("Priority", priority)] {
            if !bytes.is_empty() {
                line.extend_from_slice(format!("; {attribute}=").as_bytes());
                line.extend_from_slice(bytes);
            }
        }
        if self.partitioned {
            line.extend_from_slice(b"; Partitioned");
        }
        Some(ZString::new(&line))
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

/// `php_swoole_http_cookie_minit`: register the class (not serializable,
/// clone and unset denied, custom object).
pub fn php_swoole_http_cookie_minit(args: &MinitArgs) {
    let minit = args.minit;
    minit.register_object::<Held>(c"Swoole\\Http\\Cookie", &METHODS.0, 0);
}

/// `php_swoole_http_get_cooke_safety`: a constructed cookie's object, or
/// Swoole's `SW_ERROR_HTTP_COOKIE_UNAVAILABLE` (upstream then dereferences null).
fn constructed(held: Option<&Held>) -> Option<&Held> {
    let held = held.filter(|held| held.0.borrow().is_some());
    if held.is_none() {
        crate::host::set_last_error(COOKIE_UNAVAILABLE);
    }
    held
}

/// `$this`'s constructed cookie.
fn cookie(call: &Call) -> Option<&Held> {
    constructed(call.custom())
}

/// `__construct(bool $encode = true)`.
fn construct(call: &Call) {
    let Ok(encode) = call.arity(0, 1).and_then(|()| call.bool(1, true)) else {
        return;
    };
    let Some(Held(cookie)) = call.custom() else {
        return;
    };
    if cookie.borrow().is_some() {
        call.throw_constructed_twice();
        call.set_bool(false);
        return;
    }
    *cookie.borrow_mut() = Some(Cookie::new(encode));
}

/// `with<Field>(string $field)`: set a string field and return `$this`.
fn with_string(call: &Call, field: usize) {
    let held = cookie(call);
    let Ok(string) = call.arity(1, 1).and_then(|()| call.zstr(1)) else {
        return;
    };
    if let Some(held) = held {
        held.with(|cookie| cookie.set(field, string));
    }
    call.set_this();
}

/// `with<Flag>(bool $flag = false)`: set a flag and return `$this`.
fn with_flag(call: &Call, flag: fn(&mut Cookie) -> &mut bool) {
    let held = cookie(call);
    let Ok(value) = call.arity(0, 1).and_then(|()| call.bool(1, false)) else {
        return;
    };
    if let Some(held) = held {
        held.with(|cookie| *flag(cookie) = value);
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
    let held = cookie(call);
    let Ok(expires) = call.arity(0, 1).and_then(|()| call.long(1, 0)) else {
        return;
    };
    if let Some(held) = held {
        held.with(|cookie| cookie.expires = expires);
    }
    call.set_this();
}

/// `toString(): string|false`; a cookie that cannot be sent is reset.
fn to_string(call: &Call) {
    let Some(held) = cookie(call) else {
        return;
    };
    match held.line() {
        Some(line) => call.set_zstring(line),
        None => call.set_bool(false),
    }
}

/// `toArray(): array`.
fn to_array(call: &Call) {
    if let Some(array) = cookie(call).and_then(|held| held.with(|cookie| cookie.to_array())) {
        call.set_owned(array.into_owned());
    }
}

/// `reset(): void`.
fn reset(call: &Call) {
    if let Some(held) = cookie(call) {
        held.with(Cookie::reset);
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

/// `Response::cookie()` with an object: a constructed `Swoole\Http\Cookie`'s
/// line, or None (after a warning, which also resets the cookie, or for any
/// other object).
pub(crate) fn object_header_line(object: &Value) -> Option<ZString> {
    constructed(object.custom())?.line()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn illegal_bytes_stop_at_nul() {
        assert!(illegal(b"a;b", false), "a separator");
        assert!(illegal(b"a=b", true), "= in a name");
        assert!(!illegal(b"a=b", false), "= in a value");
        assert!(!illegal(b"a\0;", false), "after a NUL");
    }
}
