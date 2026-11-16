//! Thread-local TLS configuration builders and immutable runtime snapshots.
//! [`TlsContext`] retains a builder; sessions use independent [`TlsSettings`].
use crate::context::{PasswordBinding, copy_password, create_context};
use crate::{TlsConfig, Warn};
use openssl_sys::SSL_CTX;
use std::{
    cell::Cell,
    ffi::{CStr, c_int},
    mem::MaybeUninit,
    rc::Rc,
    sync::Arc,
};
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::StringSlot;
use swoole_rs_corebase::lent::{Count, Out};

const STRING_COUNT: usize = 11;
const PASSPHRASE: usize = 6;
#[repr(C)]
#[derive(Default)]
pub struct OwnedTlsFields {
    pub http: bool,
    pub http_v2: bool,
    pub prefer_server_ciphers: bool,
    pub session_tickets: bool,
    pub stapling: bool,
    pub stapling_verify: bool,
    pub disable_tls_host_name: bool,
    pub grease: u8,
    pub verify_depth: u8,
    pub disable_compress: bool,
    pub verify_peer: bool,
    pub allow_self_signed: bool,
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
    fields: Cell<Option<Box<OwnedTlsFields>>>,
    strings: [Arc<StringSlot>; STRING_COUNT],
    /// The passphrase slot the context's password callback reads, kept while it may.
    /// Only a [`Lent`] reaches it.
    context_password: Cell<Option<Arc<StringSlot>>>,
    warn: Warn,
}

/// A completed TLS configuration. Its context and strings have no mutable
/// builder aliases; readers can create sessions without borrowing a builder.
pub struct TlsSettings {
    fields: OwnedTlsFields,
    // Drop the context before these slots, including its password callback's storage.
    strings: [Arc<StringSlot>; STRING_COUNT],
}
#[expect(
    clippy::non_send_fields_in_send_ty,
    reason = "SslContext is normally configurable and not Send; this private instance is \
              configured before publication and exposes only independent session creation"
)]
// SAFETY: construction creates a new SSL_CTX and independent string slots, then
// exposes only session creation and immutable observations. OpenSSL synchronizes
// its context reference counts and session state; configuration is never changed.
unsafe impl Send for TlsSettings {}
// SAFETY: no API lends the context for configuration or mutates these fields or
// strings. Concurrent readers create independent SSL sessions from this SSL_CTX.
unsafe impl Sync for TlsSettings {}

impl TlsSettings {
    #[must_use]
    pub fn protocols(&self) -> u32 {
        self.fields.protocols
    }
    #[must_use]
    pub fn verify_peer(&self) -> bool {
        self.fields.verify_peer
    }
    #[must_use]
    pub fn allow_self_signed(&self) -> bool {
        self.fields.allow_self_signed
    }
    #[must_use]
    pub fn has_client_cert(&self) -> bool {
        !self.strings[7].bytes().is_empty()
    }
    #[must_use]
    pub fn names_no_ca(&self) -> bool {
        let [.., client_cert_file, _, cafile, capath] = &self.strings;
        [client_cert_file, cafile, capath]
            .into_iter()
            .all(|slot| slot.bytes().is_empty())
    }
    /// Creates an independent server session on `socket`.
    #[must_use]
    pub fn create_session(&self, socket: swoole_rs_reactor::Socket<'_>) -> bool {
        crate::upstream::socket_ssl_create(
            socket,
            self.fields.context.as_deref(),
            crate::SslCreateFlag::SW_SSL_SERVER as c_int,
        )
    }
    /// Creates an independent server session on `socket` that reads and writes through
    /// `bio`, which it takes (the bio goes when no session is made).
    #[must_use]
    pub(crate) fn create_dtls_session(
        &self,
        socket: swoole_rs_reactor::Socket<'_>,
        bio: crate::OwnedBio,
    ) -> bool {
        let context = self.fields.context.as_deref();
        let server = crate::SslCreateFlag::SW_SSL_SERVER as c_int;
        crate::socket::socket_ssl_new_over(socket, context, server, Some(bio))
            .inspect_err(|error| error.warn())
            .is_ok()
    }
    /// Creates an independent client session on `socket`.
    #[must_use]
    pub fn create_client_session(&self, socket: swoole_rs_reactor::Socket<'_>) -> bool {
        crate::upstream::socket_ssl_create(
            socket,
            self.fields.context.as_deref(),
            crate::SslCreateFlag::SW_SSL_CLIENT as c_int,
        )
    }
    /// The explicit SNI/verification name, empty when configuration names none.
    #[must_use]
    pub fn host_name(&self) -> &CStr {
        self.strings[TLS_HOST_NAME].c_str()
    }
    /// Selects this context for a server session during its SNI handshake.
    pub fn select(&self, ssl: crate::Ssl<'_>) {
        ssl.set_context(self.fields.context.as_deref());
    }
}
/// An owner's pointer, scalar fields and string slots.
pub struct OwnedTlsPrepared {
    pub owner: *const OwnedTlsContext,
    pub fields: *mut OwnedTlsFields,
    pub strings: [*const StringSlot; STRING_COUNT],
}
/// `SSLContext`'s warnings, `function(): message` at warning level.
pub fn tls_warning(function: &CStr, message: &str) {
    swoole_rs_corebase::log::warning(function, format_args!("{message}"));
}
/// `owner`'s scalar fields and string slots, valid while a reference is held. A write
/// through them holds the owner as a [`Lent`] does.
#[must_use]
pub fn owner_view(owner: &OwnedTlsContext) -> OwnedTlsPrepared {
    let mut loan = owner.borrow_configuration();
    OwnedTlsPrepared {
        owner,
        fields: loan.edit(std::ptr::from_mut),
        strings: owner.strings.each_ref().map(Arc::as_ptr),
    }
}
/// A new owner that warns through `warn`, with fresh string slots of its own.
fn fresh(warn: Warn) -> Rc<OwnedTlsContext> {
    Rc::new(OwnedTlsContext {
        fields: Cell::new(Some(Box::default())),
        strings: std::array::from_fn(|_| StringSlot::empty()),
        context_password: Cell::new(None),
        warn,
    })
}
/// A new owner that warns through `warn`, holding one reference, and its view.
#[must_use]
pub fn owner_new(warn: Warn) -> OwnedTlsPrepared {
    let owner = fresh(warn);
    let view = owner_view(&owner);
    OwnedTlsPrepared {
        // The Rc's own pointer, which `Rc::from_raw` can turn back into the Rc: one
        // taken from a reference covers only the context, not its counts.
        owner: Rc::into_raw(owner),
        ..view
    }
}
/// One counted reference to an owner. Cloning takes another reference and dropping
/// releases this one.
#[derive(Clone)]
pub struct TlsContext(Rc<OwnedTlsContext>);
impl TlsContext {
    /// A new owner that warns through [`tls_warning`].
    #[must_use]
    pub fn new() -> Self {
        Self(fresh(tls_warning))
    }
    /// A new owner holding `texts`, each set as [`Lent::set_text`] sets it.
    #[must_use]
    pub fn with_texts(texts: &[(Text, &[u8])]) -> Self {
        let owner = fresh(tls_warning);
        {
            let mut owner = owner.borrow_configuration();
            for &(which, value) in texts {
                _ = owner.set_text(which, value);
            }
        }
        Self(owner)
    }
    /// A new owner holding a copy of `source`'s configuration, as
    /// [`owner_assign`] copies it, or none when a string can't be copied.
    #[must_use]
    pub fn copy_of(source: &mut Lent<'_>) -> Option<Self> {
        let owner = fresh(tls_warning);
        let copied = owner_assign(&mut owner.borrow_configuration(), source);
        copied.ok().map(|()| Self(owner))
    }
    /// The owner, for native code and the raw-owner API; the handle keeps it live.
    #[must_use]
    pub fn as_ptr(&self) -> *const OwnedTlsContext {
        Rc::as_ptr(&self.0)
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
        &self.0
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
/// `tls_host_name`'s and `client_cert_file`'s slots.
const TLS_HOST_NAME: usize = 8;
const CLIENT_CERT_FILE: usize = 7;
impl OwnedTlsContext {
    /// Lends ownership of the configuration until the returned loan is dropped.
    /// # Panics
    /// The builder already has an active configuration loan.
    #[must_use]
    #[expect(
        clippy::panic,
        reason = "reject overlapping configuration loans before exposing mutable state"
    )]
    pub fn borrow_configuration(&self) -> Lent<'_> {
        let Some(fields) = self.fields.take() else {
            panic!("TLS configuration is already lent");
        };
        Lent {
            owner: self,
            fields: Some(fields),
        }
    }

    /// An owned copy of the settings, independent of subsequent edits.
    /// # Panics
    /// The builder already has an active configuration loan.
    #[must_use]
    pub fn fields(&self) -> OwnedTlsFields {
        self.borrow_configuration()
            .fields
            .as_ref()
            .and_then(|fields| fields.try_clone())
            .unwrap_or_else(|| swoole_rs_corebase::fatal("cannot retain TLS configuration"))
    }

    /// Whether `client_cert_file` is set.
    #[must_use]
    pub fn has_client_cert_file(&self) -> bool {
        !self.strings[CLIENT_CERT_FILE].bytes().is_empty()
    }
}
/// A copy of `tls_host_name`, independent of later configuration changes.
#[must_use]
pub fn owner_host_name(owner: &OwnedTlsContext) -> Vec<u8> {
    owner.strings[TLS_HOST_NAME].bytes().to_vec()
}
/// A copy of `tls_host_name` up to its first NUL, as C reads it.
#[must_use]
pub fn owner_host_c_str(owner: &OwnedTlsContext) -> std::ffi::CString {
    owner.strings[TLS_HOST_NAME].c_str().to_owned()
}
/// An exclusive configuration loan on the builder's owning thread.
pub struct Lent<'a> {
    owner: &'a OwnedTlsContext,
    fields: Option<Box<OwnedTlsFields>>,
}
impl Drop for Lent<'_> {
    fn drop(&mut self) {
        self.owner.fields.set(self.fields.take());
    }
}
impl Lent<'_> {
    /// Builds an independent, immutable runtime configuration. Reconfiguration of
    /// this builder cannot change the context or password a reader already holds.
    #[must_use]
    pub fn freeze(&self, callback: Option<crate::ServernameCallback>) -> Option<TlsSettings> {
        let owner = OwnedTlsContext {
            fields: Cell::new(Some(Box::new(OwnedTlsFields {
                context: None,
                ..*self.fields.as_deref()?
            }))),
            strings: self.owner.strings.each_ref().map(|from| {
                let mut slot = StringSlot::new();
                if !slot.set(from.bytes()) {
                    swoole_rs_corebase::fatal("out of memory freezing TLS settings");
                }
                Arc::new(slot)
            }),
            context_password: Cell::new(None),
            warn: self.owner.warn,
        };
        crate::swoole_ssl_init();
        if !create(&mut owner.borrow_configuration()) {
            return None;
        }
        let fields = owner.fields.into_inner()?;
        if let Some(callback) = callback {
            fields.context.as_deref()?.set_servername_callback(callback);
        }
        Some(TlsSettings {
            fields: *fields,
            strings: owner.strings,
        })
    }

    /// Another reference to the retained password.
    fn password(&mut self) -> Option<Arc<StringSlot>> {
        let held = self.owner.context_password.take();
        let shared = held.clone();
        self.owner.context_password.set(held);
        shared
    }
    /// Retains `password` in place of the one held, which goes.
    fn retain(&mut self, password: Option<Arc<StringSlot>>) {
        drop(self.owner.context_password.replace(password));
    }
}
/// The owner's strings, as views, and the scalar `fields` copied from it.
fn configuration<'a>(owner: &'a OwnedTlsContext, fields: &OwnedTlsFields) -> TlsConfig<'a> {
    let strings = owner.strings.each_ref().map(|slot| &**slot);
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
        http: fields.http,
        http_v2: fields.http_v2,
        prefer_server_ciphers: fields.prefer_server_ciphers,
        disable_compress: fields.disable_compress,
        verify_peer: fields.verify_peer,
        warn: owner.warn,
    }
}
/// The password callback: the passphrase in `slot`, the owner's passphrase slot (the
/// binding's userdata), into the `capacity` bytes OpenSSL lends at `buffer`.
extern "C" fn slot_password<'id>(
    buffer: Out<'id, MaybeUninit<u8>>,
    capacity: Count<'id, c_int>,
    _verify: c_int,
    slot: &StringSlot,
) -> c_int {
    copy_password(buffer.fill(capacity).slots(), slot.bytes_with_nul())
}
/// A new `SSL_CTX` from the lent owner's configuration, which replaces the one it
/// held: false when OpenSSL makes none. The owner's logger does not throw.
fn create(owner: &mut Lent<'_>) -> bool {
    let state = owner.owner;
    // `configuration` copies the fields, so no borrow of them is kept across
    // OpenSSL/log callbacks.
    let mut config = owner.edit(|fields| configuration(state, fields));
    let slot = Arc::as_ptr(&state.strings[PASSPHRASE]);
    let retained_password = if config.passphrase.is_empty() {
        None
    } else {
        Some(Arc::clone(&state.strings[PASSPHRASE]))
    };
    let password = PasswordBinding {
        userdata: slot,
        callback: slot_password,
    };
    // SAFETY: a live password binding: the owner keeps the slot (`retained_password`).
    let ctx = unsafe { create_context(&mut config, Some(password)) };
    if let Some(ctx) = &ctx {
        crate::info::SSL_CTX_set_info_callback(ctx, Some(crate::info::info));
    }
    let created = ctx.is_some();
    // The callbacks have returned.
    owner.edit(|fields| {
        // The old OpenSSL reference gives way to the new one, released as it goes.
        fields.context = ctx;
        fields.protocols = config.protocols;
    });
    // The new context's live password data is retained independently of native views.
    owner.retain(retained_password);
    created
}
/// `bool SSLContext::create()` of the lent owner, which initializes OpenSSL first.
#[must_use]
pub fn ssl_context_create(context: &mut Lent<'_>) -> bool {
    crate::swoole_ssl_init();
    create(context)
}
/// What [`owner_assign`] couldn't copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyError {
    /// The configuration string at this index.
    String(usize),
    /// The scalar values and the context reference.
    Fields,
}
/// `SSLContext`'s assignment: `source`'s configuration values copied into
/// `destination`, with its own reference to the OpenSSL context when ready.
/// Destination slots stay where they are. A string that can't be copied stops the
/// copy; assigning an owner to itself copies nothing.
///
/// # Errors
/// The first part that couldn't be copied.
pub fn owner_assign(destination: &mut Lent<'_>, source: &mut Lent<'_>) -> Result<(), CopyError> {
    if std::ptr::eq(destination.owner, source.owner) {
        return Ok(());
    }
    for (index, from) in source.owner.strings.iter().enumerate() {
        // The source's bytes, a distinct owner's, stay unchanged while they are copied.
        if !destination.store(index, from.bytes()) {
            return Err(CopyError::String(index));
        }
    }
    // Copy scalar values after string allocation/cache updates have succeeded; the
    // destination takes its own reference to the source's context.
    let Some(fields) = source.edit(|fields| fields.try_clone()) else {
        return Err(CopyError::Fields);
    };
    // Independent retention of the context's actual live password value.
    let password = source.password();
    // Publish the copied scalar values and reference, releasing the destination's
    // previous one.
    destination.edit(|to| *to = fields);
    // Release old password ownership only after releasing its OpenSSL reference.
    destination.retain(password);
    Ok(())
}

/// Why a coroutine socket got no context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketContextError {
    /// A datagram socket without DTLS support.
    UnsupportedDtls,
    /// Context creation failed.
    Create,
}

/// Configure and freeze a coroutine socket's independent runtime context.
/// # Errors
/// [`SocketContextError`].
pub fn freeze_socket_context(
    owner: &mut Lent<'_>,
    socket: swoole_rs_reactor::Socket<'_>,
    http2: bool,
    dtls_supported: bool,
) -> Result<TlsSettings, SocketContextError> {
    let kind = socket.read(swoole_rs_reactor::SocketFields::kind);
    let dtls = kind.is_some_and(swoole_rs_corebase::constants::SocketType::is_dgram);
    if dtls && !dtls_supported {
        return Err(SocketContextError::UnsupportedDtls);
    }
    // Leaf updates, before OpenSSL callbacks may run.
    owner.edit(|fields| {
        if dtls {
            fields.protocols = crate::SslVersion::SW_SSL_DTLS as u32;
        }
        fields.http_v2 = http2;
    });
    if dtls {
        socket.edit(|record| {
            record.dtls = true;
            record.chunk_size = swoole_rs_corebase::constants::SW_SSL_BUFFER_SIZE;
        });
    }
    let settings = owner.freeze(None).ok_or(SocketContextError::Create)?;
    socket.edit(|record| record.output.ssl_send = true);
    Ok(settings)
}

/// Create and configure one coroutine TLS session, including moving write buffers
/// and the context/automatic hostname precedence.
///
/// # Errors
/// As [`crate::socket_ssl_new`].
///
/// `hostname` is the automatic host name, none when it is empty.
pub fn socket_session(
    owner: Option<&TlsSettings>,
    record: swoole_rs_reactor::Socket<'_>,
    hostname: Option<&CStr>,
) -> Result<(), crate::SslNewError> {
    if !record.read(swoole_rs_reactor::SocketFields::ssl).is_null() {
        return Ok(());
    }
    // No context to make one over: `SSL_new` of none.
    let Some(owner) = owner else {
        return Err(crate::SslNewError::New);
    };
    let fields = &owner.fields;
    crate::socket_ssl_new(record, fields.context.as_deref(), 0)?;
    let disable_tls_host_name = fields.disable_tls_host_name;
    let ssl = crate::Ssl::of(record);
    let explicit = &owner.strings[TLS_HOST_NAME];
    let selected = if !explicit.bytes().is_empty() {
        Some(explicit.c_str())
    } else if !disable_tls_host_name {
        hostname
    } else {
        None
    };
    if let (Some(name), Some(ssl)) = (selected, ssl) {
        _ = ssl.set_host_name(name);
    }
    Ok(())
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
        .is_ok_and(|name| swoole_rs_corebase::sys::access(name, libc::R_OK).is_ok());
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
    pub fn set_text(&mut self, which: Text, value: &[u8]) -> bool {
        // The slot, and for a file its setter's log origin and name.
        let (slot, file) = match which {
            Text::CertFile => (4, Some((c"set_cert_file", "cert"))),
            Text::KeyFile => (5, Some((c"set_key_file", "key"))),
            Text::ClientCertFile => (
                CLIENT_CERT_FILE,
                Some((c"set_client_cert_file", "client cert")),
            ),
            Text::Passphrase => (PASSPHRASE, None),
            Text::HostName => (TLS_HOST_NAME, None),
            Text::Cafile => (9, None),
            Text::Capath => (10, None),
            Text::Ciphers => (0, None),
            Text::EcdhCurve => (1, None),
            Text::DhParam => (3, None),
        };
        if let Some((origin, what)) = file
            && !readable(value, origin, what)
        {
            return false;
        }
        // The stored value keeps every byte. An allocation refusal, which
        // std::bad_alloc ends.
        if !self.store(slot, value) {
            swoole_rs_corebase::fatal("out of memory storing a TLS setting");
        }
        if matches!(which, Text::HostName) {
            self.edit(|fields| fields.disable_tls_host_name = value.is_empty());
        }
        true
    }

    /// Write the scalar setting into the owner.
    pub fn set_value(&mut self, value: Value) {
        self.edit(|fields| match value {
            Value::Protocols(value) => fields.protocols = value.c_cast::<u32>(),
            Value::DisableCompress(value) => fields.disable_compress = value,
            Value::VerifyPeer(value) => fields.verify_peer = value,
            Value::AllowSelfSigned(value) => fields.allow_self_signed = value,
            Value::VerifyDepth(value) => fields.verify_depth = value,
            Value::Grease(value) => fields.grease = value,
            Value::PreferServerCiphers(value) => fields.prefer_server_ciphers = value,
        });
    }

    /// Stores `value` in the owner's string slot `index`: false, leaving the slot as it
    /// was, when the allocation fails.
    fn store(&self, index: usize, value: &[u8]) -> bool {
        // SAFETY: this loan holds the builder's checked exclusive configuration borrow
        // on its owning thread, and no safe API lends the owner's string slots out, so
        // nothing else reads or sets the slot and `value` is not its bytes.
        let store = |slot: &Arc<StringSlot>| unsafe { slot.set_shared(value) };
        self.owner.strings.get(index).is_some_and(store)
    }

    /// Edits the configuration owned by this loan.
    pub fn edit<R>(&mut self, f: impl FnOnce(&mut OwnedTlsFields) -> R) -> R {
        f(self
            .fields
            .as_deref_mut()
            .unwrap_or_else(|| swoole_rs_corebase::fatal("TLS loan was returned")))
    }
}

/// Whether the owner has a certificate file and a key file.
#[must_use]
pub fn has_files(owner: &OwnedTlsContext) -> (bool, bool) {
    let set = |slot: &Arc<StringSlot>| !slot.bytes().is_empty();
    (set(&owner.strings[4]), set(&owner.strings[5]))
}

impl OwnedTlsContext {
    /// No client certificate file, cafile or capath: with `verify_peer`, a peer
    /// verified without a CA.
    #[must_use]
    pub fn names_no_ca(&self) -> bool {
        let [.., client_cert_file, _, cafile, capath] = &self.strings;
        [client_cert_file, cafile, capath]
            .iter()
            .all(|slot| slot.bytes().is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::{c_char, c_void};
    use std::ptr;

    /// The password callback as OpenSSL calls it: `capacity` (any value C may pass, at
    /// most `bytes`' length) at `bytes`.
    fn call_password(bytes: &mut [u8], capacity: c_int, slot: &StringSlot) -> c_int {
        let slots = unsafe { &mut *(ptr::from_mut(bytes) as *mut [MaybeUninit<u8>]) };
        swoole_rs_corebase::lent::lend_out(slots, |at, _: Count<'_, c_int>| {
            let capacity = unsafe { core::mem::transmute::<c_int, Count<'_, c_int>>(capacity) };
            slot_password(at, capacity, 0, slot)
        })
    }

    #[test]
    fn configuration_loans_reject_aliases_and_hostname_copies_survive_changes() {
        use std::panic::{AssertUnwindSafe, catch_unwind};
        let builder = TlsContext::new();
        let alias = builder.clone();
        let mut loan = builder.borrow_configuration();
        assert!(catch_unwind(AssertUnwindSafe(|| alias.borrow_configuration())).is_err());
        assert!(loan.set_text(Text::HostName, b"first.test"));
        let name = owner_host_c_str(&builder);
        assert!(loan.set_text(Text::HostName, b"second.test"));
        assert_eq!(name, c"first.test");
        assert!(catch_unwind(AssertUnwindSafe(|| builder.fields())).is_err());
        drop(loan);
        let fields = builder.fields();
        alias
            .borrow_configuration()
            .set_value(Value::VerifyPeer(true));
        assert!(builder.fields().verify_peer);
        assert!(
            !fields.verify_peer,
            "an owned read remains independent of later edits"
        );
        assert_eq!(owner_host_name(&builder), b"second.test");
    }

    #[test]
    fn frozen_settings_survive_builder_changes_and_thread_handoffs() {
        fn shared<T: Send + Sync>() {}
        shared::<TlsSettings>();
        let builder = TlsContext::new();
        let mut owner = builder.borrow_configuration();
        assert!(owner.set_text(Text::Passphrase, b"first password"));
        owner.set_value(Value::AllowSelfSigned(true));
        let first = Arc::new(owner.freeze(None).unwrap());
        assert!(owner.set_text(Text::Passphrase, b"replacement password"));
        owner.set_value(Value::AllowSelfSigned(false));
        let second = owner.freeze(None).unwrap();
        assert_ne!(first.fields.context_ptr(), second.fields.context_ptr());
        assert!(!second.allow_self_signed());
        drop(owner);
        drop(builder);
        std::thread::scope(|scope| {
            for _ in 0..4 {
                let first = Arc::clone(&first);
                scope.spawn(move || {
                    assert!(first.allow_self_signed());
                    assert_eq!(first.strings[PASSPHRASE].bytes(), b"first password");
                    assert!(first.fields.context.is_some());
                    for _ in 0..8 {
                        let session = crate::handle::SSL_new(first.fields.context.as_deref());
                        drop(session.expect("independent session from a shared frozen context"));
                    }
                });
            }
        });
    }
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
            let mut lent = (&*owner).borrow_configuration();
            let text = |index: usize| {
                let view = swoole_rs_corebase::swr_v1_core_string_view(&*prepared.strings[index]);
                std::slice::from_raw_parts(view.data.cast::<u8>(), view.length).to_vec()
            };
            assert!(lent.set_text(Text::HostName, b"example.test"));
            assert_eq!(text(8), b"example.test");
            assert!(!(*prepared.fields).disable_tls_host_name);
            assert!(lent.set_text(Text::HostName, b""));
            assert!((*prepared.fields).disable_tls_host_name);
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
            assert!((*prepared.fields).verify_peer);
            drop(lent);
            drop(Rc::from_raw(owner));
        }
    }
    #[test]
    fn socket_session_hostname_precedence_and_record_identity() {
        use std::os::fd::AsRawFd;
        unsafe {
            crate::swr_v1_tls_init().unwrap();
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
                assert!((*prepared.strings[8]).set_shared(explicit.as_bytes()));
                (*prepared.fields).disable_tls_host_name = disabled;
                let hostname = StringSlot::empty();
                assert!(hostname.set_shared(automatic.as_bytes()));
                let socket =
                    swoole_rs_reactor::SocketRecord::with_fd(stream.as_raw_fd(), 6).into_raw();
                let handle = swoole_rs_reactor::Socket::from_ptr(socket).unwrap();
                let settings = freeze_socket_context(
                    &mut (&*prepared.owner).borrow_configuration(),
                    handle,
                    true,
                    true,
                )
                .unwrap();
                assert!((*socket).fields.output.ssl_send);
                assert!((*prepared.fields).http_v2);
                // Session creation must use the snapshot even after the builder changes
                // and its last reference is released.
                assert!((*prepared.strings[8]).set_shared(b"changed.test"));
                (*prepared.fields).disable_tls_host_name = !disabled;
                drop(Rc::from_raw(prepared.owner));
                let result = socket_session(
                    Some(&settings),
                    handle,
                    (!automatic.is_empty()).then(|| hostname.c_str()),
                );
                assert_eq!(result, Ok(()));
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
                assert_eq!(socket_session(None, handle, None), Ok(()));
                openssl_sys::SSL_free(ssl);
                drop(Box::from_raw(socket));
            }
        }
    }
    #[test]
    fn copied_ready_context_retains_independent_configuration_and_live_password() {
        unsafe {
            let source = owner_new(warning);
            let destination = owner_new(warning);
            assert!((*source.strings[PASSPHRASE]).set_shared(b"source"));
            assert!(create(&mut (&*source.owner).borrow_configuration()));
            let context = (*source.fields).context_ptr();
            assert_eq!(
                owner_assign(
                    &mut (&*destination.owner).borrow_configuration(),
                    &mut (&*source.owner).borrow_configuration()
                ),
                Ok(())
            );
            assert_eq!((*destination.fields).context_ptr(), context);
            assert!((*destination.strings[PASSPHRASE]).set_shared(b"copy"));
            assert_eq!(
                std::ffi::CStr::from_ptr(
                    swoole_rs_corebase::swr_v1_core_string_view(&*source.strings[PASSPHRASE]).data
                )
                .to_bytes(),
                b"source"
            );
            drop(Rc::from_raw(source.owner));
            let password = (*destination.owner).context_password.as_ptr();
            let slot = Arc::as_ptr((&*password).as_ref().unwrap());
            let mut output = [0u8; 16];
            assert_eq!(call_password(&mut output, 16, &*slot), 6);
            assert_eq!(&output[..7], b"source\0");
            assert_eq!(
                openssl_sys::SSL_CTX_get_options(context),
                openssl_sys::SSL_CTX_get_options((*destination.fields).context_ptr())
            );
            drop(Rc::from_raw(destination.owner));
        }
    }
    #[test]
    fn shared_context_reads_the_live_passphrase_after_the_first_reference_goes() {
        unsafe {
            let prepared = owner_new(warning);
            let owner = prepared.owner;
            let slot = &*prepared.strings[PASSPHRASE];
            assert!((*prepared.strings[PASSPHRASE]).set_shared(b"initial"));
            assert!(create(&mut (&*owner).borrow_configuration()));
            let ctx = (*prepared.fields).context_ptr();
            assert_eq!((*prepared.fields).protocols, 126);
            let data = ptr::from_ref(slot).cast_mut().cast();
            assert!(password_binding_matches(ctx, data, slot_password));
            for value in [b"swoole".as_slice(), b"a\0b", b"", b"1234567"] {
                assert!((*prepared.strings[PASSPHRASE]).set_shared(value));
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
                        call_password(&mut bytes, capacity, slot),
                        c_int::try_from(length).unwrap()
                    );
                    assert_eq!(bytes, expected);
                }
            }
            // Another ownership handle, as a `TlsContext` clone takes one.
            Rc::increment_strong_count(owner);
            let retained = owner;
            drop(Rc::from_raw(owner));
            assert!((*prepared.strings[PASSPHRASE]).set_shared(b"retained"));
            let mut bytes = [0u8; 16];
            assert_eq!(call_password(&mut bytes, 16, slot), 8);
            assert_eq!(&bytes[..9], b"retained\0");
            drop(Rc::from_raw(retained));
        }
    }
}
