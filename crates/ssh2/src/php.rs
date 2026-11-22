//! The bundled ext/ssh2's PHP functions (`thirdparty/php/ssh2/ssh2.cc`) as
//! they move to Rust: so far `ssh2_connect`, `ssh2_methods_negotiated`,
//! `ssh2_fingerprint`, the `ssh2_auth_*` functions,
//! `ssh2_forward_listen`/`ssh2_forward_accept`, the SFTP functions that
//! change a path, `ssh2_sftp_readlink`/`ssh2_sftp_realpath`, `ssh2_sftp`,
//! `ssh2_sftp_chmod`, `ssh2_sftp_stat`/`ssh2_sftp_lstat`, and
//! `ssh2_send_eof`/`ssh2_shell_resize` on a channel stream, `ssh2_tunnel`,
//! `ssh2_shell` and `ssh2_exec`.
//! The resources stay ext/ssh2's; their sessions, listeners and SFTP sessions
//! are read through `swr_ssh2_session`, `swr_ssh2_listener` and `swr_ssh2_sftp`.
use crate::{Ops, Session, Sftp, Wait};
use std::ffi::{CStr, CString, c_int, c_long, c_uint, c_ulong, c_void};
use std::ptr::NonNull;
use swoole_rs_corebase::sys::Text;
use swoole_rs_corebase::{
    CCast,
    lent::{Count, In, Out, lend_in, lend_out},
};
use swoole_rs_ssh2_sys::{KbdintResponse, LIBSSH2_CHANNEL};
use swoole_rs_zend::{
    Call, E_ERROR, E_NOTICE, E_WARNING, Owned, OwnedArray, ReturnSlot, Value, ZStr, ZString,
    bailout, report,
};

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
    /// libssh2's own agent declarations, over the typed [`Agent`] and the
    /// [`Identity`] handles it lists.
    safe fn libssh2_agent_init(session: Session) -> Option<Agent>;
    safe fn libssh2_agent_free(agent: Agent);
    safe fn libssh2_agent_connect(agent: Agent) -> c_int;
    safe fn libssh2_agent_list_identities(agent: Agent) -> c_int;
    safe fn libssh2_agent_get_identity(
        agent: Agent,
        store: &mut Option<Identity>,
        previous: Option<Identity>,
    ) -> c_int;
    safe fn libssh2_agent_userauth(agent: Agent, username: Text<'_>, identity: Identity) -> c_int;
    safe fn libssh2_agent_disconnect(agent: Agent) -> c_int;
    /// The SFTP session an `SSH2 SFTP` resource holds (`zend_fetch_resource`),
    /// or null with Zend's `TypeError` thrown.
    safe fn swr_ssh2_sftp(resource: &Value) -> Sftp;
    /// The session of the `SSH2 SFTP` resource [`swr_ssh2_sftp`] accepted.
    safe fn swr_ssh2_sftp_session(resource: &Value) -> Session;
    /// libssh2's own SFTP declarations, each path its bytes and their length.
    safe fn libssh2_sftp_rename_ex(
        sftp: Sftp,
        source: Text<'_>,
        source_len: c_uint,
        destination: Text<'_>,
        destination_len: c_uint,
        flags: c_long,
    ) -> c_int;
    safe fn libssh2_sftp_unlink_ex(sftp: Sftp, path: Text<'_>, path_len: c_uint) -> c_int;
    safe fn libssh2_sftp_mkdir_ex(
        sftp: Sftp,
        path: Text<'_>,
        path_len: c_uint,
        mode: c_long,
    ) -> c_int;
    safe fn libssh2_sftp_rmdir_ex(sftp: Sftp, path: Text<'_>, path_len: c_uint) -> c_int;
    /// `libssh2_sftp_symlink_ex` making a link: both paths are read.
    #[link_name = "libssh2_sftp_symlink_ex"]
    safe fn sftp_symlink(
        sftp: Sftp,
        path: Text<'_>,
        path_len: c_uint,
        target: Text<'_>,
        target_len: c_uint,
        link_type: c_int,
    ) -> c_int;
    /// libssh2's own declaration: the session's SFTP subsystem, or null.
    safe fn libssh2_sftp_init(session: Session) -> Option<Started>;
    /// `ssh2_sftp`'s `SSH2 SFTP` resource over `sftp`, which holds a
    /// reference to the session resource, put in `out`.
    safe fn swr_ssh2_sftp_open(
        session_resource: &Value,
        session: Session,
        sftp: Started,
        out: ReturnSlot<'_>,
    );
    /// `libssh2_sftp_stat_ex` of `stat_type` (`LIBSSH2_SFTP_STAT` or
    /// `_LSTAT`), retried through the hook, each attribute stored in its out
    /// parameter; 0, or libssh2's error with none stored.
    safe fn swr_ssh2_sftp_stat(
        sftp: Sftp,
        session: Session,
        path: Text<'_>,
        path_len: c_uint,
        stat_type: c_int,
        flags: &mut c_ulong,
        size: &mut u64,
        uid: &mut c_ulong,
        gid: &mut c_ulong,
        permissions: &mut c_ulong,
        atime: &mut c_ulong,
        mtime: &mut c_ulong,
    ) -> c_int;
    /// `libssh2_sftp_stat_ex` setting only the permissions, retried through
    /// the hook: 0, or libssh2's error.
    safe fn swr_ssh2_sftp_chmod(
        sftp: Sftp,
        session: Session,
        path: Text<'_>,
        path_len: c_uint,
        permissions: c_ulong,
    ) -> c_int;
    /// The channel an `SSH2 Channel` stream holds: null, with Zend's
    /// `TypeError` for a resource that isn't a stream or C's warning for a
    /// stream of another kind, or, when `abstract_warns`, C's warning for a
    /// channel stream with no channel data.
    safe fn swr_ssh2_stream_channel(stream: &Value, abstract_warns: bool) -> Option<StreamChannel>;
    /// The session of the channel stream [`swr_ssh2_stream_channel`]
    /// accepted (`ssh2_get_session`): null, with Zend's `TypeError`, once the
    /// session resource has been closed.
    safe fn swr_ssh2_stream_session(stream: &Value) -> Session;
    /// libssh2's own declaration, as `crate`'s alias declares it: a channel to
    /// `host`:`port` through the server, or null.
    safe fn libssh2_channel_direct_tcpip_ex(
        session: Session,
        host: Text<'_>,
        port: c_int,
        shost: Text<'_>,
        sport: c_int,
    ) -> *mut LIBSSH2_CHANNEL;
    /// The channel stream over `channel`, which holds a reference to the
    /// session resource, put in `out`; false when no stream can be allocated.
    safe fn swr_ssh2_session_stream(session_resource: &Value, channel: Opened, out: ReturnSlot<'_>);
    /// libssh2's own declarations for a shell channel: an environment
    /// variable, the pty, the shell itself, and freeing a channel no stream
    /// was made over.
    safe fn libssh2_channel_setenv_ex<'name, 'value>(
        channel: Opened,
        name: In<'name, u8>,
        name_len: Count<'name, c_uint>,
        value: In<'value, u8>,
        value_len: Count<'value, c_uint>,
    ) -> c_int;
    safe fn libssh2_channel_request_pty_ex(
        channel: Opened,
        term: Text<'_>,
        term_len: c_uint,
        modes: Option<&'static Nothing>,
        modes_len: c_uint,
        width: c_int,
        height: c_int,
        width_px: c_int,
        height_px: c_int,
    ) -> c_int;
    safe fn libssh2_channel_process_startup(
        channel: Opened,
        request: Text<'_>,
        request_len: c_uint,
        message: Option<Text<'_>>,
        message_len: c_uint,
    ) -> c_int;
    safe fn libssh2_channel_free(channel: Opened) -> c_int;
    /// libssh2's own channel declarations.
    safe fn libssh2_channel_send_eof(channel: StreamChannel) -> c_int;
    safe fn libssh2_channel_request_pty_size_ex(
        channel: StreamChannel,
        width: c_int,
        height: c_int,
        width_px: c_int,
        height_px: c_int,
    ) -> c_int;
    /// Whether the session of the `SSH2 SFTP` resource [`swr_ssh2_sftp`]
    /// accepted has been closed (`ssh2_disconnect`).
    safe fn swr_ssh2_sftp_session_closed(resource: &Value) -> bool;
    /// `libssh2_sftp_symlink_ex` reading a link or resolving a path: the path
    /// is read and the result written to `target`; its length, or a negative
    /// error.
    #[expect(
        clashing_extern_declarations,
        reason = "libssh2_sftp_symlink_ex's target is read or written by link type: each declaration types its use"
    )]
    #[link_name = "libssh2_sftp_symlink_ex"]
    safe fn sftp_resolve<'a>(
        sftp: Sftp,
        path: Text<'_>,
        path_len: c_uint,
        target: Out<'a, u8>,
        target_len: Count<'a, c_uint>,
        link_type: c_int,
    ) -> c_int;
    /// `ssh2.cc`'s session over a socket connected to `host` and `port`, its
    /// allocator Zend's, its banner swoole's and its socket non-blocking; null,
    /// with C's warning, when the socket can't connect or libssh2 can't
    /// initialise the session.
    safe fn swr_ssh2_session_open(host: Text<'_>, port: c_int) -> Session;
    /// `ssh2_connect`'s `$callbacks`: each of `ignore`, `debug`, `macerror` and
    /// `disconnect` set on the session, with C's warning for one that isn't
    /// callable.
    safe fn swr_ssh2_session_callbacks(session: Session, callbacks: &Value);
    /// The socket descriptor the session's handshake runs on.
    safe fn swr_ssh2_session_fd(session: Session) -> c_int;
    /// A session [`swr_ssh2_session_open`] made, its socket and its data freed
    /// when its handshake fails.
    safe fn swr_ssh2_session_drop(session: Session);
    /// The `SSH2 Session` resource over `session`, put in `out`.
    safe fn swr_ssh2_session_resource(session: Session, out: ReturnSlot<'_>);
    /// libssh2's own declarations: a method preference, and the handshake.
    safe fn libssh2_session_method_pref(session: Session, kind: c_int, prefs: Text<'_>) -> c_int;
    safe fn libssh2_session_handshake(session: Session, socket: c_int) -> c_int;
    /// libssh2's own declarations for remote port forwarding: the listener, or
    /// null, and a connection it accepted, or null.
    safe fn libssh2_channel_forward_listen_ex(
        session: Session,
        host: Option<Text<'_>>,
        port: c_int,
        bound_port: Option<&'static Nothing>,
        queue_maxsize: c_int,
    ) -> Option<Listener>;
    safe fn libssh2_channel_forward_accept(listener: Listener) -> Option<Opened>;
    /// `ssh2_forward_listen`'s `SSH2 Listener` resource over `listener`, which
    /// holds a reference to the session resource, put in `out`.
    safe fn swr_ssh2_listener_open(
        session_resource: &Value,
        session: Session,
        listener: Listener,
        out: ReturnSlot<'_>,
    );
    /// The listener an `SSH2 Listener` resource holds (`zend_fetch_resource`),
    /// or null with Zend's `TypeError` thrown.
    safe fn swr_ssh2_listener(resource: &Value) -> Option<Listener>;
    /// The session of the `SSH2 Listener` resource [`swr_ssh2_listener`]
    /// accepted.
    safe fn swr_ssh2_listener_session(resource: &Value) -> Session;
    /// The channel stream over `channel`, which holds a reference to the
    /// listener's session resource, put in `out`; false, with C's warning and
    /// the channel freed, when no stream can be allocated.
    safe fn swr_ssh2_accepted_stream(listener: &Value, channel: Opened, out: ReturnSlot<'_>);
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

/// A session's ssh-agent handle (`LIBSSH2_AGENT *`), live until freed.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct Agent(NonNull<c_void>);

/// One of the identities an agent lists (`struct libssh2_agent_publickey *`),
/// which the agent keeps until it is freed: a handle, never dereferenced here.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct Identity(NonNull<c_void>);

/// A remote port's listener (`LIBSSH2_LISTENER *`), live until its resource
/// is destroyed.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct Listener(NonNull<c_void>);

/// A channel libssh2 just opened (`LIBSSH2_CHANNEL *`): a connection a
/// listener accepted, a tunnel or a shell, which the stream made over it owns.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub(crate) struct Opened(NonNull<c_void>);

/// An SFTP subsystem `libssh2_sftp_init` started (`LIBSSH2_SFTP *`), which the
/// resource made over it owns.
#[repr(transparent)]
struct Started(NonNull<c_void>);

/// The channel (`LIBSSH2_CHANNEL *`) an `SSH2 Channel` stream holds, live
/// while the stream is.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct StreamChannel(NonNull<c_void>);

/// What no value has: the password-change callback `ssh2_auth_password` never
/// passes, the bound port `ssh2_forward_listen` never asks for, and the pty
/// modes a shell or command never sends.
enum Nothing {}

swoole_rs_zend::handlers! {
    zif_ssh2_connect => connect;
    zif_ssh2_sftp => sftp_init;
    zif_ssh2_send_eof => send_eof;
    zif_ssh2_tunnel => tunnel;
    zif_ssh2_shell => shell;
    zif_ssh2_exec => exec;
    zif_ssh2_shell_resize => shell_resize;
    zif_ssh2_sftp_rename => sftp_rename;
    zif_ssh2_sftp_chmod => sftp_chmod;
    zif_ssh2_sftp_stat => sftp_stat;
    zif_ssh2_sftp_lstat => sftp_lstat;
    zif_ssh2_sftp_unlink => sftp_unlink;
    zif_ssh2_sftp_mkdir => sftp_mkdir;
    zif_ssh2_sftp_rmdir => sftp_rmdir;
    zif_ssh2_sftp_symlink => sftp_symlink_fn;
    zif_ssh2_sftp_readlink => sftp_readlink;
    zif_ssh2_sftp_realpath => sftp_realpath;
    zif_ssh2_methods_negotiated => methods_negotiated;
    zif_ssh2_fingerprint => fingerprint;
    zif_ssh2_auth_none => auth_none;
    zif_ssh2_auth_password => auth_password;
    zif_ssh2_auth_pubkey_file => auth_pubkey_file;
    zif_ssh2_auth_pubkey => auth_pubkey;
    zif_ssh2_auth_hostbased_file => auth_hostbased_file;
    zif_ssh2_auth_agent => auth_agent;
    zif_ssh2_forward_listen => forward_listen;
    zif_ssh2_forward_accept => forward_accept;
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

/// `ssh2_auth_agent(resource $session, string $username): bool`: each identity
/// the ssh-agent at `SSH_AUTH_SOCK` holds, tried in turn, unless the server
/// doesn't offer "publickey". Every failure warns with C's message.
fn auth_agent(call: &Call<'_>) {
    if call.arity(2, 2).is_err() {
        return;
    }
    let Ok(resource) = call.resource(1) else {
        return;
    };
    let Ok(Some(username)) = call.zstr(2) else {
        return;
    };
    let Some(session) = unauthenticated(call, resource) else {
        return;
    };
    let (name, name_len) = text(username);
    let methods = hooked(session, || libssh2_userauth_list(session, name, name_len));
    let refuse = |message: &str| {
        warn(message);
        call.set_bool(false);
    };
    if methods.is_some_and(|methods| {
        !methods
            .get()
            .to_bytes()
            .windows(9)
            .any(|window| window == b"publickey")
    }) {
        refuse("\"publickey\" authentication is not supported");
        return;
    }
    let Some(agent) = libssh2_agent_init(session) else {
        refuse("Failure initializing ssh-agent support");
        return;
    };
    if hooked(session, || libssh2_agent_connect(agent)) != 0 {
        refuse("Failure connecting to ssh-agent");
        libssh2_agent_free(agent);
        return;
    }
    let close = || {
        // Disconnecting is best effort, as in C, before the agent is freed.
        let _status = hooked(session, || libssh2_agent_disconnect(agent));
        libssh2_agent_free(agent);
    };
    if hooked(session, || libssh2_agent_list_identities(agent)) != 0 {
        refuse("Failure requesting identities to ssh-agent");
        close();
        return;
    }
    let mut previous = None;
    loop {
        let mut identity = None;
        let status = hooked(session, || {
            libssh2_agent_get_identity(agent, &mut identity, previous)
        });
        let current = match (status, identity) {
            (1, _) => {
                refuse("Couldn't continue authentication");
                close();
                return;
            }
            (0, Some(current)) => current,
            _ => {
                refuse("Failure obtaining identity from ssh-agent support");
                close();
                return;
            }
        };
        if hooked(session, || libssh2_agent_userauth(agent, name, current)) == 0 {
            close();
            call.set_bool(true);
            return;
        }
        previous = Some(current);
    }
}

/// `PHP_SSH2_LISTEN_MAX_QUEUED`: `ssh2_forward_listen`'s default queue.
const LISTEN_MAX_QUEUED: i64 = 128;

/// `ssh2_forward_listen(resource $session, int $port, string $host = null,
/// int $max_connections = 128): resource|false`: the remote server binds
/// `$port` (on `$host`, else every address) and forwards what connects to it.
fn forward_listen(call: &Call<'_>) {
    if call.arity(2, 4).is_err() {
        return;
    }
    let Ok(resource) = call.resource(1) else {
        return;
    };
    let Ok(port) = call.long(2, 0) else {
        return;
    };
    let Ok(host) = call.zstr(3) else {
        return;
    };
    let Ok(queue) = call.long(4, LISTEN_MAX_QUEUED) else {
        return;
    };
    let Some(session) = session(call, resource) else {
        return;
    };
    if !authenticated(session) {
        warn("Connection not authenticated");
        call.set_bool(false);
        return;
    }
    let host = host.map(|host| text(host).0);
    // C passed the longs to libssh2's ints as they were.
    let (port, queue) = (port.c_cast(), queue.c_cast());
    let Some(listener) = hooked(session, || {
        libssh2_channel_forward_listen_ex(session, host, port, None, queue)
    }) else {
        warn("Failure listening on remote port");
        call.set_bool(false);
        return;
    };
    swr_ssh2_listener_open(resource, session, listener, call.return_slot());
}

/// `ssh2_forward_accept(resource $listener): resource|false`: the next
/// connection to the listener's port, as a stream; false when none comes.
fn forward_accept(call: &Call<'_>) {
    if call.arity(1, 1).is_err() {
        return;
    }
    let Ok(resource) = call.resource(1) else {
        return;
    };
    let Some(listener) = swr_ssh2_listener(resource) else {
        call.set_bool(false);
        return;
    };
    let session = swr_ssh2_listener_session(resource);
    let Some(channel) = hooked(session, || libssh2_channel_forward_accept(listener)) else {
        call.set_bool(false);
        return;
    };
    swr_ssh2_accepted_stream(resource, channel, call.return_slot());
}

/// `PHP_SSH2_DEFAULT_PORT`.
const DEFAULT_PORT: i64 = 22;

/// `ssh2_connect(string $host, int $port = 22, ?array $methods = null,
/// ?array $callbacks = null): resource|false`: a session with the server,
/// its method preferences and callbacks set before the handshake.
fn connect(call: &Call<'_>) {
    if call.arity(1, 4).is_err() {
        return;
    }
    let Ok(Some(host)) = call.zstr(1) else {
        return;
    };
    let Ok(port) = call.long(2, DEFAULT_PORT) else {
        return;
    };
    let nullable_array = |n| match call.value(n) {
        None => Ok(None),
        Some(_) => call.array_or_null(n),
    };
    let Ok(methods) = nullable_array(3) else {
        return;
    };
    let Ok(callbacks) = nullable_array(4) else {
        return;
    };
    let (host, _) = text(host);
    let session = session_connect(
        host,
        port.c_cast(),
        methods.as_deref(),
        callbacks.as_deref(),
    );
    if session.0.is_null() {
        warn_bytes(&[b"Unable to connect to ", host.get().to_bytes()]);
        call.set_bool(false);
        return;
    }
    swr_ssh2_session_resource(session, call.return_slot());
}

swoole_rs_zend::exports! {
/// A session with the server at `host` and `port`, its method preferences
/// (`methods`) and callbacks (`callbacks`) set before the handshake, for
/// `ssh2_connect` and the `ssh2.*://` wrappers; null, with C's warning, when
/// the connection, the session or its handshake fails.
#[unsafe(no_mangle)]
pub extern "C" fn php_ssh2_session_connect(
    host: Text<'_>,
    port: c_int,
    methods: Option<&Value>,
    callbacks: Option<&Value>,
) -> Session {
    session_connect(host, port, methods, callbacks)
}
}

/// [`php_ssh2_session_connect`].
fn session_connect(
    host: Text<'_>,
    port: c_int,
    methods: Option<&Value>,
    callbacks: Option<&Value>,
) -> Session {
    let session = swr_ssh2_session_open(host, port);
    if session.0.is_null() {
        return session;
    }
    if let Some(methods) = methods.and_then(Value::array) {
        prefer_methods(session, methods);
    }
    if let Some(callbacks) = callbacks {
        swr_ssh2_session_callbacks(session, callbacks);
    }
    let socket = swr_ssh2_session_fd(session);
    if hooked(session, || libssh2_session_handshake(session, socket)) != 0 {
        let (mut message, mut length) = (None, 0);
        let code = libssh2_session_last_error(session, &mut message, &mut length, 0);
        warn_bytes(&[
            format!("Error starting up SSH connection({code}): ").as_bytes(),
            message.map_or(&[][..], |text| text.get().to_bytes()),
        ]);
        swr_ssh2_session_drop(session);
        return Session(std::ptr::null_mut());
    }
    session
}

/// `ssh2_connect`'s `$methods`: each preference given set on the session, with
/// C's warning for one that isn't a string or that libssh2 refuses.
fn prefer_methods(session: Session, methods: &swoole_rs_zend::Array) {
    let prefer = |table: &swoole_rs_zend::Array, key: &[u8], kind, what: &str| {
        let Some(value) = table.find(key) else {
            return;
        };
        let refused = value
            .str()
            .is_none_or(|prefs| libssh2_session_method_pref(session, kind, text(prefs).0) != 0);
        if refused {
            warn(&format!("Failed overriding {what} method"));
        }
    };
    prefer(methods, b"kex", METHOD_KEX, "KEX");
    prefer(methods, b"hostkey", METHOD_HOSTKEY, "HOSTKEY");
    for (key, direction, [crypt, mac, comp, lang]) in [
        (
            &b"client_to_server"[..],
            "client to server",
            [
                METHOD_CRYPT_CS,
                METHOD_MAC_CS,
                METHOD_COMP_CS,
                METHOD_LANG_CS,
            ],
        ),
        (
            b"server_to_client",
            "server to client",
            [
                METHOD_CRYPT_SC,
                METHOD_MAC_SC,
                METHOD_COMP_SC,
                METHOD_LANG_SC,
            ],
        ),
    ] {
        let Some(container) = methods.find(key).and_then(Value::array) else {
            continue;
        };
        prefer(container, b"crypt", crypt, &format!("{direction} CRYPT"));
        prefer(container, b"mac", mac, &format!("{direction} MAC"));
        prefer(container, b"comp", comp, &format!("{direction} COMP"));
        prefer(container, b"lang", lang, &format!("{direction} LANG"));
    }
}

/// `LIBSSH2_SFTP_RENAME_OVERWRITE | LIBSSH2_SFTP_RENAME_ATOMIC |
/// LIBSSH2_SFTP_RENAME_NATIVE`, every rename's flags.
const RENAME_FLAGS: c_long = 0x1 | 0x2 | 0x4;
/// `LIBSSH2_SFTP_SYMLINK`.
const SFTP_SYMLINK: c_int = 0;

/// The SFTP session `resource` holds and its session, or false returned (with
/// Zend's error raised) when it isn't an `SSH2 SFTP` resource.
fn sftp(call: &Call<'_>, resource: &Value) -> Option<(Sftp, Session)> {
    let sftp = swr_ssh2_sftp(resource);
    if sftp.0.is_null() {
        call.set_bool(false);
        return None;
    }
    Some((sftp, swr_ssh2_sftp_session(resource)))
}

/// An SFTP function's resource, of `min` to `max` arguments.
fn sftp_resource<'c>(call: &'c Call<'_>, min: u32, max: u32) -> Option<&'c Value> {
    call.arity(min, max).ok()?;
    call.resource(1).ok()
}

/// Path argument `n` of an SFTP function, or None with Zend's error raised.
fn path<'c>(call: &'c Call<'_>, n: u32) -> Option<&'c ZStr> {
    call.zstr(n).ok().flatten()
}

/// `ssh2_sftp_rename(resource $sftp, string $from, string $to): bool`,
/// overwriting `$to`.
fn sftp_rename(call: &Call<'_>) {
    let Some(resource) = sftp_resource(call, 3, 3) else {
        return;
    };
    let Some(from) = path(call, 2) else {
        return;
    };
    let Some(to) = path(call, 3) else {
        return;
    };
    let Some((sftp, session)) = sftp(call, resource) else {
        return;
    };
    let ((from, from_len), (to, to_len)) = (text(from), text(to));
    let status = hooked(session, || {
        libssh2_sftp_rename_ex(sftp, from, from_len, to, to_len, RENAME_FLAGS)
    });
    call.set_bool(status == 0);
}

/// `ssh2_sftp_unlink(resource $sftp, string $filename): bool`.
fn sftp_unlink(call: &Call<'_>) {
    let Some(resource) = sftp_resource(call, 2, 2) else {
        return;
    };
    let Some(path) = path(call, 2) else {
        return;
    };
    let Some((sftp, session)) = sftp(call, resource) else {
        return;
    };
    let (path, path_len) = text(path);
    call.set_bool(hooked(session, || libssh2_sftp_unlink_ex(sftp, path, path_len)) == 0);
}

/// `ssh2_sftp_mkdir(resource $sftp, string $dirname, int $mode = 0777,
/// bool $recursive = false): bool`. Recursively, each parent up to a `/` is
/// made first, its failure ignored; the result is the last directory's.
fn sftp_mkdir(call: &Call<'_>) {
    let Some(resource) = sftp_resource(call, 2, 4) else {
        return;
    };
    let Some(path) = path(call, 2) else {
        return;
    };
    let Ok(mode) = call.long(3, 0o777) else {
        return;
    };
    let Ok(recursive) = call.bool(4, false) else {
        return;
    };
    let Some((sftp, session)) = sftp(call, resource) else {
        return;
    };
    let (dir, dir_len) = text(path);
    let mkdir = |len| hooked(session, || libssh2_sftp_mkdir_ex(sftp, dir, len, mode));
    if recursive {
        // C's strchr from the second byte, to the path's first NUL; a slash
        // that ends the path is the last directory itself.
        let slashes = dir.get().to_bytes().iter().enumerate().skip(1);
        for (slash, _) in slashes.filter(|&(_, &byte)| byte == b'/') {
            if slash + 1 == path.bytes().len() {
                break;
            }
            // Only the last directory's result is returned.
            let _status = mkdir(slash.c_cast());
        }
    }
    call.set_bool(mkdir(dir_len) == 0);
}

/// `ssh2_sftp_rmdir(resource $sftp, string $dirname): bool`.
fn sftp_rmdir(call: &Call<'_>) {
    let Some(resource) = sftp_resource(call, 2, 2) else {
        return;
    };
    let Some(path) = path(call, 2) else {
        return;
    };
    let Some((sftp, session)) = sftp(call, resource) else {
        return;
    };
    let (path, path_len) = text(path);
    call.set_bool(hooked(session, || libssh2_sftp_rmdir_ex(sftp, path, path_len)) == 0);
}

/// `ssh2_sftp_symlink(resource $sftp, string $target, string $link): bool`.
fn sftp_symlink_fn(call: &Call<'_>) {
    let Some(resource) = sftp_resource(call, 3, 3) else {
        return;
    };
    let Some(target) = path(call, 2) else {
        return;
    };
    let Some(link) = path(call, 3) else {
        return;
    };
    let Some((sftp, session)) = sftp(call, resource) else {
        return;
    };
    let ((target, target_len), (link, link_len)) = (text(target), text(link));
    let status = hooked(session, || {
        sftp_symlink(sftp, target, target_len, link, link_len, SFTP_SYMLINK)
    });
    call.set_bool(status == 0);
}

/// `LIBSSH2_SFTP_READLINK` and `LIBSSH2_SFTP_REALPATH`.
const SFTP_READLINK: c_int = 1;
const SFTP_REALPATH: c_int = 2;
/// The target buffer C read into, `char targ[8192]`.
const TARGET_MAX: usize = 8192;

/// `$path` resolved by `link_type` into C's 8192 bytes: the target, or C's
/// warning (`what` and the path up to its first NUL) and false.
fn resolve(
    call: &Call<'_>,
    sftp: Sftp,
    session: Session,
    path: &ZStr,
    link_type: c_int,
    what: &[u8],
) {
    let (path, path_len) = text(path);
    let mut target = [0u8; TARGET_MAX];
    let length = hooked(session, || {
        lend_out(&mut target, |out, count| {
            sftp_resolve(sftp, path, path_len, out, count, link_type)
        })
    });
    let Some(target) = usize::try_from(length)
        .ok()
        .and_then(|length| target.get(..length))
    else {
        warn_bytes(&[what, b" '", path.get().to_bytes(), b"'"]);
        call.set_bool(false);
        return;
    };
    call.set_owned(Owned::string(ZString::new(target)));
}

/// `ssh2_sftp_readlink(resource $sftp, string $link): string|false`.
fn sftp_readlink(call: &Call<'_>) {
    let Some(resource) = sftp_resource(call, 2, 2) else {
        return;
    };
    let Some(link) = path(call, 2) else {
        return;
    };
    let Some((sftp, session)) = sftp(call, resource) else {
        return;
    };
    resolve(
        call,
        sftp,
        session,
        link,
        SFTP_READLINK,
        b"Unable to read link",
    );
}

/// `ssh2_sftp_realpath(resource $sftp, string $filename): string|false`:
/// false, without a warning, once the session has been closed.
fn sftp_realpath(call: &Call<'_>) {
    let Some(resource) = sftp_resource(call, 2, 2) else {
        return;
    };
    let Some(filename) = path(call, 2) else {
        return;
    };
    let Some((sftp, session)) = sftp(call, resource) else {
        return;
    };
    if swr_ssh2_sftp_session_closed(resource) {
        call.set_bool(false);
        return;
    }
    resolve(
        call,
        sftp,
        session,
        filename,
        SFTP_REALPATH,
        b"Unable to resolve realpath for",
    );
}

/// `ssh2_sftp(resource $session): resource|false`: the session's SFTP
/// subsystem, or C's warning with the session's last error and false.
fn sftp_init(call: &Call<'_>) {
    if call.arity(1, 1).is_err() {
        return;
    }
    let Ok(resource) = call.resource(1) else {
        return;
    };
    let Some(session) = session(call, resource) else {
        return;
    };
    let Some(sftp) = hooked(session, || libssh2_sftp_init(session)) else {
        warn_bytes(&[b"Unable to startup SFTP subsystem: ", last_error(session)]);
        call.set_bool(false);
        return;
    };
    swr_ssh2_sftp_open(resource, session, sftp, call.return_slot());
}

/// `ssh2_sftp_chmod(resource $sftp, string $filename, int $mode): bool`:
/// false for an empty filename, before the resource is read.
fn sftp_chmod(call: &Call<'_>) {
    let Some(resource) = sftp_resource(call, 3, 3) else {
        return;
    };
    let Some(filename) = path(call, 2) else {
        return;
    };
    let Ok(mode) = call.long(3, 0) else {
        return;
    };
    if filename.bytes().is_empty() {
        call.set_bool(false);
        return;
    }
    let Some((sftp, session)) = sftp(call, resource) else {
        return;
    };
    let (path, path_len) = text(filename);
    // C stored the long in the attributes' unsigned long as it was.
    call.set_bool(swr_ssh2_sftp_chmod(sftp, session, path, path_len, mode.c_cast()) == 0);
}

/// `LIBSSH2_SFTP_STAT` and `LIBSSH2_SFTP_LSTAT`.
const SFTP_STAT: c_int = 0;
const SFTP_LSTAT: c_int = 1;
/// `LIBSSH2_SFTP_ATTR_*`: which attributes the server sent.
const ATTR_SIZE: c_ulong = 0x1;
const ATTR_UIDGID: c_ulong = 0x2;
const ATTR_PERMISSIONS: c_ulong = 0x4;
const ATTR_ACMODTIME: c_ulong = 0x8;

/// `ssh2_sftp_stat(resource $sftp, string $path): array|false`, following
/// links.
fn sftp_stat(call: &Call<'_>) {
    stat(call, SFTP_STAT);
}

/// `ssh2_sftp_lstat(resource $sftp, string $path): array|false`, of a link
/// itself.
fn sftp_lstat(call: &Call<'_>) {
    stat(call, SFTP_LSTAT);
}

/// The attributes the server sent for `$path`, each under its `stat()` index
/// and name in C's order, or C's warning and false.
fn stat(call: &Call<'_>, stat_type: c_int) {
    let Some(resource) = sftp_resource(call, 2, 2) else {
        return;
    };
    let Some(path) = path(call, 2) else {
        return;
    };
    let Some((sftp, session)) = sftp(call, resource) else {
        return;
    };
    let (path, path_len) = text(path);
    let (mut flags, mut size, mut uid, mut gid) = (0, 0, 0, 0);
    let (mut permissions, mut atime, mut mtime) = (0, 0, 0);
    let status = swr_ssh2_sftp_stat(
        sftp,
        session,
        path,
        path_len,
        stat_type,
        &mut flags,
        &mut size,
        &mut uid,
        &mut gid,
        &mut permissions,
        &mut atime,
        &mut mtime,
    );
    if status != 0 {
        warn("Failed to stat remote file");
        call.set_bool(false);
        return;
    }
    let mut stat = OwnedArray::default();
    // C's add_index_long and add_assoc_long took each as a zend_long.
    let mut add = |index, name: &[u8], value: i64| {
        stat.insert_index(index, Owned::long(value));
        stat.insert_long(name, value);
    };
    if flags & ATTR_SIZE != 0 {
        add(7, b"size", size.c_cast());
    }
    if flags & ATTR_UIDGID != 0 {
        add(4, b"uid", uid.c_cast());
        add(5, b"gid", gid.c_cast());
    }
    if flags & ATTR_PERMISSIONS != 0 {
        add(2, b"mode", permissions.c_cast());
    }
    if flags & ATTR_ACMODTIME != 0 {
        add(8, b"atime", atime.c_cast());
        add(9, b"mtime", mtime.c_cast());
    }
    call.set_owned(stat.into_owned());
}

/// `ssh2_send_eof(resource $channel): bool`: EOF sent on the stream's channel,
/// or C's warning with libssh2's return code and false.
fn send_eof(call: &Call<'_>) {
    if call.arity(1, 1).is_err() {
        return;
    }
    let Ok(stream) = call.resource(1) else {
        return;
    };
    let Some(channel) = swr_ssh2_stream_channel(stream, true) else {
        call.set_bool(false);
        return;
    };
    let session = swr_ssh2_stream_session(stream);
    let status = hooked(session, || libssh2_channel_send_eof(channel));
    if status < 0 {
        warn(&format!(
            "Couldn't send EOF to channel (Return code {status})"
        ));
        call.set_bool(false);
        return;
    }
    call.set_bool(true);
}

/// `ssh2_shell_resize(resource $stream, int $width, int $height,
/// int $width_height_type = 0, int $height_px = 0): bool`: the channel's
/// terminal resized; true whatever libssh2 answers, as in C.
fn shell_resize(call: &Call<'_>) {
    if call.arity(3, 5).is_err() {
        return;
    }
    let Ok(stream) = call.resource(1) else {
        return;
    };
    let size = |n, default| call.long(n, default).ok();
    let (Some(width), Some(height), Some(width_px), Some(height_px)) =
        (size(2, 0), size(3, 0), size(4, 0), size(5, 0))
    else {
        return;
    };
    let Some(channel) = swr_ssh2_stream_channel(stream, false) else {
        call.set_bool(false);
        return;
    };
    let session = swr_ssh2_stream_session(stream);
    // C passed the longs to libssh2's ints as they were.
    let (width, height) = (width.c_cast(), height.c_cast());
    let (width_px, height_px) = (width_px.c_cast(), height_px.c_cast());
    // C ignored the answer: the resize is a request the server may refuse.
    let _status = hooked(session, || {
        libssh2_channel_request_pty_size_ex(channel, width, height, width_px, height_px)
    });
    call.set_bool(true);
}

/// `ssh2_tunnel(resource $session, string $host, int $port): resource|false`:
/// a stream to `$host`:`$port`, opened from the server.
fn tunnel(call: &Call<'_>) {
    if call.arity(3, 3).is_err() {
        return;
    }
    let Ok(resource) = call.resource(1) else {
        return;
    };
    let Ok(Some(host)) = call.zstr(2) else {
        return;
    };
    let Ok(port) = call.long(3, 0) else {
        return;
    };
    let Some(session) = session(call, resource) else {
        return;
    };
    if !authenticated(session) {
        warn("Connection not authenticated");
        call.set_bool(false);
        return;
    }
    let (host, _) = text(host);
    // C passed the long to the int port as it was, and libssh2_channel_direct_tcpip's
    // source address.
    let port = port.c_cast();
    let source = c"127.0.0.1".into();
    let channel = hooked(session, || {
        libssh2_channel_direct_tcpip_ex(session, host, port, source, 22)
    });
    let Some(channel) = NonNull::new(channel.cast()).map(Opened) else {
        warn("Unable to request a channel from remote host");
        call.set_bool(false);
        return;
    };
    swr_ssh2_session_stream(resource, channel, call.return_slot());
}

/// `PHP_SSH2_DEFAULT_TERMINAL`, `_TERM_WIDTH`, `_TERM_HEIGHT` and
/// `PHP_SSH2_TERM_UNIT_CHARS`, `ssh2_shell`'s defaults.
const DEFAULT_TERMINAL: &CStr = c"vanilla";
const DEFAULT_TERM_WIDTH: i64 = 80;
const DEFAULT_TERM_HEIGHT: i64 = 25;
const TERM_UNIT_CHARS: i64 = 0;

/// `ssh2_shell(resource $session, string $term_type = "vanilla",
/// ?array $env = null, int $width = 80, int $height = 25,
/// int $width_height_type = SSH2_TERM_UNIT_CHARS): resource|false`: a shell
/// on the server, as a stream. Five arguments are C's fatal error.
fn shell(call: &Call<'_>) {
    if call.num_args() == 5 {
        bailout::defer(report(E_ERROR, c"width specified without height parameter"));
        call.set_bool(false);
        return;
    }
    if call.arity(1, 6).is_err() {
        return;
    }
    let Ok(resource) = call.resource(1) else {
        return;
    };
    let Ok(term) = call.zstr(2) else {
        return;
    };
    let environment = match call.value(3) {
        None => Ok(None),
        Some(_) => call.array_or_null(3),
    };
    let Ok(environment) = environment else {
        return;
    };
    let long = |n, default| call.long(n, default).ok();
    let (Some(width), Some(height), Some(unit)) = (
        long(4, DEFAULT_TERM_WIDTH),
        long(5, DEFAULT_TERM_HEIGHT),
        long(6, TERM_UNIT_CHARS),
    ) else {
        return;
    };
    let Some(session) = session(call, resource) else {
        return;
    };
    if !authenticated(session) {
        warn("Connection not authenticated");
        call.set_bool(false);
        return;
    }
    let (term, term_len) = term.map_or(
        (
            DEFAULT_TERMINAL.into(),
            DEFAULT_TERMINAL.count_bytes().c_cast(),
        ),
        text,
    );
    let pty = Pty {
        term,
        term_len,
        width,
        height,
        unit,
    };
    let Some(channel) = shell_channel(session, pty, environment.as_deref()) else {
        call.set_bool(false);
        return;
    };
    swr_ssh2_session_stream(resource, channel, call.return_slot());
}

swoole_rs_zend::exports! {
/// A session channel running a shell in a `term` pty of `width` by `height`
/// characters or pixels (`unit`), with `environment`'s string keys set first,
/// for `ssh2_shell` and the `ssh2.shell://` wrapper; null, with C's warning,
/// when the channel, its pty or its shell can't be had.
#[unsafe(no_mangle)]
pub extern "C" fn swr_ssh2_shell_channel(
    session: Session,
    term: Text<'_>,
    term_len: c_uint,
    environment: Option<&Value>,
    width: c_long,
    height: c_long,
    unit: c_long,
) -> Option<Opened> {
    let pty = Pty {
        term,
        term_len,
        width,
        height,
        unit,
    };
    shell_channel(session, pty, environment)
}
}

/// [`swr_ssh2_shell_channel`].
fn shell_channel(session: Session, pty: Pty<'_>, environment: Option<&Value>) -> Option<Opened> {
    let opened = hooked(session, || crate::open_session(session));
    let Some(channel) = NonNull::new(opened.cast()).map(Opened) else {
        warn("Unable to request a channel from remote host");
        return None;
    };
    if let Some(environment) = environment.and_then(Value::array) {
        set_environment(session, channel, environment, Values::Own);
    }
    if !request_pty(session, channel, pty) {
        return None;
    }
    let request = c"shell";
    let started = hooked(session, || {
        libssh2_channel_process_startup(
            channel,
            request.into(),
            request.count_bytes().c_cast(),
            None,
            0,
        )
    });
    if started != 0 {
        warn("Unable to request shell from remote host");
        // As above.
        let _status = libssh2_channel_free(channel);
        return None;
    }
    Some(channel)
}

/// A pty to request: its terminal type, as a C string and the length libssh2
/// is told, and its size in characters, or pixels for any other `unit`.
#[derive(Clone, Copy)]
struct Pty<'a> {
    term: Text<'a>,
    term_len: c_uint,
    width: i64,
    height: i64,
    unit: i64,
}

/// Which value each environment variable is set to.
#[derive(Clone, Copy)]
enum Values {
    /// Its own (`ssh2_shell`).
    Own,
    /// The array's element at its internal pointer, whatever the key, and
    /// none past the end (`ssh2_exec`: C read `zend_hash_get_current_data`
    /// while it walked the keys with a position of its own).
    Current,
}

/// Each string key of `environment` set on the channel to its value
/// (`values`) as a string, with C's warning when the server refuses one; an
/// integer key is skipped with C's notice.
fn set_environment(
    session: Session,
    channel: Opened,
    environment: &swoole_rs_zend::Array,
    values: Values,
) {
    for (name, own) in environment.entries() {
        let Some(name) = name else {
            bailout::defer(report(
                E_NOTICE,
                c"Skipping numeric index in environment array",
            ));
            continue;
        };
        let value = match values {
            Values::Own => Some(own),
            Values::Current => environment.current(),
        };
        let Some(value) = value.map(Value::string) else {
            continue;
        };
        let set = hooked(session, || {
            lend_in(name, |name, name_len| {
                lend_in(value.bytes(), |value, value_len| {
                    libssh2_channel_setenv_ex(channel, name, name_len, value, value_len)
                })
            })
        });
        if set != 0 {
            // C's %s of each, up to its first NUL.
            let until_nul = |bytes: &[u8]| {
                bytes
                    .split(|&byte| byte == 0)
                    .next()
                    .unwrap_or_default()
                    .to_vec()
            };
            warn_bytes(&[
                b"Failed setting ",
                &until_nul(name),
                b"=",
                &until_nul(value.bytes()),
                b" on remote end",
            ]);
        }
    }
}

/// A `term` pty of `width` by `height` characters, or pixels for any other
/// `unit`, requested on the channel; false, with C's warning and the channel
/// freed, when the server refuses it.
fn request_pty(session: Session, channel: Opened, pty: Pty<'_>) -> bool {
    let Pty {
        term,
        term_len,
        width,
        height,
        unit,
    } = pty;
    // C passed the longs to libssh2's ints as they were.
    let (columns, rows) = (width.c_cast(), height.c_cast());
    let (size, units) = if unit == TERM_UNIT_CHARS {
        ([columns, rows, 0, 0], "characters")
    } else {
        ([0, 0, columns, rows], "pixels")
    };
    let [width_chars, height_chars, width_px, height_px] = size;
    let refused = hooked(session, || {
        libssh2_channel_request_pty_ex(
            channel,
            term,
            term_len,
            None,
            0,
            width_chars,
            height_chars,
            width_px,
            height_px,
        )
    });
    if refused != 0 {
        warn_bytes(&[
            b"Failed allocating ",
            term.get().to_bytes(),
            format!(" pty at {width}x{height} {units}").as_bytes(),
        ]);
        // The channel failed; its status adds nothing to the warning.
        let _status = libssh2_channel_free(channel);
        return false;
    }
    true
}

/// `ssh2_exec(resource $session, string $command, ?string $pty = null,
/// ?array $env = null, int $width = 80, int $height = 25,
/// int $width_height_type = SSH2_TERM_UNIT_CHARS): resource|false`: the
/// command run on the server, as a stream. As in C, an array third argument
/// swaps `$pty` and `$env`.
fn exec(call: &Call<'_>) {
    if call.arity(2, 7).is_err() {
        return;
    }
    let Ok(resource) = call.resource(1) else {
        return;
    };
    let Ok(Some(command)) = call.zstr(2) else {
        return;
    };
    let long = |n, default| call.long(n, default).ok();
    let (Some(width), Some(height), Some(unit)) = (
        long(5, DEFAULT_TERM_WIDTH),
        long(6, DEFAULT_TERM_HEIGHT),
        long(7, TERM_UNIT_CHARS),
    ) else {
        return;
    };
    let given = |n| call.value(n).filter(|value| !value.is_null());
    let (mut pty, mut environment) = (given(3), given(4));
    if pty.is_some_and(|pty| pty.array().is_some()) {
        (pty, environment) = (environment, pty);
    }
    if environment.is_some_and(|environment| environment.array().is_none()) {
        warn("ssh2_exec() expects arg 4 to be of type array");
        call.set_bool(false);
        return;
    }
    let term = pty.map(Value::string);
    let Some(session) = session(call, resource) else {
        return;
    };
    if !authenticated(session) {
        warn("Connection not authenticated");
        call.set_bool(false);
        return;
    }
    let (command, _) = text(command);
    let pty = term.as_deref().map(|term| {
        let (term, term_len) = text(term);
        Pty {
            term,
            term_len,
            width,
            height,
            unit,
        }
    });
    let Some(channel) = exec_channel(session, command, pty, environment) else {
        call.set_bool(false);
        return;
    };
    swr_ssh2_session_stream(resource, channel, call.return_slot());
}

swoole_rs_zend::exports! {
/// A session channel running `command`, with `environment`'s string keys set
/// to the value at its internal pointer and, given a `term`, a pty requested
/// first, for `ssh2_exec` and the `ssh2.exec://` wrapper; null, with C's
/// warning, when the channel, its pty or the command can't be had.
#[unsafe(no_mangle)]
#[expect(
    clippy::too_many_arguments,
    reason = "php_ssh2_exec_command's arguments, as the ssh2.exec:// wrapper passes them"
)]
pub extern "C" fn swr_ssh2_exec_channel(
    session: Session,
    command: Text<'_>,
    term: Option<Text<'_>>,
    term_len: c_uint,
    environment: Option<&Value>,
    width: c_long,
    height: c_long,
    unit: c_long,
) -> Option<Opened> {
    let pty = term.map(|term| Pty {
        term,
        term_len,
        width,
        height,
        unit,
    });
    exec_channel(session, command, pty, environment)
}
}

/// [`swr_ssh2_exec_channel`].
fn exec_channel(
    session: Session,
    command: Text<'_>,
    pty: Option<Pty<'_>>,
    environment: Option<&Value>,
) -> Option<Opened> {
    let opened = hooked(session, || crate::open_session(session));
    let Some(channel) = NonNull::new(opened.cast()).map(Opened) else {
        warn("Unable to request a channel from remote host");
        return None;
    };
    if let Some(environment) = environment.and_then(Value::array) {
        set_environment(session, channel, environment, Values::Current);
    }
    if let Some(pty) = pty
        && !request_pty(session, channel, pty)
    {
        return None;
    }
    let request = c"exec";
    let started = hooked(session, || {
        libssh2_channel_process_startup(
            channel,
            request.into(),
            request.count_bytes().c_cast(),
            Some(command),
            command.get().count_bytes().c_cast(),
        )
    });
    if started != 0 {
        warn("Unable to request command execution on remote host");
        // The command failed; the channel's status adds nothing to the warning.
        let _status = libssh2_channel_free(channel);
        return None;
    }
    Some(channel)
}
