//! The bundled ext/ssh2's PHP functions (`thirdparty/php/ssh2/ssh2.cc`) as
//! they move to Rust: so far `ssh2_methods_negotiated`, `ssh2_fingerprint`
//! and the `ssh2_auth_*` functions but `ssh2_auth_agent`.
//! The resources stay ext/ssh2's; their sessions are read through
//! `swr_ssh2_session`.
use crate::{Ops, Session, Wait};
use std::ffi::{CStr, CString, c_int, c_uint};
use swoole_rs_corebase::sys::Text;
use swoole_rs_corebase::{
    CCast,
    lent::{Count, In, lend_in},
};
use swoole_rs_ssh2_sys::KbdintResponse;
use swoole_rs_zend::{Call, E_WARNING, Owned, OwnedArray, Value, ZStr, ZString, bailout, report};

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
    /// `ssh2_bound` (crates/hooks): the wait for the session's socket, once no
    /// other coroutine is bound to it.
    safe fn swr_v1_ssh2_bound(session: Session) -> &'static Ops;
    /// The password `ssh2.cc`'s keyboard-interactive callback answers with,
    /// which must stay live until the authentication returns.
    safe fn swr_ssh2_kbd_password(password: Text<'_>);
    /// libssh2's own declarations, with the strings typed as the C strings they
    /// are and their lengths beside them.
    safe fn libssh2_userauth_list<'a>(
        session: Session,
        username: Text<'_>,
        length: c_uint,
    ) -> Option<Text<'a>>;
    safe fn libssh2_userauth_authenticated(session: Session) -> c_int;
    safe fn libssh2_userauth_password_ex(
        session: Session,
        username: Text<'_>,
        username_len: c_uint,
        password: Text<'_>,
        password_len: c_uint,
        change: Option<&'static Nothing>,
    ) -> c_int;
    safe fn libssh2_userauth_keyboard_interactive_ex(
        session: Session,
        username: Text<'_>,
        username_len: c_uint,
        response: KbdintResponse,
    ) -> c_int;
    /// `ssh2.cc`'s keyboard-interactive callback, which answers the one prompt
    /// with the password [`swr_ssh2_kbd_password`] set.
    fn swr_ssh2_kbd_callback(
        name: *const std::ffi::c_char,
        name_len: c_int,
        instruction: *const std::ffi::c_char,
        instruction_len: c_int,
        num_prompts: c_int,
        prompts: *const std::ffi::c_void,
        responses: *mut std::ffi::c_void,
        abstract_: *mut *mut std::ffi::c_void,
    );
    /// `php_check_open_basedir_ex(path, 1)` inside `zend_try` (nonzero for a
    /// bailout): `refused` when `open_basedir` refuses the path, which warns.
    safe fn swr_zend_try_open_basedir(path: Text<'_>, warn: c_int, refused: &mut bool) -> c_int;
    /// `getpwuid(geteuid())->pw_dir`, or null: the home directory a `~/` key
    /// path is relative to, which libssh2 doesn't expand.
    safe fn swr_ssh2_home_dir() -> Option<Text<'static>>;
    /// libssh2's own declaration: the session's last error message.
    safe fn libssh2_session_last_error(
        session: Session,
        message: &mut Option<Text<'_>>,
        length: &mut c_int,
        want_buffer: c_int,
    ) -> c_int;
    safe fn libssh2_userauth_publickey_fromfile_ex(
        session: Session,
        username: Text<'_>,
        username_len: c_uint,
        publickey: Text<'_>,
        privatekey: Text<'_>,
        passphrase: Option<Text<'_>>,
    ) -> c_int;
    safe fn libssh2_userauth_publickey_frommemory<'public, 'private>(
        session: Session,
        username: Text<'_>,
        username_len: usize,
        publickey: In<'public, u8>,
        publickey_len: Count<'public>,
        privatekey: In<'private, u8>,
        privatekey_len: Count<'private>,
        passphrase: Option<Text<'_>>,
    ) -> c_int;
    safe fn libssh2_userauth_hostbased_fromfile_ex(
        session: Session,
        username: Text<'_>,
        username_len: c_uint,
        publickey: Text<'_>,
        privatekey: Text<'_>,
        passphrase: Option<Text<'_>>,
        hostname: Text<'_>,
        hostname_len: c_uint,
        local_username: Text<'_>,
        local_username_len: c_uint,
    ) -> c_int;
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

/// What no value has: the password-change callback `ssh2_auth_password` never passes.
enum Nothing {}

swoole_rs_zend::handlers! {
    zif_ssh2_methods_negotiated => methods_negotiated;
    zif_ssh2_fingerprint => fingerprint;
    zif_ssh2_auth_none => auth_none;
    zif_ssh2_auth_password => auth_password;
    zif_ssh2_auth_pubkey_file => auth_pubkey_file;
    zif_ssh2_auth_pubkey => auth_pubkey;
    zif_ssh2_auth_hostbased_file => auth_hostbased_file;
}

/// A PHP string as the C string libssh2 reads, up to its NUL, and its length
/// as the `unsigned int` libssh2 takes.
fn text(string: &ZStr) -> (Text<'_>, c_uint) {
    let text = CStr::from_bytes_until_nul(string.bytes_with_nul()).unwrap_or_default();
    (text.into(), string.bytes().len().c_cast())
}

/// The hooked call's retry: `call` while libssh2 says `EAGAIN`, parking the
/// coroutine on the session's socket, as the C call sites did through
/// `php_swoole_ssh2_hook.h`.
fn hooked<R: crate::Answer>(session: Session, call: impl FnMut() -> R) -> R::Result {
    Wait {
        ops: swr_v1_ssh2_bound(session),
        session,
    }
    .run(call)
}

/// `libssh2_userauth_authenticated`, hooked.
fn authenticated(session: Session) -> bool {
    hooked(session, || libssh2_userauth_authenticated(session)) != 0
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
    warn_bytes(&[message.as_bytes()]);
}

/// [`warn`] of `parts` joined, as raw bytes: C's `%s` passed its strings' bytes
/// through, whatever their encoding.
fn warn_bytes(parts: &[&[u8]]) {
    if let Ok(message) = CString::new(parts.concat()) {
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

/// `ssh2_auth_none(resource $session, string $username): array|bool`: true if
/// "none" authentication succeeded, else the methods the server allows (false
/// when it gave none and the session isn't authenticated).
fn auth_none(call: &Call<'_>) {
    if call.arity(2, 2).is_err() {
        return;
    }
    let Ok(resource) = call.resource(1) else {
        return;
    };
    let Ok(Some(username)) = call.zstr(2) else {
        return;
    };
    let Some(session) = session(call, resource) else {
        return;
    };
    let (name, length) = text(username);
    let Some(methods) = hooked(session, || libssh2_userauth_list(session, name, length)) else {
        // Either a failure, or an unexpected success.
        call.set_bool(authenticated(session));
        return;
    };
    let mut allowed = OwnedArray::default();
    for method in methods.get().to_bytes().split(|&byte| byte == b',') {
        if !method.is_empty() {
            allowed.push_bytes(method);
        }
    }
    call.set_owned(allowed.into_owned());
}

/// `ssh2_auth_password(resource $session, string $username, string $password): bool`:
/// keyboard-interactive authentication with the password when the server
/// offers it, else (or when it fails) password authentication.
fn auth_password(call: &Call<'_>) {
    if call.arity(3, 3).is_err() {
        return;
    }
    let Ok(resource) = call.resource(1) else {
        return;
    };
    let (Ok(Some(username)), Ok(Some(password))) = (call.zstr(2), call.zstr(3)) else {
        return;
    };
    let Some(session) = session(call, resource) else {
        return;
    };
    if authenticated(session) {
        warn("Connection already authenticated");
        call.set_bool(false);
        return;
    }
    let (name, name_len) = text(username);
    let Some(methods) = hooked(session, || libssh2_userauth_list(session, name, name_len)) else {
        call.set_bool(true);
        return;
    };
    let (secret, secret_len) = text(password);
    swr_ssh2_kbd_password(secret);
    // C's libssh2_userauth_keyboard_interactive measured the name with strlen,
    // unlike the other calls, which take the PHP string's length.
    let c_name_len: c_uint = name.get().count_bytes().c_cast();
    let offered = methods
        .get()
        .to_bytes()
        .windows(20)
        .any(|window| window == b"keyboard-interactive");
    if offered
        && hooked(session, || {
            libssh2_userauth_keyboard_interactive_ex(
                session,
                name,
                c_name_len,
                swr_ssh2_kbd_callback,
            )
        }) == 0
    {
        call.set_bool(true);
        return;
    }
    if hooked(session, || {
        libssh2_userauth_password_ex(session, name, name_len, secret, secret_len, None)
    }) != 0
    {
        warn_bytes(&[
            b"Authentication failed for ",
            name.get().to_bytes(),
            b" using password",
        ]);
        call.set_bool(false);
        return;
    }
    call.set_bool(true);
}

/// `php_check_open_basedir`: whether `open_basedir` refuses `path` (warning);
/// a bailout is deferred and counts as refused.
fn basedir_refuses(path: &ZStr) -> bool {
    let (path, _) = text(path);
    let mut refused = true;
    match bailout::trapped(swr_zend_try_open_basedir(path, 1, &mut refused)) {
        Ok(()) => refused,
        Err(bailed) => {
            bailed.defer();
            true
        }
    }
}

/// The session argument 1 holds, refused with C's warning when it is already
/// authenticated (`SSH2_FETCH_NONAUTHENTICATED_SESSION`); false returned then.
fn unauthenticated(call: &Call<'_>, resource: &Value) -> Option<Session> {
    let session = session(call, resource)?;
    if authenticated(session) {
        warn("Connection already authenticated");
        call.set_bool(false);
        return None;
    }
    Some(session)
}

/// A key path with a leading `~/` expanded to the user's home directory, as a
/// new string (libssh2 doesn't expand it); other paths as they are, up to
/// their NUL, as C passed them.
fn key_path(path: &ZStr) -> Option<CString> {
    let bytes = path.bytes();
    let (text, _) = text(path);
    if bytes.len() >= 2 && bytes.starts_with(b"~/") {
        let home = swr_ssh2_home_dir()?;
        let mut expanded = home.get().to_bytes().to_vec();
        expanded.extend_from_slice(text.get().to_bytes().get(1..).unwrap_or_default());
        return CString::new(expanded).ok();
    }
    Some(text.get().to_owned())
}

/// The session's last error message, for a failed authentication's warning.
fn last_error<'a>(session: Session) -> &'a [u8] {
    let (mut message, mut length) = (None, 0);
    // The message is all this reads; the code is the failure already reported.
    let _code = libssh2_session_last_error(session, &mut message, &mut length, 0);
    message.map_or(&[][..], |text: Text<'a>| text.get().to_bytes())
}

/// The `S`-typed arguments 2 to 4, then the optional passphrase (5), as
/// `zend_parse_parameters("rSSS|S")` reads them.
fn key_arguments<'a>(call: &'a Call<'_>) -> Option<(&'a Value, [&'a ZStr; 3], Option<&'a ZStr>)> {
    if call.arity(4, 5).is_err() {
        return None;
    }
    let resource = call.resource(1).ok()?;
    let username = call.zstr(2).ok()??;
    let public = call.zstr(3).ok()??;
    let private = call.zstr(4).ok()??;
    let passphrase = call.zstr(5).ok()?;
    Some((resource, [username, public, private], passphrase))
}

/// `ssh2_auth_pubkey_file(resource $session, string $username, string $pubkeyfile,
/// string $privkeyfile, string $passphrase = ?): bool`.
fn auth_pubkey_file(call: &Call<'_>) {
    let Some((resource, [username, public, private], passphrase)) = key_arguments(call) else {
        return;
    };
    if basedir_refuses(public) || basedir_refuses(private) {
        call.set_bool(false);
        return;
    }
    let Some(session) = unauthenticated(call, resource) else {
        return;
    };
    let (Some(public), Some(private)) = (key_path(public), key_path(private)) else {
        call.set_bool(false);
        return;
    };
    let (name, name_len) = text(username);
    let passphrase = passphrase.map(|passphrase| text(passphrase).0);
    let failed = hooked(session, || {
        libssh2_userauth_publickey_fromfile_ex(
            session,
            name,
            name_len,
            public.as_c_str().into(),
            private.as_c_str().into(),
            passphrase,
        )
    }) != 0;
    finish_key(call, session, name, failed);
}

/// `ssh2_auth_pubkey(resource $session, string $username, string $pubkey, string
/// $privkey, string $passphrase = ?): bool`, the keys as their contents.
fn auth_pubkey(call: &Call<'_>) {
    let Some((resource, [username, public, private], passphrase)) = key_arguments(call) else {
        return;
    };
    let Some(session) = unauthenticated(call, resource) else {
        return;
    };
    let (name, _) = text(username);
    let name_len = username.bytes().len();
    let passphrase = passphrase.map(|passphrase| text(passphrase).0);
    let failed = hooked(session, || {
        lend_in(public.bytes(), |public, public_len| {
            lend_in(private.bytes(), |private, private_len| {
                libssh2_userauth_publickey_frommemory(
                    session,
                    name,
                    name_len,
                    public,
                    public_len,
                    private,
                    private_len,
                    passphrase,
                )
            })
        })
    }) != 0;
    finish_key(call, session, name, failed);
}

/// The result of a key authentication: true, or C's warning with the session's
/// last error and false.
fn finish_key(call: &Call<'_>, session: Session, name: Text<'_>, failed: bool) {
    if failed {
        warn_bytes(&[
            b"Authentication failed for ",
            name.get().to_bytes(),
            b" using public key: ",
            last_error(session),
        ]);
    }
    call.set_bool(!failed);
}

/// `ssh2_auth_hostbased_file(resource $session, string $username, string $hostname,
/// string $pubkeyfile, string $privkeyfile, ?string $passphrase = null, ?string
/// $local_username = null): bool`, the local user defaulting to `$username`.
fn auth_hostbased_file(call: &Call<'_>) {
    if call.arity(5, 7).is_err() {
        return;
    }
    let Ok(resource) = call.resource(1) else {
        return;
    };
    let strings = [2, 3, 4, 5].map(|n| call.zstr(n));
    let [
        Ok(Some(username)),
        Ok(Some(hostname)),
        Ok(Some(public)),
        Ok(Some(private)),
    ] = strings
    else {
        return;
    };
    let (Ok(passphrase), Ok(local)) = (call.zstr_or_null(6), call.zstr_or_null(7)) else {
        return;
    };
    if basedir_refuses(public) || basedir_refuses(private) {
        call.set_bool(false);
        return;
    }
    let Some(session) = unauthenticated(call, resource) else {
        return;
    };
    let (name, name_len) = text(username);
    let (host, host_len) = text(hostname);
    let (local, local_len) = text(local.unwrap_or(username));
    let passphrase = passphrase.map(|passphrase| text(passphrase).0);
    let (public, private) = (text(public).0, text(private).0);
    let failed = hooked(session, || {
        libssh2_userauth_hostbased_fromfile_ex(
            session, name, name_len, public, private, passphrase, host, host_len, local, local_len,
        )
    }) != 0;
    if failed {
        warn_bytes(&[
            b"Authentication failed for ",
            name.get().to_bytes(),
            b" using hostbased public key",
        ]);
    }
    call.set_bool(!failed);
}
