//! Shared SSL context/configuration ownership: `swoole::SSLContext` is this owner,
//! reference counted (`swr_v1_tls_owner_new`, `_clone`, `_free`). Native reads its
//! scalar fields and string slots through `swr_v1_tls_owner_view`; OpenSSL
//! operations read the authoritative Rust owner.
use crate::context::{PasswordBinding, copy_password, create_context};
use crate::{TlsConfig, Warn};
use openssl_sys::{SSL_CTX, SSL_CTX_free};
use std::{
    cell::UnsafeCell,
    ffi::{CStr, c_char, c_int, c_void},
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
pub fn tls_warning(function: &CStr, message: &str) {
    swoole_rs_corebase::log::warning(function, format_args!("{message}"));
}
fn prepared(owner: &OwnedTlsContext) -> OwnedTlsPrepared {
    OwnedTlsPrepared {
        owner,
        fields: owner.fields.get(),
        strings: owner.strings.each_ref().map(Arc::as_ptr),
    }
}
/// A new owner that warns through [`tls_warning`], holding one reference, and its view.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_tls_owner_new() -> OwnedTlsPrepared {
    owner_new(tls_warning)
}
/// A new owner that warns through `warn`, holding one reference, and its view.
#[must_use]
pub fn owner_new(warn: Warn) -> OwnedTlsPrepared {
    let owner = Arc::new(OwnedTlsContext {
        fields: UnsafeCell::new(OwnedTlsFields::default()),
        strings: std::array::from_fn(|_| StringSlot::empty()),
        context_password: UnsafeCell::new(None),
        warn,
    });
    let view = prepared(&owner);
    OwnedTlsPrepared {
        // The Arc's own pointer, which `swr_v1_tls_owner_free` can turn back into the
        // Arc: one taken from a reference covers only the context, not its counts.
        owner: Arc::into_raw(owner),
        ..view
    }
}
/// `owner`'s scalar fields and string slots, valid while a reference is held.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_tls_owner_view(owner: &OwnedTlsContext) -> OwnedTlsPrepared {
    prepared(owner)
}
/// Another ownership handle on `owner`, for `swr_v1_tls_owner_free` to release once.
///
/// # Safety
/// owner is a live ownership handle from new/clone (the Arc's pointer, not one taken
/// from a reference, which would not reach the Arc's counts).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_tls_owner_clone(
    owner: *const OwnedTlsContext,
) -> *const OwnedTlsContext {
    // SAFETY: as the caller promises: an Arc `swr_v1_tls_owner_new` made, live
    // throughout this increment.
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
/// One counted reference to an owner, held as native code holds an `SSLContext *`:
/// the Arc's own pointer, so a nullable field of it is that pointer. Cloning takes
/// another reference and dropping releases this one.
#[repr(transparent)]
pub struct TlsContext(core::ptr::NonNull<OwnedTlsContext>);
impl TlsContext {
    /// A new owner that warns through [`tls_warning`].
    #[must_use]
    pub fn new() -> Self {
        let owner = core::ptr::NonNull::new(swr_v1_tls_owner_new().owner.cast_mut());
        Self(owner.unwrap_or_else(|| std::process::abort()))
    }
    /// The owner, for native code and the raw-owner API; the handle keeps it live.
    #[must_use]
    pub fn as_ptr(&self) -> *const OwnedTlsContext {
        self.0.as_ptr()
    }
}
impl Default for TlsContext {
    fn default() -> Self {
        Self::new()
    }
}
impl core::ops::Deref for TlsContext {
    type Target = OwnedTlsContext;
    fn deref(&self) -> &OwnedTlsContext {
        // SAFETY: the reference this handle holds keeps the owner live while it is borrowed.
        unsafe { self.0.as_ref() }
    }
}
impl Clone for TlsContext {
    fn clone(&self) -> Self {
        // SAFETY: the Arc's own pointer, live through the reference this handle holds.
        unsafe { Arc::increment_strong_count(self.0.as_ptr()) };
        Self(self.0)
    }
}
impl Drop for TlsContext {
    fn drop(&mut self) {
        // SAFETY: this handle's one reference to the Arc, released once.
        unsafe { drop(Arc::from_raw(self.0.as_ptr())) };
    }
}
/// OpenSSL's reason text for `error`, or `(null)` as printf shows a missing one.
#[must_use]
pub fn reason_text(error: std::os::raw::c_ulong) -> String {
    crate::reason_string(error).map_or_else(
        || "(null)".into(),
        |reason| reason.to_string_lossy().into_owned(),
    )
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
/// `tls_host_name`'s NUL-terminated bytes, read while the name stays unchanged.
#[must_use]
pub fn owner_tls_host_name(owner: &OwnedTlsContext) -> swoole_rs_corebase::CoreStringView {
    owner.strings[TLS_HOST_NAME].view()
}
/// The owner's strings, as views, and the scalar `fields` copied from it.
fn configuration(owner: &OwnedTlsContext, fields: OwnedTlsFields) -> TlsConfig {
    let strings = owner.strings.each_ref().map(|slot| slot.view());
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
    }
}
/// # Safety
/// `slot` is the owner's live passphrase slot (the binding's userdata) and `buffer`
/// holds `capacity` writable bytes, as OpenSSL's password callback lends them.
unsafe extern "C" fn slot_password(
    buffer: *mut c_char,
    capacity: c_int,
    _verify: c_int,
    slot: *mut c_void,
) -> c_int {
    // SAFETY: per the caller.
    let passphrase = unsafe { &*slot.cast::<StringSlot>() }.bytes_with_nul();
    // SAFETY: per the caller.
    unsafe { copy_password(buffer, capacity, passphrase) }
}
/// # Safety
/// owner is live/exclusive, and its logger does not throw.
pub unsafe fn swr_v1_tls_owner_create(owner: *const OwnedTlsContext) -> bool {
    // SAFETY: the live Arc owns interior-mutable slots; no exclusive borrow survives callbacks.
    let state = unsafe { &*owner };
    // SAFETY: the exclusive caller serializes field writes; a copy, so no borrow of the
    // fields is kept across OpenSSL/log callbacks.
    let mut config = configuration(state, unsafe { *state.fields.get() });
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
    let ctx = unsafe { create_context(&mut config, Some(password)) };
    if !ctx.is_null() {
        // SAFETY: the new context; each of its sessions registers its canonical network
        // socket (`info`'s contract) before callbacks run.
        unsafe { crate::info::SSL_CTX_set_info_callback(ctx, Some(crate::info::info)) };
    }
    // SAFETY: the callbacks have returned, and the caller's exclusive owner lends its
    // fields for these two writes.
    let fields = unsafe { &mut *state.fields.get() };
    // The old OpenSSL reference, still owned, gives way to the new one.
    let previous = std::mem::replace(&mut fields.context, ctx);
    fields.protocols = config.protocols;
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
        let value = from.bytes();
        // SAFETY: the destination slot is exclusive (a distinct owner) and the source's
        // bytes stay unchanged while they are copied.
        let status = unsafe { to.set_shared(value) };
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
    // SAFETY: slots are stable and exclusive; publish the copied scalar values and
    // reference, taking back the destination's previous one.
    let previous = std::mem::replace(unsafe { &mut *destination.fields.get() }, fields).context;
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
/// owner is live/exclusive; native TLS initialization has succeeded.
pub unsafe fn swr_v1_tls_owner_socket_context(
    owner: *const OwnedTlsContext,
    socket: swoole_rs_reactor::Socket<'_>,
    http2: bool,
    dtls_supported: bool,
) -> c_int {
    // SAFETY: the owner is live throughout the operation.
    let owner_ref = unsafe { &*owner };
    let kind = socket.read(|record| record.socket_type);
    let dtls = swoole_rs_reactor::type_is(kind, swoole_rs_reactor::CLASS_DGRAM);
    if dtls && !dtls_supported {
        return 1;
    }
    {
        // SAFETY: the exclusive owner lends its fields for these leaf updates, before
        // OpenSSL callbacks may run.
        let fields = unsafe { &mut *owner_ref.fields.get() };
        if dtls {
            fields.protocols = crate::SslVersion::SW_SSL_DTLS as u32;
        }
        fields.http_v2 = u8::from(http2);
    }
    if dtls {
        socket.edit(|record| {
            record.dtls = 1;
            record.chunk_size = swoole_rs_corebase::constants::SW_SSL_BUFFER_SIZE;
        });
    }
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
pub unsafe fn swr_v1_tls_owner_socket_session(
    owner: *const OwnedTlsContext,
    record: swoole_rs_reactor::Socket<'_>,
    hostname: *const StringSlot,
) -> OwnedTlsSessionResult {
    let socket = record.as_ptr();
    if !record.read(swoole_rs_reactor::SocketFields::ssl).is_null() {
        return OwnedTlsSessionResult {
            status: 0,
            error: 0,
        };
    }
    // SAFETY: the owner stays live through this operation.
    let owner_ref = unsafe { &*owner };
    // SAFETY: exclusive leaf snapshot of the owner configuration.
    let fields = unsafe { *owner_ref.fields.get() };
    let index = crate::swr_v1_tls_connection_index();
    let (status, error) = record.edit(|record| {
        let mut error = 0;
        // SAFETY: the owner's live context; OpenSSL calls nothing back while making the SSL.
        let status = unsafe {
            crate::swr_v1_socket_ssl_new(
                record,
                fields.context,
                0,
                index,
                socket.cast(),
                &mut error,
            )
        };
        (status, error)
    });
    let ssl = crate::Ssl::of(record);
    if status != 0 {
        return OwnedTlsSessionResult { status, error };
    }
    let explicit = owner_ref.strings[8].view();
    let selected = if explicit.length != 0 {
        Some(explicit)
    } else if fields.disable_tls_host_name == 0 {
        // SAFETY: the enclosing socket lends its live automatic hostname slot.
        let automatic = unsafe { &*hostname }.view();
        (automatic.length != 0).then_some(automatic)
    } else {
        None
    };
    if let (Some(name), Some(ssl)) = (selected, ssl) {
        // SAFETY: a slot's view is NUL-terminated, and stays unchanged meanwhile.
        _ = ssl.set_host_name(unsafe { CStr::from_ptr(name.data) });
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

/// `set_cert_file`/`set_key_file`'s check: false, logged from `origin`, when the
/// file can't be read.
fn readable(path: &[u8], origin: &CStr, what: &str) -> bool {
    // access(2) reads up to the first NUL, as upstream's c_str() does.
    let name = path.split(|&b| b == 0).next().unwrap_or_default();
    let terminated = [name, b"\0"].concat();
    let found = CStr::from_bytes_with_nul(&terminated)
        .is_ok_and(|name| swoole_rs_corebase::sys::access(name, libc::R_OK) >= 0);
    if !found {
        let name = String::from_utf8_lossy(name);
        swoole_rs_corebase::log::warning(origin, format_args!("ssl {what} file[{name}] not found"));
    }
    found
}

/// Write the text setting into the owner: false for a certificate or key file
/// that can't be read.
/// # Safety
/// owner is live and exclusive, and `value` is none of its own strings.
pub unsafe fn set_text(owner: *const OwnedTlsContext, which: Text, value: &[u8]) -> bool {
    // SAFETY: the caller lends the live owner.
    let state = unsafe { &*owner };
    // The slot, and for a file its setter's log origin and name.
    let (slot, file) = match which {
        Text::CertFile => (&state.strings[4], Some((c"set_cert_file", "cert"))),
        Text::KeyFile => (&state.strings[5], Some((c"set_key_file", "key"))),
        Text::ClientCertFile => (
            &state.strings[7],
            Some((c"set_client_cert_file", "client cert")),
        ),
        Text::Passphrase => (&state.strings[PASSPHRASE], None),
        Text::HostName => (&state.strings[TLS_HOST_NAME], None),
        Text::Cafile => (&state.strings[9], None),
        Text::Capath => (&state.strings[10], None),
        Text::Ciphers => (&state.strings[0], None),
        Text::EcdhCurve => (&state.strings[1], None),
        Text::DhParam => (&state.strings[3], None),
    };
    if let Some((origin, what)) = file
        && !readable(value, origin, what)
    {
        return false;
    }
    // SAFETY: a stable slot of the exclusive owner, which `value` is not; the stored
    // value keeps every byte.
    let status = unsafe { slot.set_shared(value) };
    // An allocation refusal, which std::bad_alloc ends.
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
#[must_use]
pub fn has_files(owner: &OwnedTlsContext) -> (bool, bool) {
    let set = |slot: &Arc<StringSlot>| !slot.bytes().is_empty();
    (set(&owner.strings[4]), set(&owner.strings[5]))
}

/// `verify_peer` with no client certificate file, cafile or capath.
/// # Safety
/// The caller serializes field writes against this read.
pub unsafe fn verify_without_ca(owner: &OwnedTlsContext) -> bool {
    let empty = |slot: &Arc<StringSlot>| slot.bytes().is_empty();
    // SAFETY: as the caller promises.
    let verify_peer = unsafe { &*owner.fields.get() }.verify_peer != 0;
    verify_peer && empty(&owner.strings[7]) && empty(&owner.strings[9]) && empty(&owner.strings[10])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;
    fn warning(_function: &CStr, _message: &str) {
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
        callback: crate::context::PasswordCallback,
    ) -> bool {
        let actual = unsafe { SSL_CTX_get_default_passwd_cb(ctx) };
        let userdata = unsafe { SSL_CTX_get_default_passwd_cb_userdata(ctx) };
        actual.is_some_and(|actual| std::ptr::fn_addr_eq(actual, callback)) && userdata == data
    }
    #[test]
    fn client_settings_write_the_owner() {
        unsafe {
            let prepared = owner_new(warning);
            let owner = prepared.owner;
            let text = |index: usize| {
                let view = swoole_rs_corebase::swr_v1_core_string_view(&*prepared.strings[index]);
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
            assert_eq!(has_files(&*owner), (false, false));
            let manifest = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
            assert!(set_text(owner, Text::KeyFile, manifest.as_bytes()));
            assert_eq!(has_files(&*owner), (false, true));
            // access(2) reads the path up to its first NUL, and the slot keeps every byte.
            let cut = [manifest.as_bytes(), b"\0tail"].concat();
            assert!(set_text(owner, Text::KeyFile, &cut));
            assert_eq!(text(5), cut);
            let missing = [b"/nonexistent/key.pem\0", manifest.as_bytes()].concat();
            assert!(!set_text(owner, Text::KeyFile, &missing));
            assert_eq!(text(5), cut, "a refused path leaves the slot as it was");
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
                let prepared = owner_new(warning);
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
                let handle = swoole_rs_reactor::Socket::from_ptr(socket).unwrap();
                assert_eq!(
                    swr_v1_tls_owner_socket_context(prepared.owner, handle, true, true),
                    0
                );
                assert_eq!((*socket).fields.output.ssl_send, 1);
                assert_eq!((*prepared.fields).http_v2, 1);
                let result =
                    swr_v1_tls_owner_socket_session(prepared.owner, handle, Arc::as_ptr(&hostname));
                assert_eq!(result.status, 0);
                let ssl = (*socket).fields.ssl().cast::<openssl_sys::SSL>();
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
                    swr_v1_tls_owner_socket_session(ptr::null(), handle, ptr::null()).status,
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
            let source = owner_new(warning);
            let destination = owner_new(warning);
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
                    swoole_rs_corebase::swr_v1_core_string_view(&*source.strings[PASSPHRASE]).data
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
            let prepared = owner_new(warning);
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
