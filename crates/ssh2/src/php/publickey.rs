//! ext/ssh2's publickey subsystem (once `thirdparty/php/ssh2/ssh2_publickey.cc`):
//! `ssh2_publickey_init`, `ssh2_publickey_add`, `ssh2_publickey_remove` and
//! `ssh2_publickey_list`, with C's behaviour. As in C, their libssh2 calls are
//! not retried through the coroutine hook: on a session the hook made
//! non-blocking, `ssh2_publickey_init` fails with libssh2's would-block error.
//! What stays C (`native/hooks/ssh2.cc`): the subsystem's resource, and the key
//! list and attribute layouts libssh2's header defines.
use super::{authenticated, libssh2_session_last_error, session, warn, warn_bytes};
use crate::Session;
use std::ffi::{c_int, c_ulong, c_void};
use std::ptr::NonNull;
use swoole_rs_corebase::lent::{Count, In, lend_in};
use swoole_rs_zend::{Array, Call, Owned, OwnedArray, ReturnSlot, Value, ZStr};

/// A publickey subsystem (`LIBSSH2_PUBLICKEY *`), live while its resource is.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct Publickey(NonNull<c_void>);

/// The keys a list fetched (`libssh2_publickey_list *`), live until freed.
#[repr(transparent)]
struct Listed(NonNull<c_void>);

unsafe extern "C" {
    /// libssh2's own declaration: the session's publickey subsystem, or null.
    safe fn libssh2_publickey_init(session: Session) -> Option<Publickey>;
    /// libssh2's own declaration: the entry for the `name` and `blob` key
    /// removed; 0, or a libssh2 error.
    safe fn libssh2_publickey_remove_ex<'n, 'b>(
        pkey: Publickey,
        name: In<'n, u8>,
        name_len: Count<'n, c_ulong>,
        blob: In<'b, u8>,
        blob_len: Count<'b, c_ulong>,
    ) -> c_int;
    /// libssh2's own declaration: the server's keys, and how many; 0, or a
    /// libssh2 error with neither written.
    safe fn libssh2_publickey_list_fetch(
        pkey: Publickey,
        num_keys: &mut c_ulong,
        keys: &mut Option<Listed>,
    ) -> c_int;
    /// libssh2's own declaration: the keys freed.
    safe fn libssh2_publickey_list_free(pkey: Publickey, keys: Listed);
    /// An `SSH2 Publickey Subsystem` resource over `pkey`, which keeps the
    /// session's resource, into `out`.
    safe fn swr_ssh2_publickey_open(
        session_resource: &Value,
        session: Session,
        pkey: Publickey,
        out: ReturnSlot<'_>,
    );
    /// The subsystem an `SSH2 Publickey Subsystem` resource holds, or null with
    /// Zend's error raised for another resource.
    safe fn swr_ssh2_publickey(resource: &Value) -> Option<Publickey>;
    /// `libssh2_publickey_add_ex` of the `name` and `blob` key, with the
    /// attributes packed in `packed` as [`pack`] writes them; -1, without the
    /// call, when `packed` is malformed.
    safe fn swr_ssh2_publickey_add<'n, 'b, 'p>(
        pkey: Publickey,
        name: In<'n, u8>,
        name_len: Count<'n>,
        blob: In<'b, u8>,
        blob_len: Count<'b>,
        overwrite: bool,
        packed: In<'p, u8>,
        packed_len: Count<'p>,
    ) -> c_int;
    /// The name (or, with `blob`, the blob) of the `index`th listed key, as a
    /// string into `out`; false past the list's end.
    safe fn swr_ssh2_publickey_key(
        keys: &Listed,
        index: c_ulong,
        blob: bool,
        out: &mut Owned,
    ) -> bool;
    /// The name (or, with `value`, the value) of the `index`th listed key's
    /// `attribute`th attribute, as a string into `out`; false past the list's
    /// or the key's attributes' end.
    safe fn swr_ssh2_publickey_attribute(
        keys: &Listed,
        index: c_ulong,
        attribute: c_ulong,
        value: bool,
        out: &mut Owned,
    ) -> bool;
}

/// Bytes up to their first NUL, as C's `%s` printed them.
fn until_nul(bytes: &[u8]) -> &[u8] {
    bytes.split(|&byte| byte == 0).next().unwrap_or_default()
}

/// `ssh2_publickey_init(resource $session): resource|false`: the session's
/// publickey subsystem, or C's warning with libssh2's last error and false.
pub(super) fn init(call: &Call<'_>) {
    if call.arity(1, 1).is_err() {
        return;
    }
    let Ok(resource) = call.resource(1) else {
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
    let Some(pkey) = libssh2_publickey_init(session) else {
        let (mut message, mut length) = (None, 0);
        let code = libssh2_session_last_error(session, &mut message, &mut length, 0);
        let message = message.map_or(&[][..], |text| text.get().to_bytes());
        warn_bytes(&[
            b"Unable to initialize publickey subsystem(",
            code.to_string().as_bytes(),
            b") ",
            message,
        ]);
        call.set_bool(false);
        return;
    };
    swr_ssh2_publickey_open(resource, session, pkey, call.return_slot());
}

/// The `s` arguments 2 and 3, `$algoname` and `$blob`, parsed in turn.
fn key_arguments<'a>(call: &'a Call<'_>) -> Option<(&'a ZStr, &'a ZStr)> {
    let Ok(Some(algo)) = call.zstr(2) else {
        return None;
    };
    let Ok(Some(blob)) = call.zstr(3) else {
        return None;
    };
    Some((algo, blob))
}

/// The subsystem of the resource argument 1, or false returned.
fn subsystem(call: &Call<'_>, resource: &Value) -> Option<Publickey> {
    let pkey = swr_ssh2_publickey(resource);
    if pkey.is_none() {
        call.set_bool(false);
    }
    pkey
}

/// One attribute: whether it's mandatory, its name and its value.
type Attribute<'a> = (bool, &'a [u8], Vec<u8>);

/// `$attributes`' entries as C read them, with its warnings: an integer key
/// and the key `*` skipped; a key starting `*` mandatory, named by what
/// follows; each name without its last byte, as C's lengths dropped it; each
/// value converted to a string.
fn attributes(attributes: &Array) -> Vec<Attribute<'_>> {
    let mut kept = Vec::new();
    for (key, value) in attributes.entries() {
        let Some(key) = key else {
            warn("Malformed attirbute array, contains numeric index");
            continue;
        };
        if key == b"*" {
            warn("Empty attribute key");
            continue;
        }
        let value = value.string().bytes().to_vec();
        let without_last = key.get(..key.len().saturating_sub(1)).unwrap_or_default();
        kept.push(match without_last.split_first() {
            Some((b'*', name)) => (true, name, value),
            _ => (false, without_last, value),
        });
    }
    kept
}

/// The attributes packed for [`swr_ssh2_publickey_add`]: each a mandatory
/// byte, its name's and value's lengths (`size_t`), then their bytes.
fn pack(attributes: &[Attribute<'_>]) -> Vec<u8> {
    let mut packed = Vec::new();
    for (mandatory, name, value) in attributes {
        packed.push(u8::from(*mandatory));
        packed.extend_from_slice(&name.len().to_ne_bytes());
        packed.extend_from_slice(&value.len().to_ne_bytes());
        packed.extend_from_slice(name);
        packed.extend_from_slice(value);
    }
    packed
}

/// `ssh2_publickey_add(resource $pkey, string $algoname, string $blob, bool
/// $overwrite = false, array $attributes = []): bool`: the key added, or C's
/// warning and false.
pub(super) fn add(call: &Call<'_>) {
    if call.arity(3, 5).is_err() {
        return;
    }
    let Ok(resource) = call.resource(1) else {
        return;
    };
    let Some((algo, blob)) = key_arguments(call) else {
        return;
    };
    let Ok(overwrite) = call.bool(4, false) else {
        return;
    };
    let given = if call.num_args() >= 5 {
        match call.array_ref(5) {
            Ok(given) => Some(given),
            Err(_) => return,
        }
    } else {
        None
    };
    let Some(pkey) = subsystem(call, resource) else {
        return;
    };
    let kept = given.map_or_default(attributes);
    let packed = pack(&kept);
    let added = lend_in(algo.bytes(), |name, name_len| {
        lend_in(blob.bytes(), |blob, blob_len| {
            lend_in(&packed, |packed, packed_len| {
                swr_ssh2_publickey_add(
                    pkey, name, name_len, blob, blob_len, overwrite, packed, packed_len,
                )
            })
        })
    });
    if added != 0 {
        warn_bytes(&[b"Unable to add ", until_nul(algo.bytes()), b" key"]);
        call.set_bool(false);
        return;
    }
    call.set_bool(true);
}

/// `ssh2_publickey_remove(resource $pkey, string $algoname, string $blob):
/// bool`: the key removed, or C's warning and false.
pub(super) fn remove(call: &Call<'_>) {
    if call.arity(3, 3).is_err() {
        return;
    }
    let Ok(resource) = call.resource(1) else {
        return;
    };
    let Some((algo, blob)) = key_arguments(call) else {
        return;
    };
    let Some(pkey) = subsystem(call, resource) else {
        return;
    };
    let removed = lend_in(algo.bytes(), |name, name_len| {
        lend_in(blob.bytes(), |blob, blob_len| {
            libssh2_publickey_remove_ex(pkey, name, name_len, blob, blob_len)
        })
    });
    if removed != 0 {
        warn_bytes(&[b"Unable to remove ", until_nul(algo.bytes()), b" key"]);
        call.set_bool(false);
        return;
    }
    call.set_bool(true);
}

/// The `index`th listed key as C's array: its `name`, `blob` and `attrs`, the
/// attributes keyed by name (the first of a repeated name kept, as
/// `zend_hash_add` did).
fn key(keys: &Listed, index: c_ulong) -> Option<Owned> {
    let mut entry = OwnedArray::default();
    for (field, blob) in [(&b"name"[..], false), (b"blob", true)] {
        let mut part = Owned::null();
        if !swr_ssh2_publickey_key(keys, index, blob, &mut part) {
            return None;
        }
        entry.insert(field, part);
    }
    let mut attrs = OwnedArray::default();
    let mut seen: Vec<Vec<u8>> = Vec::new();
    for attribute in 0.. {
        let (mut name, mut value) = (Owned::null(), Owned::null());
        if !swr_ssh2_publickey_attribute(keys, index, attribute, false, &mut name)
            || !swr_ssh2_publickey_attribute(keys, index, attribute, true, &mut value)
        {
            break;
        }
        let Some(name) = name.str() else {
            break;
        };
        if seen.iter().any(|earlier| earlier == name.bytes()) {
            continue;
        }
        seen.push(name.bytes().to_vec());
        attrs.add_new(name, value);
    }
    entry.insert(b"attrs", attrs.into_owned());
    Some(entry.into_owned())
}

/// `ssh2_publickey_list(resource $pkey): array|false`: the server's keys, or
/// C's warning and false.
pub(super) fn list(call: &Call<'_>) {
    if call.arity(1, 1).is_err() {
        return;
    }
    let Ok(resource) = call.resource(1) else {
        return;
    };
    let Some(pkey) = subsystem(call, resource) else {
        return;
    };
    let (mut count, mut keys) = (0, None);
    let fetched = libssh2_publickey_list_fetch(pkey, &mut count, &mut keys);
    if fetched != 0 {
        warn("Unable to list keys on remote server");
        call.set_bool(false);
        return;
    }
    let mut listed = OwnedArray::default();
    let Some(keys) = keys else {
        // No list to read or free.
        call.set_owned(listed.into_owned());
        return;
    };
    for index in 0..count {
        let Some(entry) = key(&keys, index) else {
            break;
        };
        listed.push(entry);
    }
    libssh2_publickey_list_free(pkey, keys);
    call.set_owned(listed.into_owned());
}
