//! Thread-local TLS configuration builders and immutable runtime snapshots.
//! [`TlsContext`] retains a builder; sessions use independent [`TlsSettings`].
use crate::context::create_context;
use crate::{TlsConfig, Warn};
use openssl_sys::SSL_CTX;
use std::{
    cell::Cell,
    ffi::{CStr, c_int},
    rc::Rc,
};
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::StringSlot;

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
    /// Each taken out and put back around plain Rust work, never around a callback;
    /// no pointer to one leaves the owner.
    strings: [Cell<StringSlot>; STRING_COUNT],
    warn: Warn,
}

/// A completed TLS configuration. Its context and strings have no mutable
/// builder aliases; readers can create sessions without borrowing a builder.
pub struct TlsSettings {
    fields: OwnedTlsFields,
    strings: [StringSlot; STRING_COUNT],
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
        crate::socket::socket_ssl_new_over(socket, context, server, Some((bio, None)))
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
/// An owner's pointer and scalar fields; its strings are read and set through the
/// owner.
pub struct OwnedTlsPrepared {
    pub owner: *const OwnedTlsContext,
    pub fields: *mut OwnedTlsFields,
}
/// `SSLContext`'s warnings, `function(): message` at warning level.
pub fn tls_warning(function: &CStr, message: &str) {
    swoole_rs_corebase::log::warning(function, format_args!("{message}"));
}
/// `owner`'s scalar fields, valid while a reference is held. A write through them
/// holds the owner as a [`Lent`] does.
#[must_use]
pub fn owner_view(owner: &OwnedTlsContext) -> OwnedTlsPrepared {
    let mut loan = owner.borrow_configuration();
    OwnedTlsPrepared {
        owner,
        fields: loan.edit(std::ptr::from_mut),
    }
}
/// A new owner that warns through `warn`, with fresh string slots of its own.
fn fresh(warn: Warn) -> Rc<OwnedTlsContext> {
    Rc::new(OwnedTlsContext {
        fields: Cell::new(Some(Box::default())),
        strings: std::array::from_fn(|_| Cell::default()),
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
        self.is_set(CLIENT_CERT_FILE)
    }

    /// `read` over the string slot `index`, none past the last. The slot is taken out
    /// and put back around it, so `read` must not reach this owner again.
    fn with_string<R>(&self, index: usize, read: impl FnOnce(&mut StringSlot) -> R) -> Option<R> {
        let cell = self.strings.get(index)?;
        let mut slot = cell.take();
        let result = read(&mut slot);
        cell.set(slot);
        Some(result)
    }

    /// Whether the string at `index` is set.
    fn is_set(&self, index: usize) -> bool {
        self.with_string(index, |slot| !slot.bytes().is_empty())
            .unwrap_or(false)
    }

    /// A copy of the string at `index`, embedded NULs included; empty past the last.
    #[must_use]
    pub fn string(&self, index: usize) -> Vec<u8> {
        self.with_string(index, |slot| slot.bytes().to_vec())
            .unwrap_or_default()
    }

    /// Copies of the strings, which OpenSSL callbacks can't reach.
    fn copy_strings(&self) -> [StringSlot; STRING_COUNT] {
        std::array::from_fn(|index| {
            let mut copy = StringSlot::new();
            if self.with_string(index, |slot| copy.set(slot.bytes())) != Some(true) {
                swoole_rs_corebase::fatal("out of memory copying TLS settings");
            }
            copy
        })
    }
}
/// A copy of `tls_host_name`, independent of later configuration changes.
#[must_use]
pub fn owner_host_name(owner: &OwnedTlsContext) -> Vec<u8> {
    owner.string(TLS_HOST_NAME)
}
/// A copy of `tls_host_name` up to its first NUL, as C reads it.
#[must_use]
pub fn owner_host_c_str(owner: &OwnedTlsContext) -> std::ffi::CString {
    owner
        .with_string(TLS_HOST_NAME, |slot| slot.c_str().to_owned())
        .unwrap_or_default()
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
        let mut fields = OwnedTlsFields {
            context: None,
            ..*self.fields.as_deref()?
        };
        let strings = self.owner.copy_strings();
        crate::swoole_ssl_init();
        let mut config = configuration(&strings, &fields, self.owner.warn);
        fields.context = Some(new_context(&mut config)?);
        fields.protocols = config.protocols;
        if let Some(callback) = callback {
            fields.context.as_deref()?.set_servername_callback(callback);
        }
        Some(TlsSettings { fields, strings })
    }
}
/// Views of `strings`, and the scalar `fields` copied.
fn configuration<'a>(
    strings: &'a [StringSlot; STRING_COUNT],
    fields: &OwnedTlsFields,
    warn: Warn,
) -> TlsConfig<'a> {
    let strings = strings.each_ref();
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
        warn,
    }
}
/// A new `SSL_CTX` from `config` that reports its info, none when OpenSSL makes none.
fn new_context(config: &mut TlsConfig<'_>) -> Option<crate::SslContext> {
    let ctx = create_context(config);
    if let Some(ctx) = &ctx {
        crate::info::SSL_CTX_set_info_callback(ctx, Some(crate::info::info));
    }
    ctx
}
/// A new `SSL_CTX` from the lent owner's configuration, which replaces the one it
/// held: false when OpenSSL makes none. The owner's logger does not throw.
fn create(owner: &mut Lent<'_>) -> bool {
    // `configuration` reads copies of the strings and fields, so no borrow of the
    // owner is kept across OpenSSL/log callbacks.
    let strings = owner.owner.copy_strings();
    let warn = owner.owner.warn;
    let mut config = owner.edit(|fields| configuration(&strings, fields, warn));
    let ctx = new_context(&mut config);
    let created = ctx.is_some();
    // The callbacks have returned.
    owner.edit(|fields| {
        // The old OpenSSL reference gives way to the new one, released as it goes.
        fields.context = ctx;
        fields.protocols = config.protocols;
    });
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
    for index in 0..STRING_COUNT {
        // The source, a distinct owner, lends its slot while it is copied.
        let copied = source
            .owner
            .with_string(index, |from| destination.set_string(index, from.bytes()));
        if copied != Some(true) {
            return Err(CopyError::String(index));
        }
    }
    // Copy scalar values after string allocation/cache updates have succeeded; the
    // destination takes its own reference to the source's context.
    let Some(fields) = source.edit(|fields| fields.try_clone()) else {
        return Err(CopyError::Fields);
    };
    // Publish the copied scalar values and reference, releasing the destination's
    // previous one.
    destination.edit(|to| *to = fields);
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
            fields.protocols = crate::SslVersion::SW_SSL_DTLS.bits();
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
/// `hostname` is the automatic host name, none when it is empty. A `UringSocket`'s
/// session gives `ring` the `SW_SSL_CLIENT` or `SW_SSL_SERVER` flag: it runs in that
/// state over a pair of memory BIOs, which the ring pumps (failing as `SSL_new` does
/// when OpenSSL can't make them).
pub fn socket_session(
    owner: Option<&TlsSettings>,
    record: swoole_rs_reactor::Socket<'_>,
    hostname: Option<&CStr>,
    ring: Option<c_int>,
) -> Result<(), crate::SslNewError> {
    if !record.read(swoole_rs_reactor::SocketFields::ssl).is_null() {
        return Ok(());
    }
    // No context to make one over: `SSL_new` of none.
    let Some(owner) = owner else {
        return Err(crate::SslNewError::New);
    };
    let fields = &owner.fields;
    let bios = match ring.map(|_| (crate::OwnedBio::memory(), crate::OwnedBio::memory())) {
        None => None,
        Some((Some(read), Some(write))) => Some((read, Some(write))),
        Some(_) => return Err(crate::SslNewError::New),
    };
    let flags = ring.unwrap_or(0);
    crate::socket::socket_ssl_new_over(record, fields.context.as_deref(), flags, bios)?;
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
        if !self.set_string(slot, value) {
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

    /// Stores `value` in the owner's string slot `index`, as `RustStringView`'s
    /// assignment does: false, leaving the slot as it was, when the allocation fails or
    /// past the last.
    pub fn set_string(&mut self, index: usize, value: &[u8]) -> bool {
        self.owner
            .with_string(index, |slot| slot.set(value))
            .unwrap_or(false)
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
    (owner.is_set(4), owner.is_set(5))
}

impl OwnedTlsContext {
    /// No client certificate file, cafile or capath: with `verify_peer`, a peer
    /// verified without a CA.
    #[must_use]
    pub fn names_no_ca(&self) -> bool {
        [CLIENT_CERT_FILE, 9, 10]
            .into_iter()
            .all(|index| !self.is_set(index))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::c_void;
    use std::ptr;
    use std::sync::Arc;

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
        fn SSL_CTX_get_default_passwd_cb(ctx: *mut SSL_CTX) -> *mut c_void;
        fn SSL_CTX_get_default_passwd_cb_userdata(ctx: *mut SSL_CTX) -> *mut c_void;
    }
    #[test]
    fn client_settings_write_the_owner() {
        unsafe {
            let prepared = owner_new(warning);
            let owner = prepared.owner;
            let mut lent = (&*owner).borrow_configuration();
            let text = |index: usize| (*owner).string(index);
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
                let owner = |prepared: &OwnedTlsPrepared| (*prepared.owner).borrow_configuration();
                assert!(owner(&prepared).set_string(TLS_HOST_NAME, explicit.as_bytes()));
                (*prepared.fields).disable_tls_host_name = disabled;
                let mut hostname = StringSlot::new();
                assert!(hostname.set(automatic.as_bytes()));
                let socket =
                    swoole_rs_reactor::SocketRecord::with_fd(stream.as_raw_fd(), 6).into_raw();
                let handle = swoole_rs_reactor::Socket::from_ptr(socket).unwrap();
                let settings =
                    freeze_socket_context(&mut owner(&prepared), handle, true, true).unwrap();
                assert!((*socket).fields.output.ssl_send);
                assert!((*prepared.fields).http_v2);
                // Session creation must use the snapshot even after the builder changes
                // and its last reference is released.
                assert!(owner(&prepared).set_string(TLS_HOST_NAME, b"changed.test"));
                (*prepared.fields).disable_tls_host_name = !disabled;
                drop(Rc::from_raw(prepared.owner));
                let result = socket_session(
                    Some(&settings),
                    handle,
                    (!automatic.is_empty()).then(|| hostname.c_str()),
                    None,
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
                assert_eq!(socket_session(None, handle, None, None), Ok(()));
                // The record frees its SSL.
                drop(Box::from_raw(socket));
            }
        }
    }
    #[test]
    fn copied_ready_context_retains_independent_configuration() {
        unsafe {
            let source = owner_new(warning);
            let destination = owner_new(warning);
            let owner = |prepared: &OwnedTlsPrepared| (*prepared.owner).borrow_configuration();
            assert!(owner(&source).set_string(PASSPHRASE, b"source"));
            assert!(create(&mut owner(&source)));
            let context = (*source.fields).context_ptr();
            assert_eq!(
                owner_assign(&mut owner(&destination), &mut owner(&source)),
                Ok(())
            );
            assert_eq!((*destination.fields).context_ptr(), context);
            assert!(owner(&destination).set_string(PASSPHRASE, b"copy"));
            assert_eq!((*source.owner).string(PASSPHRASE), b"source");
            drop(Rc::from_raw(source.owner));
            assert_eq!(
                openssl_sys::SSL_CTX_get_options(context),
                openssl_sys::SSL_CTX_get_options((*destination.fields).context_ptr())
            );
            drop(Rc::from_raw(destination.owner));
        }
    }
    /// The PEM text `write` puts in a memory BIO.
    unsafe fn pem(write: impl FnOnce(*mut openssl_sys::BIO) -> c_int) -> Vec<u8> {
        unsafe {
            let bio = openssl_sys::BIO_new(openssl_sys::BIO_s_mem());
            assert_eq!(write(bio), 1);
            let mut data = ptr::null_mut();
            let length = usize::try_from(openssl_sys::BIO_get_mem_data(bio, &raw mut data));
            let text = std::slice::from_raw_parts(data.cast::<u8>(), length.unwrap()).to_vec();
            openssl_sys::BIO_free_all(bio);
            text
        }
    }
    #[test]
    fn encrypted_key_loads_with_its_passphrase_and_leaves_no_callback() {
        use openssl_sys::*;
        fn quiet(_function: &CStr, _message: &str) {}
        let dir = std::env::temp_dir();
        let id = std::process::id();
        let cert_file = dir.join(format!("swr-tls-{id}-passphrase-cert.pem"));
        let key_file = dir.join(format!("swr-tls-{id}-passphrase-key.pem"));
        unsafe {
            let keygen = EVP_PKEY_CTX_new_id(EVP_PKEY_ED25519, ptr::null_mut());
            let mut key = ptr::null_mut();
            assert_eq!(EVP_PKEY_keygen_init(keygen), 1);
            assert_eq!(EVP_PKEY_keygen(keygen, &raw mut key), 1);
            EVP_PKEY_CTX_free(keygen);
            let cert = X509_new();
            assert_eq!(X509_set_version(cert, 2), 1);
            assert_eq!(ASN1_INTEGER_set(X509_get_serialNumber(cert), 1), 1);
            assert!(!X509_gmtime_adj(X509_getm_notBefore(cert), 0).is_null());
            assert!(!X509_gmtime_adj(X509_getm_notAfter(cert), 3600).is_null());
            assert_eq!(X509_set_pubkey(cert, key), 1);
            assert!(X509_sign(cert, key, ptr::null()) > 0);
            std::fs::write(&cert_file, pem(|bio| PEM_write_bio_X509(bio, cert))).unwrap();
            // Upstream copies the whole `std::string`, an embedded NUL included.
            let secret = b"a\0b secret";
            let encrypted = pem(|bio| {
                let length = c_int::try_from(secret.len()).unwrap();
                let (cipher, text) = (EVP_aes_128_cbc(), secret.as_ptr().cast());
                PEM_write_bio_PKCS8PrivateKey(bio, key, cipher, text, length, None, ptr::null_mut())
            });
            std::fs::write(&key_file, encrypted).unwrap();
            X509_free(cert);
            EVP_PKEY_free(key);
        }
        let owner = fresh(quiet);
        let mut lent = owner.borrow_configuration();
        assert!(lent.set_text(Text::CertFile, cert_file.to_str().unwrap().as_bytes()));
        assert!(lent.set_text(Text::KeyFile, key_file.to_str().unwrap().as_bytes()));
        for (passphrase, loads) in [(b"a".as_slice(), false), (b"a\0b secret", true)] {
            assert!(lent.set_text(Text::Passphrase, passphrase));
            assert_eq!(create(&mut lent), loads, "{passphrase:?}");
        }
        let ctx = lent.edit(|fields| fields.context_ptr());
        unsafe {
            assert!(SSL_CTX_get_default_passwd_cb(ctx).is_null());
            assert!(SSL_CTX_get_default_passwd_cb_userdata(ctx).is_null());
        }
        _ = std::fs::remove_file(cert_file);
        _ = std::fs::remove_file(key_file);
    }
}
