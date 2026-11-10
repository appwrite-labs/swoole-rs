//! Shared SSL context/configuration ownership: `swoole::SSLContext` is this owner,
//! reference counted (`swr_v1_tls_owner_new`, `_clone`, `_free`). Native reads its
//! scalar fields and string slots through `swr_v1_tls_owner_view`; OpenSSL
//! operations read the authoritative Rust owner.
use crate::context::{PasswordBinding, copy_password, create_context};
use crate::{TlsConfig, TlsStr, Warn};
use openssl_sys::{SSL_CTX, SSL_CTX_free};
use std::{
    cell::UnsafeCell,
    ffi::{CStr, c_char, c_int, c_void},
    ptr,
    sync::Arc,
};
use swoole_rs_corebase::StringSlot;

const STRING_COUNT: usize = 11;
const PASSPHRASE: usize = 6;
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct OwnedTlsFields {
    pub http: u8,
    pub http_v2: u8,
    pub prefer_server_ciphers: u8,
    pub session_tickets: u8,
    pub stapling: u8,
    pub stapling_verify: u8,
    pub disable_tls_host_name: u8,
    pub grease: u8,
    pub verify_depth: u8,
    pub disable_compress: u8,
    pub verify_peer: u8,
    pub allow_self_signed: u8,
    pub protocols: u32,
    pub create_flag: u8,
    pub context: *mut SSL_CTX,
}
pub struct OwnedTlsContext {
    fields: UnsafeCell<OwnedTlsFields>,
    strings: [Arc<StringSlot>; STRING_COUNT],
    context_password: UnsafeCell<Option<Arc<StringSlot>>>,
    warn: Warn,
}
// SAFETY: shared ownership exposes only unsafe, externally synchronized field
// and configuration APIs. Safe Rust can only retain or drop an Arc.
unsafe impl Send for OwnedTlsContext {}
// SAFETY: callers serialize mutations against borrowed native fields/OpenSSL
// configuration; no safe Rust method exposes the UnsafeCell contents.
unsafe impl Sync for OwnedTlsContext {}
impl Drop for OwnedTlsContext {
    fn drop(&mut self) {
        let ctx = self.fields.get_mut().context;
        if !ctx.is_null() {
            // SAFETY: the last owner releases its OpenSSL reference; all borrowed API uses ended.
            unsafe { SSL_CTX_free(ctx) };
        }
    }
}
#[repr(C)]
pub struct OwnedTlsPrepared {
    pub owner: *const OwnedTlsContext,
    pub fields: *mut OwnedTlsFields,
    pub strings: [*const StringSlot; STRING_COUNT],
}
// The layout native's swr_v1_tls_owner_fields asserts too (swoole_rs_tls_owner.h).
#[cfg(target_pointer_width = "64")]
const _: () = assert!(
    size_of::<OwnedTlsFields>() == 32
        && std::mem::offset_of!(OwnedTlsFields, allow_self_signed) == 11
        && std::mem::offset_of!(OwnedTlsFields, protocols) == 12
        && std::mem::offset_of!(OwnedTlsFields, create_flag) == 16
        && std::mem::offset_of!(OwnedTlsFields, context) == 24,
    "native swr_v1_tls_owner_fields' layout"
);
/// `SSLContext`'s warnings, `function(): message` at warning level.
#[unsafe(no_mangle)]
#[expect(
    clippy::not_unsafe_ptr_arg_deref,
    reason = "a `Warn`, which owners call only with NUL-terminated text"
)]
pub extern "C" fn swr_v1_tls_warning(function: *const c_char, message: *const c_char) {
    // SAFETY: the owner's callers pass NUL-terminated names.
    let function = unsafe { CStr::from_ptr(function) };
    // SAFETY: and NUL-terminated messages.
    let message = unsafe { CStr::from_ptr(message) }.to_string_lossy();
    swoole_rs_corebase::log::warning(function, format_args!("{message}"));
}
fn prepared(owner: &OwnedTlsContext) -> OwnedTlsPrepared {
    OwnedTlsPrepared {
        owner,
        fields: owner.fields.get(),
        strings: owner.strings.each_ref().map(Arc::as_ptr),
    }
}
/// A new owner, holding one reference, and its view.
/// warn is a nonthrowing native logging leaf that outlives the shared owner.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_tls_owner_new(warn: Warn) -> OwnedTlsPrepared {
    let owner = Arc::new(OwnedTlsContext {
        fields: UnsafeCell::new(OwnedTlsFields::default()),
        strings: std::array::from_fn(|_| StringSlot::empty()),
        context_password: UnsafeCell::new(None),
        warn,
    });
    // SAFETY: the reference just made, which the caller now holds.
    prepared(unsafe { &*Arc::into_raw(owner) })
}
/// `owner`'s scalar fields and string slots, valid while a reference is held.
/// # Safety
/// owner is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_tls_owner_view(owner: *const OwnedTlsContext) -> OwnedTlsPrepared {
    // SAFETY: as the caller promises.
    prepared(unsafe { &*owner })
}
/// # Safety
/// owner is live; each returned ownership handle is released once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_tls_owner_clone(
    owner: *const OwnedTlsContext,
) -> *const OwnedTlsContext {
    // SAFETY: caller holds a live ownership handle throughout this increment.
    unsafe { Arc::increment_strong_count(owner) };
    owner
}
/// # Safety
/// owner is null or one owned Arc from new/clone. Borrowed fields/OpenSSL uses
/// end before the last ownership handle is released.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_tls_owner_free(owner: *const OwnedTlsContext) {
    if !owner.is_null() {
        // SAFETY: consumes exactly this owned Arc.
        unsafe { drop(Arc::from_raw(owner)) };
    }
}
/// OpenSSL's reason text for `error`, or `(null)` as printf shows a missing one.
#[must_use]
pub fn reason_text(error: std::os::raw::c_ulong) -> String {
    // SAFETY: OpenSSL's static string table (null for an unknown code).
    let reason = unsafe { openssl_sys::ERR_reason_error_string(error) };
    if reason.is_null() {
        return "(null)".to_owned();
    }
    // SAFETY: a NUL-terminated static string.
    unsafe { std::ffi::CStr::from_ptr(reason) }
        .to_string_lossy()
        .into_owned()
}
/// `tls_host_name`'s slot.
const TLS_HOST_NAME: usize = 8;
/// The owner's scalar fields, copied.
/// # Safety
/// owner is live and no field write runs during the call.
#[must_use]
pub unsafe fn owner_fields(owner: *const OwnedTlsContext) -> OwnedTlsFields {
    // SAFETY: as the caller promises.
    let owner = unsafe { &*owner };
    // SAFETY: as the caller promises; a copy, no borrow is kept.
    unsafe { *owner.fields.get() }
}
/// `tls_host_name`'s NUL-terminated bytes.
/// # Safety
/// owner is live; the name does not change while the view is used.
#[must_use]
pub unsafe fn owner_tls_host_name(
    owner: *const OwnedTlsContext,
) -> swoole_rs_corebase::CoreStringView {
    // SAFETY: as the caller promises.
    let owner = unsafe { &*owner };
    // SAFETY: the owner's stable slot, unchanged while the view is used.
    unsafe {
        swoole_rs_corebase::swr_v1_core_string_view(Arc::as_ptr(&owner.strings[TLS_HOST_NAME]))
    }
}
/// # Safety
/// owner is live; neither its strings nor scalar slots change during the operation.
unsafe fn configuration(owner: *const OwnedTlsContext) -> TlsConfig {
    // SAFETY: the caller lends the live owner for this short snapshot.
    let owner = unsafe { &*owner };
    let strings = owner.strings.each_ref().map(|slot| {
        // SAFETY: owner's stable string slots remain live and unchanged for the returned view.
        unsafe { swoole_rs_corebase::swr_v1_core_string_view(Arc::as_ptr(slot)) }
    });
    // SAFETY: field observation is externally synchronized.
    let fields = unsafe { &*owner.fields.get() };
    TlsConfig {
        ciphers: strings[0].into(),
        ecdh_curve: strings[1].into(),
        dhparam: strings[3].into(),
        cert_file: strings[4].into(),
        key_file: strings[5].into(),
        passphrase: strings[PASSPHRASE].into(),
        client_cert_file: strings[7].into(),
        cafile: strings[9].into(),
        capath: strings[10].into(),
        protocols: fields.protocols,
        verify_depth: fields.verify_depth,
        http: fields.http != 0,
        http_v2: fields.http_v2 != 0,
        prefer_server_ciphers: fields.prefer_server_ciphers != 0,
        disable_compress: fields.disable_compress != 0,
        verify_peer: fields.verify_peer != 0,
        warn: owner.warn,
        owner: ptr::from_ref(owner).cast(),
        passphrase_of: passphrase_view,
    }
}
extern "C" fn passphrase_view(owner: *const c_void) -> TlsStr {
    // SAFETY: used only with the live Rust owner placed in the local configuration view.
    let owner = unsafe { &*owner.cast::<OwnedTlsContext>() };
    // SAFETY: the owner's passphrase slot remains live and externally synchronized.
    unsafe { swoole_rs_corebase::swr_v1_core_string_view(Arc::as_ptr(&owner.strings[PASSPHRASE])) }
        .into()
}
extern "C" fn slot_password(
    buffer: *mut c_char,
    capacity: c_int,
    _verify: c_int,
    slot: *mut c_void,
) -> c_int {
    // SAFETY: the shared context owns this stable slot for its borrowed OpenSSL lifetime.
    let view = unsafe { swoole_rs_corebase::swr_v1_core_string_view(slot.cast()) };
    // SAFETY: OpenSSL lends the output buffer and the live slot lends its exact bytes.
    unsafe { copy_password(buffer, capacity, view.into()) }
}
/// # Safety
/// owner is live/exclusive, and its logger does not throw.
pub unsafe fn swr_v1_tls_owner_create(owner: *const OwnedTlsContext) -> bool {
    // SAFETY: snapshot raw views without retaining a record reference across OpenSSL/log callbacks.
    let mut config = unsafe { configuration(owner) };
    // SAFETY: the live Arc owns interior-mutable slots; no exclusive borrow survives callbacks.
    let state = unsafe { &*owner };
    let slot = Arc::as_ptr(&state.strings[PASSPHRASE]);
    let retained_password = if config.passphrase.length == 0 {
        None
    } else {
        Some(Arc::clone(&state.strings[PASSPHRASE]))
    };
    let password = PasswordBinding {
        userdata: slot.cast_mut().cast(),
        callback: slot_password,
    };
    // SAFETY: local configuration, stable string slots and a live password binding.
    let ctx = unsafe { create_context(&raw mut config, Some(password)) };
    if !ctx.is_null() {
        // SAFETY: each session registers its canonical network socket before callbacks run.
        unsafe { crate::info::install(ctx) };
    }
    // SAFETY: obtain the writable stable record slots after callbacks have returned.
    let fields = unsafe { (*owner).fields.get() };
    // SAFETY: synchronized owner observation; the old OpenSSL reference is still owned.
    let previous = unsafe { (*fields).context };
    // SAFETY: no callback is active; publish the resulting OpenSSL reference.
    unsafe { (*fields).context = ctx };
    // SAFETY: create's normalized protocol mask belongs to the same owner.
    unsafe { (*fields).protocols = config.protocols };
    if !previous.is_null() {
        // SAFETY: replacement releases the previous owned context reference.
        unsafe { SSL_CTX_free(previous) };
    }
    // SAFETY: the new context's live password data is retained independently of native views.
    unsafe { *state.context_password.get() = retained_password };
    !ctx.is_null()
}
/// `bool SSLContext::create()`, which initializes OpenSSL first.
///
/// # Safety
/// `context` is a live owner, as for `swr_v1_tls_owner_create`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole10SSLContext6createEv")
)]
#[must_use]
pub unsafe extern "C" fn ssl_context_create(context: *const OwnedTlsContext) -> bool {
    crate::swoole_ssl_init();
    // SAFETY: as the caller promises.
    unsafe { swr_v1_tls_owner_create(context) }
}
/// Path access and assignment are one Rust operation. 1 access denied, otherwise
/// `string_set`'s status; only cert(4), key(5) and client certificate(7) are accepted.
/// # Safety
/// owner is live/exclusive; path is terminated and readable for length bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_tls_owner_set_path(
    owner: *const OwnedTlsContext,
    index: u32,
    path: *const c_char,
    length: usize,
) -> c_int {
    if !matches!(index, 4 | 5 | 7) {
        return -1;
    }
    // SAFETY: the caller supplies a terminated path for access(2).
    if unsafe { libc::access(path, libc::R_OK) } < 0 {
        return 1;
    }
    // SAFETY: the owner is live and index names one of its stable slots.
    let Some(slot) = (unsafe { &(*owner).strings }).get(index as usize) else {
        return -1;
    };
    let slot = Arc::as_ptr(slot);
    // SAFETY: forwards the synchronized owner and readable bytes/cache contract.
    unsafe { swoole_rs_corebase::swr_v1_core_string_set(slot, path, length) }
}

#[repr(C)]
pub struct OwnedTlsCopy {
    pub status: i32,
    pub index: u32,
}
/// Independent configuration values, with an owned OpenSSL reference when ready.
/// Native references keep their stable destination slots; cache failures are caught
/// before the affected string is published and rethrown by its native view.
/// # Safety
/// Both owners are live, synchronized and distinct (self-assignment is accepted).
/// Native cache callbacks catch exceptions and do not reenter.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_tls_owner_assign(
    destination: *const OwnedTlsContext,
    source: *const OwnedTlsContext,
) -> OwnedTlsCopy {
    if destination == source {
        return OwnedTlsCopy {
            status: 0,
            index: 0,
        };
    }
    // SAFETY: two live shared owners, exposing mutation only through interior slots.
    let destination = unsafe { &*destination };
    // SAFETY: source remains unchanged for the configuration copy.
    let source = unsafe { &*source };
    let slots = source.strings.iter().zip(&destination.strings);
    for (index, (from, to)) in slots.enumerate() {
        // SAFETY: the source owns this stable, unchanged string slot.
        let value = unsafe { swoole_rs_corebase::swr_v1_core_string_view(Arc::as_ptr(from)) };
        // SAFETY: the destination slot is exclusive and its cache catches exceptions.
        let status = unsafe {
            swoole_rs_corebase::swr_v1_core_string_set(Arc::as_ptr(to), value.data, value.length)
        };
        if status != 0 {
            return OwnedTlsCopy {
                status,
                index: u32::try_from(index).unwrap_or(0),
            };
        }
    }
    // SAFETY: copy scalar values after string allocation/cache updates have succeeded.
    let fields = unsafe { *source.fields.get() };
    // SAFETY: independent retention of the context's actual live password value.
    let password = unsafe { &*source.context_password.get() }.clone();
    if !fields.context.is_null() {
        // SAFETY: source owns a valid OpenSSL reference; destination acquires its own.
        if unsafe { openssl_sys::SSL_CTX_up_ref(fields.context) } != 1 {
            return OwnedTlsCopy {
                status: -2,
                index: 0,
            };
        }
    }
    // SAFETY: synchronized observation of the destination's previous owned reference.
    let previous = unsafe { (*destination.fields.get()).context };
    // SAFETY: slots are stable and exclusive; publish the copied scalar values/reference.
    unsafe { *destination.fields.get() = fields };
    if !previous.is_null() {
        // SAFETY: the destination releases its replaced OpenSSL reference.
        unsafe { SSL_CTX_free(previous) };
    }
    // SAFETY: release old password ownership only after releasing its OpenSSL reference.
    unsafe { *destination.context_password.get() = password };
    OwnedTlsCopy {
        status: 0,
        index: 0,
    }
}

/// Configure and create a coroutine socket's context from its authoritative owner.
/// 0 success, 1 unsupported DTLS, 2 context creation failure.
/// # Safety
/// owner and socket are live/exclusive; native TLS initialization has succeeded.
pub unsafe fn swr_v1_tls_owner_socket_context(
    owner: *const OwnedTlsContext,
    socket: *mut swoole_rs_reactor::SocketRecord,
    http2: bool,
    dtls_supported: bool,
) -> c_int {
    // SAFETY: the owner is live throughout the operation.
    let owner_ref = unsafe { &*owner };
    let fields = owner_ref.fields.get();
    // SAFETY: as the caller promises.
    let Some(socket) = (unsafe { swoole_rs_reactor::Socket::from_ptr(socket) }) else {
        return 2;
    };
    let kind = socket.read(|record| record.socket_type);
    if swoole_rs_reactor::type_is(kind, swoole_rs_reactor::CLASS_DGRAM) {
        if !dtls_supported {
            return 1;
        }
        socket.edit(|record| {
            record.dtls = 1;
            record.chunk_size = swoole_rs_corebase::constants::SW_SSL_BUFFER_SIZE;
        });
        // SAFETY: the live owner's fields are exclusive and stable.
        unsafe { (*fields).protocols = crate::SslVersion::SW_SSL_DTLS as u32 };
    }
    // SAFETY: a leaf update before OpenSSL callbacks may run.
    unsafe { (*fields).http_v2 = u8::from(http2) };
    // SAFETY: initialization and exclusive owner access are the caller contract.
    if !unsafe { swr_v1_tls_owner_create(owner) } {
        return 2;
    }
    socket.edit(|record| record.output.ssl_send = 1);
    0
}

#[repr(C)]
pub struct OwnedTlsSessionResult {
    pub status: c_int,
    pub error: std::os::raw::c_ulong,
}
/// Create and configure one coroutine TLS session, including moving write buffers
/// and the context/automatic hostname precedence. Native only formats diagnostics.
/// # Safety
/// All pointers are live/exclusive, hostname is stable, and TLS is initialized.
/// grease, when supplied by `BoringSSL`, is a nonthrowing SSL configuration leaf.
pub unsafe fn swr_v1_tls_owner_socket_session(
    owner: *const OwnedTlsContext,
    socket: *mut swoole_rs_reactor::SocketRecord,
    hostname: *const StringSlot,
    grease: Option<unsafe extern "C" fn(*mut openssl_sys::SSL, bool)>,
) -> OwnedTlsSessionResult {
    // SAFETY: as the caller promises.
    let Some(record) = (unsafe { swoole_rs_reactor::Socket::from_ptr(socket) }) else {
        return OwnedTlsSessionResult {
            status: 1,
            error: 0,
        };
    };
    if !record.read(|record| record.ssl).is_null() {
        return OwnedTlsSessionResult {
            status: 0,
            error: 0,
        };
    }
    // SAFETY: the owner stays live through this operation.
    let owner_ref = unsafe { &*owner };
    // SAFETY: exclusive leaf snapshot of the owner configuration.
    let fields = unsafe { *owner_ref.fields.get() };
    let mut error = 0;
    // The record's TLS group has `SocketTls`'s asserted C layout.
    let (tls, output, fd) = record.edit(|record| {
        (
            (&raw mut record.tls).cast::<crate::SocketTls>(),
            (&raw mut record.ssl).cast(),
            record.fd,
        )
    });
    // SAFETY: caller lends exclusive socket fields and a live owner/context.
    let status = unsafe {
        crate::swr_v1_socket_ssl_new(
            tls,
            fields.context,
            fd,
            0,
            crate::swr_v1_tls_connection_index(),
            socket.cast(),
            output,
            &raw mut error,
        )
    };
    if status != 0 {
        return OwnedTlsSessionResult { status, error };
    }
    // The preceding successful creation populated this session slot.
    let ssl = record.read(|record| record.ssl.cast::<openssl_sys::SSL>());
    if let Some(grease) = grease {
        // SAFETY: the optional native leaf accepts the live session and does not reenter.
        unsafe { grease(ssl, fields.grease != 0) };
    }
    // SAFETY: the owner keeps the explicit hostname slot alive throughout configuration.
    let explicit =
        unsafe { swoole_rs_corebase::swr_v1_core_string_view(Arc::as_ptr(&owner_ref.strings[8])) };
    let selected = if explicit.length != 0 {
        Some(explicit)
    } else if fields.disable_tls_host_name == 0 {
        // SAFETY: the enclosing socket lends its automatic hostname slot.
        let automatic = unsafe { swoole_rs_corebase::swr_v1_core_string_view(hostname) };
        (automatic.length != 0).then_some(automatic)
    } else {
        None
    };
    if let Some(name) = selected {
        // SAFETY: terminated owner bytes remain stable through this OpenSSL call.
        unsafe { openssl_sys::SSL_set_tlsext_host_name(ssl, name.data.cast_mut()) };
    }
    OwnedTlsSessionResult {
        status: 0,
        error: 0,
    }
}

/// A text setting the `SSLContext` setters (`set_ssl_*`) write, for
/// `Swoole\Client`, `Coroutine\Client`, `Coroutine\Socket` and `Server\Port`.
#[derive(Clone, Copy)]
pub enum Text {
    /// A readable file; a missing one is logged from `set_cert_file`.
    CertFile,
    /// As `CertFile`, from `set_key_file`.
    KeyFile,
    /// As `CertFile`, from `set_client_cert_file`.
    ClientCertFile,
    Passphrase,
    /// An empty name also turns the automatic one off.
    HostName,
    Cafile,
    Capath,
    Ciphers,
    EcdhCurve,
    DhParam,
}

/// A scalar setting, as [`Text`].
#[derive(Clone, Copy)]
pub enum Value {
    Protocols(i64),
    DisableCompress(bool),
    VerifyPeer(bool),
    AllowSelfSigned(bool),
    VerifyDepth(u8),
    Grease(u8),
    PreferServerCiphers(bool),
}

/// `set_cert_file`/`set_key_file`: false, logged from `origin`, when the file
/// can't be read.
/// # Safety
/// owner is live and exclusive.
unsafe fn set_file(
    owner: *const OwnedTlsContext,
    index: u32,
    path: &[u8],
    origin: &CStr,
    what: &str,
) -> bool {
    // access(2) reads up to the first NUL, as upstream's c_str() does.
    let name = path.split(|&b| b == 0).next().unwrap_or_default();
    let terminated = [name, b"\0"].concat();
    // SAFETY: the exclusive owner and a terminated path; the stored value keeps every byte.
    let status =
        unsafe { swr_v1_tls_owner_set_path(owner, index, terminated.as_ptr().cast(), path.len()) };
    match status {
        0 => true,
        1 => {
            let name = String::from_utf8_lossy(name);
            swoole_rs_corebase::log::warning(
                origin,
                format_args!("ssl {what} file[{name}] not found"),
            );
            false
        }
        // An allocation refusal, which std::bad_alloc ends.
        _ => std::process::abort(),
    }
}

/// Write the text setting into the owner: false for a certificate or key file
/// that can't be read.
/// # Safety
/// owner is live and exclusive.
pub unsafe fn set_text(owner: *const OwnedTlsContext, which: Text, value: &[u8]) -> bool {
    // SAFETY: the caller lends the live owner.
    let state = unsafe { &*owner };
    let slot = match which {
        // SAFETY: as the caller promises.
        Text::CertFile => return unsafe { set_file(owner, 4, value, c"set_cert_file", "cert") },
        // SAFETY: as the caller promises.
        Text::KeyFile => return unsafe { set_file(owner, 5, value, c"set_key_file", "key") },
        Text::ClientCertFile => {
            // SAFETY: as the caller promises.
            return unsafe { set_file(owner, 7, value, c"set_client_cert_file", "client cert") };
        }
        Text::Passphrase => &state.strings[PASSPHRASE],
        Text::HostName => &state.strings[8],
        Text::Cafile => &state.strings[9],
        Text::Capath => &state.strings[10],
        Text::Ciphers => &state.strings[0],
        Text::EcdhCurve => &state.strings[1],
        Text::DhParam => &state.strings[3],
    };
    // SAFETY: a stable slot of the exclusive owner; its cache catches exceptions.
    let status = unsafe {
        swoole_rs_corebase::swr_v1_core_string_set(
            Arc::as_ptr(slot),
            value.as_ptr().cast(),
            value.len(),
        )
    };
    if status != 0 {
        std::process::abort();
    }
    if matches!(which, Text::HostName) {
        // SAFETY: the exclusive caller serializes field writes.
        unsafe { (*state.fields.get()).disable_tls_host_name = value.is_empty().into() };
    }
    true
}

/// Write the scalar setting into the owner.
/// # Safety
/// owner is live and exclusive.
pub unsafe fn set_value(owner: *const OwnedTlsContext, value: Value) {
    // SAFETY: the caller lends the live owner.
    let state = unsafe { &*owner };
    // SAFETY: the exclusive caller serializes field writes.
    let fields = unsafe { &mut *state.fields.get() };
    match value {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "upstream assigns the long to a uint32_t"
        )]
        Value::Protocols(value) => fields.protocols = value as u32,
        Value::DisableCompress(value) => fields.disable_compress = value.into(),
        Value::VerifyPeer(value) => fields.verify_peer = value.into(),
        Value::AllowSelfSigned(value) => fields.allow_self_signed = value.into(),
        Value::VerifyDepth(value) => fields.verify_depth = value,
        Value::Grease(value) => fields.grease = value,
        Value::PreferServerCiphers(value) => fields.prefer_server_ciphers = value.into(),
    }
}

/// Whether the owner has a certificate file and a key file.
/// # Safety
/// owner is live.
pub unsafe fn has_files(owner: *const OwnedTlsContext) -> (bool, bool) {
    // SAFETY: the caller lends the live owner.
    let state = unsafe { &*owner };
    let set = |slot: &Arc<StringSlot>| {
        // SAFETY: a stable slot of the live owner.
        unsafe { swoole_rs_corebase::swr_v1_core_string_view(Arc::as_ptr(slot)) }.length != 0
    };
    (set(&state.strings[4]), set(&state.strings[5]))
}

/// `verify_peer` with no client certificate file, cafile or capath.
/// # Safety
/// owner is live.
pub unsafe fn verify_without_ca(owner: *const OwnedTlsContext) -> bool {
    // SAFETY: the caller lends the live owner.
    let state = unsafe { &*owner };
    let empty = |slot: &Arc<StringSlot>| {
        // SAFETY: a stable slot of the live owner.
        unsafe { swoole_rs_corebase::swr_v1_core_string_view(Arc::as_ptr(slot)) }.length == 0
    };
    // SAFETY: the caller serializes field writes against this read.
    let verify_peer = unsafe { &*state.fields.get() }.verify_peer != 0;
    verify_peer && empty(&state.strings[7]) && empty(&state.strings[9]) && empty(&state.strings[10])
}

#[cfg(test)]
mod tests {
    use super::*;
    extern "C" fn warning(_function: *const c_char, _message: *const c_char) {
        panic!("unexpected TLS warning");
    }
    unsafe extern "C" {
        fn SSL_CTX_get_default_passwd_cb(
            ctx: *mut SSL_CTX,
        ) -> Option<extern "C" fn(*mut c_char, c_int, c_int, *mut c_void) -> c_int>;
        fn SSL_CTX_get_default_passwd_cb_userdata(ctx: *mut SSL_CTX) -> *mut c_void;
    }
    /// Whether `ctx`'s password callback is `callback` with `data`.
    unsafe fn password_binding_matches(
        ctx: *mut SSL_CTX,
        data: *mut c_void,
        callback: extern "C" fn(*mut c_char, c_int, c_int, *mut c_void) -> c_int,
    ) -> bool {
        let actual = unsafe { SSL_CTX_get_default_passwd_cb(ctx) };
        let userdata = unsafe { SSL_CTX_get_default_passwd_cb_userdata(ctx) };
        actual.is_some_and(|actual| std::ptr::fn_addr_eq(actual, callback)) && userdata == data
    }
    #[test]
    fn client_settings_write_the_owner() {
        unsafe {
            let prepared = swr_v1_tls_owner_new(warning);
            let owner = prepared.owner;
            let text = |index: usize| {
                let view = swoole_rs_corebase::swr_v1_core_string_view(prepared.strings[index]);
                std::slice::from_raw_parts(view.data.cast::<u8>(), view.length).to_vec()
            };
            assert!(set_text(owner, Text::HostName, b"example.test"));
            assert_eq!(text(8), b"example.test");
            assert_eq!((*prepared.fields).disable_tls_host_name, 0);
            assert!(set_text(owner, Text::HostName, b""));
            assert_eq!((*prepared.fields).disable_tls_host_name, 1);
            assert!(set_text(owner, Text::Ciphers, b"HIGH"));
            assert_eq!(text(0), b"HIGH");
            assert!(!set_text(owner, Text::CertFile, b"/nonexistent/cert.pem"));
            assert_eq!(has_files(owner), (false, false));
            let manifest = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
            assert!(set_text(owner, Text::KeyFile, manifest.as_bytes()));
            assert_eq!(has_files(owner), (false, true));
            set_value(owner, Value::Protocols(1 << 3));
            set_value(owner, Value::VerifyDepth(7));
            set_value(owner, Value::VerifyPeer(true));
            assert_eq!((*prepared.fields).protocols, 1 << 3);
            assert_eq!((*prepared.fields).verify_depth, 7);
            assert_eq!((*prepared.fields).verify_peer, 1);
            swr_v1_tls_owner_free(owner);
        }
    }
    #[test]
    fn socket_session_hostname_precedence_and_record_identity() {
        use std::os::fd::AsRawFd;
        unsafe {
            crate::swr_v1_tls_init();
            let (_peer, stream) = std::os::unix::net::UnixStream::pair().unwrap();
            for (explicit, disabled, automatic, expected) in [
                (
                    "explicit.test",
                    false,
                    "automatic.test",
                    Some("explicit.test"),
                ),
                ("", false, "automatic.test", Some("automatic.test")),
                ("", true, "automatic.test", None),
                (
                    "explicit.test",
                    true,
                    "automatic.test",
                    Some("explicit.test"),
                ),
                ("", false, "", None),
            ] {
                let prepared = swr_v1_tls_owner_new(warning);
                assert_eq!(
                    swoole_rs_corebase::swr_v1_core_string_set(
                        prepared.strings[8],
                        explicit.as_ptr().cast(),
                        explicit.len()
                    ),
                    0
                );
                (*prepared.fields).disable_tls_host_name = u8::from(disabled);
                let hostname = StringSlot::empty();
                assert_eq!(
                    swoole_rs_corebase::swr_v1_core_string_set(
                        Arc::as_ptr(&hostname),
                        automatic.as_ptr().cast(),
                        automatic.len()
                    ),
                    0
                );
                let socket =
                    swoole_rs_reactor::SocketRecord::with_fd(stream.as_raw_fd(), 6).into_raw();
                assert_eq!(
                    swr_v1_tls_owner_socket_context(prepared.owner, socket, true, true),
                    0
                );
                assert_eq!((*socket).output.ssl_send, 1);
                assert_eq!((*prepared.fields).http_v2, 1);
                let result = swr_v1_tls_owner_socket_session(
                    prepared.owner,
                    socket,
                    Arc::as_ptr(&hostname),
                    None,
                );
                assert_eq!(result.status, 0);
                let ssl = (*socket).ssl.cast::<openssl_sys::SSL>();
                assert_eq!(
                    openssl_sys::SSL_get_ex_data(ssl, crate::swr_v1_tls_connection_index()),
                    socket.cast()
                );
                let selected =
                    openssl_sys::SSL_get_servername(ssl, openssl_sys::TLSEXT_NAMETYPE_host_name);
                assert_eq!(
                    (!selected.is_null())
                        .then(|| std::ffi::CStr::from_ptr(selected).to_str().unwrap()),
                    expected
                );
                assert_ne!(
                    openssl_sys::SSL_ctrl(ssl, openssl_sys::SSL_CTRL_MODE, 0, ptr::null_mut())
                        & openssl_sys::SSL_MODE_ACCEPT_MOVING_WRITE_BUFFER,
                    0
                );
                assert_eq!(
                    swr_v1_tls_owner_socket_session(ptr::null(), socket, ptr::null(), None).status,
                    0
                );
                openssl_sys::SSL_free(ssl);
                swoole_rs_reactor::SocketRecord::free(socket);
                swr_v1_tls_owner_free(prepared.owner);
            }
        }
    }
    #[test]
    fn copied_ready_context_retains_independent_configuration_and_live_password() {
        unsafe {
            let source = swr_v1_tls_owner_new(warning);
            let destination = swr_v1_tls_owner_new(warning);
            assert_eq!(
                swoole_rs_corebase::swr_v1_core_string_set(
                    source.strings[PASSPHRASE],
                    c"source".as_ptr(),
                    6
                ),
                0
            );
            assert!(swr_v1_tls_owner_create(source.owner));
            let context = (*source.fields).context;
            assert_eq!(
                swr_v1_tls_owner_assign(destination.owner, source.owner).status,
                0
            );
            assert_eq!((*destination.fields).context, context);
            assert_eq!(
                swoole_rs_corebase::swr_v1_core_string_set(
                    destination.strings[PASSPHRASE],
                    c"copy".as_ptr(),
                    4
                ),
                0
            );
            assert_eq!(
                std::ffi::CStr::from_ptr(
                    swoole_rs_corebase::swr_v1_core_string_view(source.strings[PASSPHRASE]).data
                )
                .to_bytes(),
                b"source"
            );
            swr_v1_tls_owner_free(source.owner);
            let password = (*destination.owner).context_password.get();
            let slot = Arc::as_ptr((&*password).as_ref().unwrap());
            let mut output = [0u8; 16];
            assert_eq!(
                slot_password(output.as_mut_ptr().cast(), 16, 0, slot.cast_mut().cast()),
                6
            );
            assert_eq!(&output[..7], b"source\0");
            assert_eq!(
                openssl_sys::SSL_CTX_get_options(context),
                openssl_sys::SSL_CTX_get_options((*destination.fields).context)
            );
            swr_v1_tls_owner_free(destination.owner);
        }
    }
    #[test]
    fn shared_context_reads_the_live_passphrase_after_the_first_reference_goes() {
        unsafe {
            let prepared = swr_v1_tls_owner_new(warning);
            let owner = prepared.owner;
            let slot = prepared.strings[PASSPHRASE].cast_mut().cast();
            assert_eq!(
                swoole_rs_corebase::swr_v1_core_string_set(
                    prepared.strings[PASSPHRASE],
                    c"initial".as_ptr(),
                    7
                ),
                0
            );
            assert!(swr_v1_tls_owner_create(owner));
            let ctx = (*prepared.fields).context;
            assert_eq!((*prepared.fields).protocols, 126);
            assert!(password_binding_matches(ctx, slot, slot_password));
            for value in [b"swoole".as_slice(), b"a\0b", b"", b"1234567"] {
                assert_eq!(
                    swoole_rs_corebase::swr_v1_core_string_set(
                        prepared.strings[PASSPHRASE],
                        value.as_ptr().cast(),
                        value.len()
                    ),
                    0
                );
                for capacity in [-8, -1, 0, 1, 2, 3, 4, 7, 8, 15, 16] {
                    let mut bytes = [0x55u8; 16];
                    let mut expected = bytes;
                    let length = if !value.is_empty()
                        && c_int::try_from(value.len()).unwrap() < capacity - 1
                    {
                        value.len()
                    } else {
                        0
                    };
                    if length != 0 {
                        expected[..length].copy_from_slice(value);
                        expected[length] = 0;
                    }
                    assert_eq!(
                        slot_password(bytes.as_mut_ptr().cast(), capacity, 0, slot),
                        c_int::try_from(length).unwrap()
                    );
                    assert_eq!(bytes, expected);
                }
            }
            let retained = swr_v1_tls_owner_clone(owner);
            swr_v1_tls_owner_free(owner);
            assert_eq!(
                swoole_rs_corebase::swr_v1_core_string_set(
                    prepared.strings[PASSPHRASE],
                    c"retained".as_ptr(),
                    8
                ),
                0
            );
            let mut bytes = [0u8; 16];
            assert_eq!(slot_password(bytes.as_mut_ptr().cast(), 16, 0, slot), 8);
            assert_eq!(&bytes[..9], b"retained\0");
            swr_v1_tls_owner_free(retained);
        }
    }
}
