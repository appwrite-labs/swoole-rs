//! The bundled ext/ssh2's PHP functions (`thirdparty/php/ssh2/ssh2.cc`) as
//! they move to Rust: so far `ssh2_methods_negotiated` and `ssh2_fingerprint`.
//! The resources stay ext/ssh2's; their sessions are read through
//! `swr_ssh2_session`.
use crate::Session;
use std::ffi::{CString, c_int};
use swoole_rs_corebase::sys::Text;
use swoole_rs_zend::{Call, E_WARNING, Owned, OwnedArray, Value, ZString, bailout, report};

/// `LIBSSH2_METHOD_*`, the negotiated method kinds `ssh2_methods_negotiated` reads.
const METHOD_KEX: c_int = 0;
const METHOD_HOSTKEY: c_int = 1;
const METHOD_CRYPT_CS: c_int = 2;
const METHOD_CRYPT_SC: c_int = 3;
const METHOD_MAC_CS: c_int = 4;
const METHOD_MAC_SC: c_int = 5;
const METHOD_COMP_CS: c_int = 6;
const METHOD_COMP_SC: c_int = 7;
const METHOD_LANG_CS: c_int = 8;
const METHOD_LANG_SC: c_int = 9;
/// `LIBSSH2_HOSTKEY_HASH_MD5` and `LIBSSH2_HOSTKEY_HASH_SHA1`.
const HASH_MD5: c_int = 1;
const HASH_SHA1: c_int = 2;
/// `PHP_SSH2_FINGERPRINT_SHA1` and `PHP_SSH2_FINGERPRINT_RAW`.
const FINGERPRINT_SHA1: i64 = 0x0001;
const FINGERPRINT_RAW: i64 = 0x0002;

unsafe extern "C" {
    /// The session an `SSH2 Session` resource holds (`zend_fetch_resource`):
    /// null, with Zend's `TypeError` thrown, for a resource of another kind.
    safe fn swr_ssh2_session(resource: &Value) -> Session;
    /// libssh2's own declaration: the negotiated method of `kind`, which the
    /// session keeps, or null.
    safe fn libssh2_session_methods<'a>(session: Session, kind: c_int) -> Option<Text<'a>>;
    /// libssh2's own declaration for the MD5 hostkey hash: the session's 16
    /// bytes, or null.
    #[link_name = "libssh2_hostkey_hash"]
    safe fn hostkey_md5<'a>(session: Session, kind: c_int) -> Option<&'a [u8; 16]>;
    /// libssh2's own declaration for the SHA1 hostkey hash: the session's 20
    /// bytes, or null.
    #[expect(
        clashing_extern_declarations,
        reason = "libssh2_hostkey_hash returns a char *: each declaration types the hash it asks for"
    )]
    #[link_name = "libssh2_hostkey_hash"]
    safe fn hostkey_sha1<'a>(session: Session, kind: c_int) -> Option<&'a [u8; 20]>;
}

swoole_rs_zend::handlers! {
    zif_ssh2_methods_negotiated => methods_negotiated;
    zif_ssh2_fingerprint => fingerprint;
}

/// The session `resource` holds, or false returned (with Zend's error raised)
/// when it isn't an `SSH2 Session` resource.
fn session(call: &Call<'_>, resource: &Value) -> Option<Session> {
    let session = swr_ssh2_session(resource);
    if session.0.is_null() {
        call.set_bool(false);
        return None;
    }
    Some(session)
}

/// `php_error_docref(NULL, E_WARNING, "%s", message)`, a bailout deferred.
fn warn(message: &str) {
    if let Ok(message) = CString::new(message) {
        bailout::defer(report(E_WARNING, &message));
    }
}

/// `ssh2_methods_negotiated(resource $session): array|false`: the key exchange
/// and hostkey methods, and each direction's cipher, MAC, compression and
/// language. A method libssh2 has none for is null (C would have crashed).
fn methods_negotiated(call: &Call<'_>) {
    if call.arity(1, 1).is_err() {
        return;
    }
    let Ok(resource) = call.resource(1) else {
        return;
    };
    let Some(session) = session(call, resource) else {
        return;
    };
    let method = |kind| libssh2_session_methods(session, kind).map(|text| text.get().to_bytes());
    let direction = |crypt, mac, comp, lang| {
        let mut endpoint = OwnedArray::default();
        endpoint.insert_bytes(b"crypt", method(crypt));
        endpoint.insert_bytes(b"mac", method(mac));
        endpoint.insert_bytes(b"comp", method(comp));
        endpoint.insert_bytes(b"lang", method(lang));
        endpoint.into_owned()
    };
    let mut methods = OwnedArray::default();
    methods.insert_bytes(b"kex", method(METHOD_KEX));
    methods.insert_bytes(b"hostkey", method(METHOD_HOSTKEY));
    methods.insert(
        b"client_to_server",
        direction(
            METHOD_CRYPT_CS,
            METHOD_MAC_CS,
            METHOD_COMP_CS,
            METHOD_LANG_CS,
        ),
    );
    methods.insert(
        b"server_to_client",
        direction(
            METHOD_CRYPT_SC,
            METHOD_MAC_SC,
            METHOD_COMP_SC,
            METHOD_LANG_SC,
        ),
    );
    call.set_owned(methods.into_owned());
}

/// `ssh2_fingerprint(resource $session, int $flags = 0): string|false|null`:
/// the server's hostkey hash, MD5 unless `SSH2_FINGERPRINT_SHA1`, as uppercase
/// hex unless `SSH2_FINGERPRINT_RAW`; null, with a warning, when it is all zero.
fn fingerprint(call: &Call<'_>) {
    // The arguments in order, as `zend_parse_parameters("r|l")` checks them.
    if call.arity(1, 2).is_err() {
        return;
    }
    let Ok(resource) = call.resource(1) else {
        return;
    };
    let Ok(flags) = call.long(2, 0) else {
        return;
    };
    let Some(session) = session(call, resource) else {
        return;
    };
    let hash: Option<&[u8]> = if flags & FINGERPRINT_SHA1 != 0 {
        hostkey_sha1(session, HASH_SHA1).map(<[u8; 20]>::as_slice)
    } else {
        hostkey_md5(session, HASH_MD5).map(<[u8; 16]>::as_slice)
    };
    let Some(hash) = hash else {
        warn("Unable to retrieve fingerprint from specified session");
        call.set_bool(false);
        return;
    };
    if hash.iter().all(|&byte| byte == 0) {
        warn("No fingerprint available using specified hash");
        call.set_null();
        return;
    }
    let value = if flags & FINGERPRINT_RAW != 0 {
        ZString::new(hash)
    } else {
        // `%02X` of each byte.
        let digit = |nibble: u8| {
            char::from_digit(nibble.into(), 16).map_or('0', |c| c.to_ascii_uppercase())
        };
        let hex: String = hash
            .iter()
            .flat_map(|&byte| [digit(byte >> 4), digit(byte & 0x0f)])
            .collect();
        ZString::new(hex.as_bytes())
    };
    call.set_owned(Owned::string(value));
}
