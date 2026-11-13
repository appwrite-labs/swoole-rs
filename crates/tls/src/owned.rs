//! Shared SSL context/configuration ownership: `swoole::SSLContext` is this owner,
//! reference counted (`swr_v1_tls_owner_new`, `_clone`, `_free`). Native reads its
//! scalar fields and string slots through `swr_v1_tls_owner_view`; OpenSSL
//! operations read the authoritative Rust owner.
use crate::context::{PasswordBinding, copy_password, create_context};
use crate::{TlsConfig, Warn};
use openssl_sys::SSL_CTX;
use std::{
    cell::{Cell, UnsafeCell},
    ffi::{CStr, c_char, c_int, c_void},
    marker::PhantomData,
    sync::Arc,
};
use swoole_rs_corebase::StringSlot;

const STRING_COUNT: usize = 11;
const PASSPHRASE: usize = 6;
#[repr(C)]
#[derive(Default)]
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
    /// The owner's reference to its `SSL_CTX`, released with the fields; native
    /// reads it as a pointer, null without one.
    pub context: Option<crate::SslContext>,
}
impl OwnedTlsFields {
    /// The context, or null without one.
    #[must_use]
    pub fn context_ptr(&self) -> *mut SSL_CTX {
        self.context
            .as_ref()
            .map_or(std::ptr::null_mut(), |context| context.as_ptr())
    }
    /// A copy that takes its own reference to the context, None when OpenSSL fails to.
    fn try_clone(&self) -> Option<Self> {
        let context = match &self.context {
            Some(context) => Some(context.try_clone()?),
            None => None,
        };
        Some(Self { context, ..*self })
    }
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
    /// A new owner holding `texts`, each set as [`Lent::set_text`] sets it.
    #[must_use]
    pub fn with_texts(texts: &[(Text, &[u8])]) -> Self {
        let context = Self::new();
        for &(which, value) in texts {
            // SAFETY: the fresh owner, which nothing else holds yet.
            _ = unsafe { Lent::new(context.as_ptr()) }.set_text(which, value);
        }
        context
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
/// The owner's fields, borrowed.
/// # Safety
/// owner is live for `'a` and no field write runs while the borrow is used.
#[must_use]
pub unsafe fn owner_fields<'a>(owner: *const OwnedTlsContext) -> &'a OwnedTlsFields {
    // SAFETY: as the caller promises.
    let owner = unsafe { &*owner };
    // SAFETY: as the caller promises.
    unsafe { &*owner.fields.get() }
}
/// `tls_host_name`'s NUL-terminated bytes, read while the name stays unchanged.
#[must_use]
pub fn owner_tls_host_name(owner: &OwnedTlsContext) -> swoole_rs_corebase::CoreStringView {
    owner.strings[TLS_HOST_NAME].view()
}
/// `tls_host_name`'s bytes, borrowed from the owner. Whoever sets the name holds the
/// owner exclusively ([`set_text`]'s contract, and the native setters'), so no write
/// overlaps a borrow safe code holds.
#[must_use]
pub fn owner_host_name(owner: &OwnedTlsContext) -> &[u8] {
    owner.strings[TLS_HOST_NAME].bytes()
}
/// An owner whose caller serializes it: the borrow every operation here takes once,
/// instead of each field access promising it again. It is not `Sync`, so only the
/// thread holding it writes through it.
pub struct Lent<'a>(&'a OwnedTlsContext, PhantomData<Cell<()>>);
impl Lent<'_> {
    /// # Safety
    /// `owner` is live while the guard is, and until the guard goes only its holder
    /// reaches the owner's fields and retained password: no other thread does, and
    /// what the holder calls meanwhile (OpenSSL, the logger, native code reading its
    /// view) runs while no borrow the guard lent is in use.
    pub unsafe fn new(owner: *const OwnedTlsContext) -> Self {
        // SAFETY: as the caller promises.
        Self(unsafe { &*owner }, PhantomData)
    }
    fn fields(&self) -> &OwnedTlsFields {
        // SAFETY: the guard's contract: nothing writes the fields while it lends them.
        unsafe { &*self.0.fields.get() }
    }
    fn fields_mut(&mut self) -> &mut OwnedTlsFields {
        // SAFETY: the guard's contract: only its holder reaches the fields, and this
        // borrow of the guard is its only one.
        unsafe { &mut *self.0.fields.get() }
    }
    fn password(&mut self) -> &mut Option<Arc<StringSlot>> {
        // SAFETY: as `fields_mut`, for the retained password.
        unsafe { &mut *self.0.context_password.get() }
    }
}
/// The owner's strings, as views, and the scalar `fields` copied from it.
fn configuration(owner: &OwnedTlsContext, fields: &OwnedTlsFields) -> TlsConfig {
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
    let capacity = usize::try_from(capacity).unwrap_or(0);
    // SAFETY: per the caller: `capacity` writable bytes, lent for this call.
    let buffer = unsafe { crate::upstream::slots(buffer.cast(), capacity) };
    copy_password(buffer, passphrase)
}
/// A new `SSL_CTX` from the lent owner's configuration, which replaces the one it
/// held: false when OpenSSL makes none. The owner's logger does not throw.
fn create(owner: &mut Lent<'_>) -> bool {
    let state = owner.0;
    // `configuration` copies the fields, so no borrow of them is kept across
    // OpenSSL/log callbacks.
    let mut config = configuration(state, owner.fields());
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
    if let Some(ctx) = &ctx {
        crate::info::SSL_CTX_set_info_callback(ctx, Some(crate::info::info));
    }
    let created = ctx.is_some();
    // The callbacks have returned.
    let fields = owner.fields_mut();
    // The old OpenSSL reference gives way to the new one, released as it goes.
    fields.context = ctx;
    fields.protocols = config.protocols;
    // The new context's live password data is retained independently of native views.
    *owner.password() = retained_password;
    created
}
/// `bool SSLContext::create()`, which initializes OpenSSL first.
///
/// # Safety
/// `context` is live/exclusive, and its logger does not throw.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole10SSLContext6createEv")
)]
#[must_use]
pub unsafe extern "C" fn ssl_context_create(context: *const OwnedTlsContext) -> bool {
    crate::swoole_ssl_init();
    // SAFETY: as the caller promises.
    create(&mut unsafe { Lent::new(context) })
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
    // SAFETY: a live owner its caller serializes, distinct from the source.
    let mut destination = unsafe { Lent::new(destination) };
    // SAFETY: as above; the source remains unchanged for the configuration copy.
    let mut source = unsafe { Lent::new(source) };
    let slots = source.0.strings.iter().zip(&destination.0.strings);
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
    // Copy scalar values after string allocation/cache updates have succeeded; the
    // destination takes its own reference to the source's context.
    let Some(fields) = source.fields().try_clone() else {
        return OwnedTlsCopy {
            status: -2,
            index: 0,
        };
    };
    // Independent retention of the context's actual live password value.
    let password = source.password().clone();
    // Publish the copied scalar values and reference, releasing the destination's
    // previous one.
    *destination.fields_mut() = fields;
    // Release old password ownership only after releasing its OpenSSL reference.
    *destination.password() = password;
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
    // SAFETY: as the caller promises.
    let mut owner = unsafe { Lent::new(owner) };
    let kind = socket.read(|record| record.socket_type);
    let dtls = swoole_rs_reactor::type_is(kind, swoole_rs_reactor::CLASS_DGRAM);
    if dtls && !dtls_supported {
        return 1;
    }
    {
        // Leaf updates, before OpenSSL callbacks may run.
        let fields = owner.fields_mut();
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
    if !create(&mut owner) {
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
    // SAFETY: as the caller promises.
    let owner = unsafe { Lent::new(owner) };
    let owner_ref = owner.0;
    let (context, disable_tls_host_name) = {
        // The context is borrowed until the SSL is made; nothing writes the fields
        // meanwhile.
        let fields = owner.fields();
        (fields.context.as_deref(), fields.disable_tls_host_name)
    };
    let index = crate::swr_v1_tls_connection_index();
    let (status, error) = record.edit(|record| {
        let mut error = 0;
        // SAFETY: the record's own socket as the SSL's ex data; OpenSSL calls nothing
        // back while making the SSL.
        let status = unsafe {
            crate::swr_v1_socket_ssl_new(record, context, 0, index, socket.cast(), &mut error)
        };
        (status, error)
    });
    let ssl = crate::Ssl::of(record);
    if status != 0 {
        return OwnedTlsSessionResult { status, error };
    }
    let explicit = &owner_ref.strings[TLS_HOST_NAME];
    let selected = if !explicit.bytes().is_empty() {
        Some(explicit.c_str())
    } else if disable_tls_host_name == 0 {
        // SAFETY: the enclosing socket lends its live automatic hostname slot.
        let automatic = unsafe { &*hostname };
        (!automatic.bytes().is_empty()).then(|| automatic.c_str())
    } else {
        None
    };
    if let (Some(name), Some(ssl)) = (selected, ssl) {
        _ = ssl.set_host_name(name);
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

impl Lent<'_> {
    /// Write the text setting into the owner: false for a certificate or key file
    /// that can't be read.
    #[must_use]
    pub fn set_text(&self, which: Text, value: &[u8]) -> bool {
        let state = self.0;
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
        // SAFETY: a stable slot of the exclusive owner. `value` borrows none of its
        // strings: they are private to this crate, which lends no borrow of them out.
        // The stored value keeps every byte.
        let status = unsafe { slot.set_shared(value) };
        // An allocation refusal, which std::bad_alloc ends.
        if status != 0 {
            std::process::abort();
        }
        if matches!(which, Text::HostName) {
            self.edit(|fields| fields.disable_tls_host_name = value.is_empty().into());
        }
        true
    }

    /// Write the scalar setting into the owner.
    pub fn set_value(&self, value: Value) {
        self.edit(|fields| match value {
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
        });
    }

    /// `f` over the owner's fields, which takes no other borrow of them.
    fn edit(&self, f: impl FnOnce(&mut OwnedTlsFields)) {
        // SAFETY: the guard's contract, on its one thread: only its holder reaches the
        // fields, and every borrow of them the guard lends ends inside a method of its
        // own, none of which is running `f`.
        f(unsafe { &mut *self.0.fields.get() });
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
            let lent = Lent::new(owner);
            let text = |index: usize| {
                let view = swoole_rs_corebase::swr_v1_core_string_view(&*prepared.strings[index]);
                std::slice::from_raw_parts(view.data.cast::<u8>(), view.length).to_vec()
            };
            assert!(lent.set_text(Text::HostName, b"example.test"));
            assert_eq!(text(8), b"example.test");
            assert_eq!((*prepared.fields).disable_tls_host_name, 0);
            assert!(lent.set_text(Text::HostName, b""));
            assert_eq!((*prepared.fields).disable_tls_host_name, 1);
            assert!(lent.set_text(Text::Ciphers, b"HIGH"));
            assert_eq!(text(0), b"HIGH");
            assert!(!lent.set_text(Text::CertFile, b"/nonexistent/cert.pem"));
            assert_eq!(has_files(&*owner), (false, false));
            let manifest = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
            assert!(lent.set_text(Text::KeyFile, manifest.as_bytes()));
            assert_eq!(has_files(&*owner), (false, true));
            // access(2) reads the path up to its first NUL, and the slot keeps every byte.
            let cut = [manifest.as_bytes(), b"\0tail"].concat();
            assert!(lent.set_text(Text::KeyFile, &cut));
            assert_eq!(text(5), cut);
            let missing = [b"/nonexistent/key.pem\0", manifest.as_bytes()].concat();
            assert!(!lent.set_text(Text::KeyFile, &missing));
            assert_eq!(text(5), cut, "a refused path leaves the slot as it was");
            lent.set_value(Value::Protocols(1 << 3));
            lent.set_value(Value::VerifyDepth(7));
            lent.set_value(Value::VerifyPeer(true));
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
            assert!(create(&mut Lent::new(source.owner)));
            let context = (*source.fields).context_ptr();
            assert_eq!(
                swr_v1_tls_owner_assign(destination.owner, source.owner).status,
                0
            );
            assert_eq!((*destination.fields).context_ptr(), context);
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
                openssl_sys::SSL_CTX_get_options((*destination.fields).context_ptr())
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
            assert!(create(&mut Lent::new(owner)));
            let ctx = (*prepared.fields).context_ptr();
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
