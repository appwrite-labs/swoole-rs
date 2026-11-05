//! Shared SSL context/configuration ownership. Native exposes borrowed scalar
//! slots and string views; OpenSSL operations read the authoritative Rust owner.
use crate::context::{PasswordBinding, copy_password, create_context};
use crate::{TlsConfig, TlsStr, Warn};
use openssl_sys::{SSL_CTX, SSL_CTX_free};
use std::{
    cell::UnsafeCell,
    ffi::{c_char, c_int, c_void},
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
    native_identity: UnsafeCell<*mut c_void>,
    bound_password_context: UnsafeCell<*mut SSL_CTX>,
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
#[repr(C)]
pub struct OwnedTlsLayout {
    pub size: usize,
    pub alignment: usize,
    pub offsets: [usize; 15],
}
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_tls_owner_layout() -> OwnedTlsLayout {
    OwnedTlsLayout {
        size: size_of::<OwnedTlsFields>(),
        alignment: align_of::<OwnedTlsFields>(),
        offsets: [
            std::mem::offset_of!(OwnedTlsFields, http),
            std::mem::offset_of!(OwnedTlsFields, http_v2),
            std::mem::offset_of!(OwnedTlsFields, prefer_server_ciphers),
            std::mem::offset_of!(OwnedTlsFields, session_tickets),
            std::mem::offset_of!(OwnedTlsFields, stapling),
            std::mem::offset_of!(OwnedTlsFields, stapling_verify),
            std::mem::offset_of!(OwnedTlsFields, disable_tls_host_name),
            std::mem::offset_of!(OwnedTlsFields, grease),
            std::mem::offset_of!(OwnedTlsFields, verify_depth),
            std::mem::offset_of!(OwnedTlsFields, disable_compress),
            std::mem::offset_of!(OwnedTlsFields, verify_peer),
            std::mem::offset_of!(OwnedTlsFields, allow_self_signed),
            std::mem::offset_of!(OwnedTlsFields, protocols),
            std::mem::offset_of!(OwnedTlsFields, create_flag),
            std::mem::offset_of!(OwnedTlsFields, context),
        ],
    }
}
/// warn is a nonthrowing native logging leaf that outlives the shared owner.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_tls_owner_new(warn: Warn) -> OwnedTlsPrepared {
    let owner = Arc::new(OwnedTlsContext {
        fields: UnsafeCell::new(OwnedTlsFields::default()),
        strings: std::array::from_fn(|_| StringSlot::empty()),
        native_identity: UnsafeCell::new(ptr::null_mut()),
        bound_password_context: UnsafeCell::new(ptr::null_mut()),
        context_password: UnsafeCell::new(None),
        warn,
    });
    OwnedTlsPrepared {
        fields: owner.fields.get(),
        strings: owner.strings.each_ref().map(Arc::as_ptr),
        owner: Arc::into_raw(owner),
    }
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
/// Bind the native compatibility identity used by OpenSSL's public userdata API.
/// # Safety
/// owner is live/exclusive. identity's first pointer is this owner; the native
/// constructor verifies that layout before exposing callbacks, and detach precedes destruction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_tls_owner_bind(
    owner: *const OwnedTlsContext,
    identity: *mut c_void,
) {
    // SAFETY: exclusive compatibility-view registration.
    let owner = unsafe { &*owner };
    // SAFETY: caller serializes registration against every configuration operation.
    unsafe { *owner.native_identity.get() = identity };
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
extern "C" fn native_password(
    buffer: *mut c_char,
    capacity: c_int,
    _verify: c_int,
    identity: *mut c_void,
) -> c_int {
    // SAFETY: the native constructor verifies its first pointer is the owner;
    // detach replaces this callback before that compatibility identity disappears.
    let owner = unsafe { identity.cast::<*const OwnedTlsContext>().read() };
    let view = passphrase_view(owner.cast());
    // SAFETY: OpenSSL lends the output buffer; the live slot lends its exact bytes.
    unsafe { copy_password(buffer, capacity, view) }
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
/// Native view teardown keeps cloned Rust handles valid and switches userdata
/// to the Rust slot, so no callback retains an address in the destroyed view.
/// # Safety
/// owner is live/exclusive; its native view is being destroyed with no callback active.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_tls_owner_detach(owner: *const OwnedTlsContext) {
    // SAFETY: the live owner remains owned during compatibility-view destruction.
    let owner = unsafe { &*owner };
    // SAFETY: the compatibility identity remains valid through binding teardown.
    unsafe { detach_bound_password(owner) };
    // SAFETY: no callback is active while its native view detaches.
    unsafe { *owner.native_identity.get() = ptr::null_mut() };
}
/// # Safety
/// owner is live/exclusive, including its current context and password lease.
unsafe fn detach_bound_password(owner: &OwnedTlsContext) {
    // SAFETY: exclusive registration observation.
    let identity = unsafe { *owner.native_identity.get() };
    // SAFETY: observe which context this owner bound to the native identity.
    let bound = unsafe { *owner.bound_password_context.get() };
    // SAFETY: a synchronized field read; the owner retains the current SSL_CTX.
    let ctx = unsafe { (*owner.fields.get()).context };
    if !ctx.is_null() && ctx == bound && !identity.is_null()
        // SAFETY: the live context may be inspected; preserve custom bindings.
        && unsafe { crate::context::password_binding_matches(ctx, identity, native_password) }
    {
        // SAFETY: copied contexts retain this same password lease independently.
        let password = unsafe { &*owner.context_password.get() };
        if let Some(password) = password {
            let slot = Arc::as_ptr(password).cast_mut().cast();
            // SAFETY: every context owner keeps this stable slot alive after rebinding.
            unsafe { crate::context::bind_password(ctx, slot, slot_password) };
        }
    }
    // SAFETY: replacing/detaching this native binding ends this owner's tracking.
    unsafe { *owner.bound_password_context.get() = ptr::null_mut() };
}
/// # Safety
/// owner and native cache views are live/exclusive, and its logger does not throw.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_tls_owner_create(owner: *const OwnedTlsContext) -> bool {
    // SAFETY: snapshot raw views without retaining a record reference across OpenSSL/log callbacks.
    let mut config = unsafe { configuration(owner) };
    // SAFETY: the live Arc owns interior-mutable slots; no exclusive borrow survives callbacks.
    let state = unsafe { &*owner };
    // SAFETY: short synchronized observation of the identity slot.
    let identity = unsafe { *state.native_identity.get() };
    let slot = Arc::as_ptr(&state.strings[PASSPHRASE]);
    let retained_password = if config.passphrase.length == 0 {
        None
    } else {
        Some(Arc::clone(&state.strings[PASSPHRASE]))
    };
    let password = if identity.is_null() {
        PasswordBinding {
            userdata: slot.cast_mut().cast(),
            callback: slot_password,
        }
    } else {
        PasswordBinding {
            userdata: identity,
            callback: native_password,
        }
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
    // SAFETY: a replaced context may remain alive in a value copy; detach its native identity first.
    unsafe { detach_bound_password(state) };
    // SAFETY: no callback is active; publish the resulting OpenSSL reference.
    unsafe { (*fields).context = ctx };
    // SAFETY: create's normalized protocol mask belongs to the same owner.
    unsafe { (*fields).protocols = config.protocols };
    // SAFETY: remember only a native password binding actually installed by this create.
    unsafe {
        *state.bound_password_context.get() =
            if !identity.is_null() && config.passphrase.length != 0 {
                ctx
            } else {
                ptr::null_mut()
            }
    };
    if !previous.is_null() {
        // SAFETY: replacement releases the previous owned context reference.
        unsafe { SSL_CTX_free(previous) };
    }
    // SAFETY: the new context's live password data is retained independently of native views.
    unsafe { *state.context_password.get() = retained_password };
    !ctx.is_null()
}
macro_rules! owned_method {
    ($name:ident, $method:ident) => {
        /// # Safety
        /// Owner and borrowed OpenSSL context are live and externally synchronized.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(owner: *const OwnedTlsContext) -> bool {
            // SAFETY: temporary raw views into the live, unchanged configuration.
            let config = unsafe { configuration(owner) };
            // SAFETY: borrow interior-mutable metadata, without an exclusive record reference.
            let state = unsafe { &*owner };
            let fields = state.fields.get();
            // SAFETY: synchronized field observation before OpenSSL/log callbacks.
            let ctx = unsafe { (*fields).context };
            // SAFETY: all views remain live and the logger is nonthrowing.
            unsafe { crate::$method(ctx, &raw const config) }
        }
    };
}
owned_method!(swr_v1_tls_owner_set_capath, swr_v1_tls_set_capath);
owned_method!(swr_v1_tls_owner_set_ciphers, swr_v1_tls_set_ciphers);
owned_method!(
    swr_v1_tls_owner_set_client_certificate,
    swr_v1_tls_set_client_certificate
);
owned_method!(swr_v1_tls_owner_set_ecdh_curve, swr_v1_tls_set_ecdh_curve);
owned_method!(swr_v1_tls_owner_set_dhparam, swr_v1_tls_set_dhparam);
/// Path access and assignment are one Rust operation. 1 access denied, otherwise
/// `string_set`'s status; only cert(4), key(5) and client certificate(7) are accepted.
/// # Safety
/// owner is live/exclusive; path is terminated and readable for length bytes;
/// native cache callbacks catch exceptions and do not reenter.
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
    if previous != fields.context {
        // SAFETY: the old context's remaining copies must not retain this replaced native identity.
        unsafe { detach_bound_password(destination) };
    }
    // SAFETY: slots are stable and exclusive; publish the copied scalar values/reference.
    unsafe { *destination.fields.get() = fields };
    // SAFETY: private ownership metadata copied without any native callback.
    let bound = unsafe { *source.bound_password_context.get() };
    // SAFETY: the destination is exclusive; its native identity itself is not copied.
    unsafe { *destination.bound_password_context.get() = bound };
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_tls_owner_socket_context(
    owner: *const OwnedTlsContext,
    socket: *mut swoole_rs_reactor::SocketRecord,
    http2: bool,
    dtls_supported: bool,
) -> c_int {
    // SAFETY: the owner is live throughout the operation.
    let owner_ref = unsafe { &*owner };
    let fields = owner_ref.fields.get();
    // SAFETY: a short synchronized observation of the live socket.
    let kind = unsafe { (*socket).socket_type };
    if swoole_rs_reactor::type_is(kind, swoole_rs_reactor::CLASS_DGRAM) {
        if !dtls_supported {
            return 1;
        }
        // SAFETY: caller exclusively lends socket state.
        unsafe { (*socket).dtls = 1 };
        // SAFETY: the live owner's fields are exclusive and stable.
        unsafe { (*fields).protocols = crate::SslVersion::SW_SSL_DTLS as u32 };
        // SAFETY: caller exclusively lends the socket's transfer limit.
        unsafe { (*socket).chunk_size = swoole_rs_corebase::constants::SW_SSL_BUFFER_SIZE };
    }
    // SAFETY: a leaf update before OpenSSL callbacks may run.
    unsafe { (*fields).http_v2 = u8::from(http2) };
    // SAFETY: initialization and exclusive owner access are the caller contract.
    if !unsafe { swr_v1_tls_owner_create(owner) } {
        return 2;
    }
    // SAFETY: context creation returned; publish send mode without a retained Rust borrow.
    unsafe { (*socket).output.ssl_send = 1 };
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_tls_owner_socket_session(
    owner: *const OwnedTlsContext,
    socket: *mut swoole_rs_reactor::SocketRecord,
    hostname: *const StringSlot,
    grease: Option<unsafe extern "C" fn(*mut openssl_sys::SSL, bool)>,
) -> OwnedTlsSessionResult {
    // SAFETY: copy the current session pointer; no Rust borrow crosses OpenSSL.
    let ssl = unsafe { (*socket).ssl };
    if !ssl.is_null() {
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
    // SAFETY: the Rust reactor record's canonical TLS group has the asserted C layout.
    let tls = unsafe { ptr::addr_of_mut!((*socket).tls).cast::<crate::SocketTls>() };
    // SAFETY: a stable slot in the live canonical record.
    let output = unsafe { ptr::addr_of_mut!((*socket).ssl).cast() };
    // SAFETY: a leaf descriptor observation before any OpenSSL callback.
    let fd = unsafe { (*socket).fd };
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
    // SAFETY: the preceding successful creation populated this session slot.
    let ssl = unsafe { (*socket).ssl.cast::<openssl_sys::SSL>() };
    if let Some(grease) = grease {
        // SAFETY: the optional native leaf accepts the live session and does not reenter.
        unsafe { grease(ssl, fields.grease != 0) };
    }
    // SAFETY: the session is live; OpenSSL exposes its set-mode macro as SSL_ctrl.
    unsafe {
        openssl_sys::SSL_ctrl(
            ssl,
            openssl_sys::SSL_CTRL_MODE,
            openssl_sys::SSL_MODE_ACCEPT_MOVING_WRITE_BUFFER,
            ptr::null_mut(),
        )
    };
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

#[cfg(test)]
mod tests {
    use super::*;
    extern "C" fn warning(_function: *const c_char, _message: *const c_char) {
        panic!("unexpected TLS warning");
    }
    extern "C" fn custom(
        _buffer: *mut c_char,
        _capacity: c_int,
        _verify: c_int,
        _data: *mut c_void,
    ) -> c_int {
        0
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
                let socket = swoole_rs_reactor::swr_v1_socket_make(stream.as_raw_fd(), 6);
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
                swoole_rs_reactor::swr_v1_socket_dealloc(socket);
                swr_v1_tls_owner_detach(prepared.owner);
                swr_v1_tls_owner_free(prepared.owner);
            }
        }
    }
    #[test]
    fn source_replacement_detaches_older_shared_password_bindings() {
        unsafe {
            for replace_by_assignment in [false, true] {
                let source = swr_v1_tls_owner_new(warning);
                let copy = swr_v1_tls_owner_new(warning);
                let mut native = [source.owner];
                let identity = native.as_mut_ptr().cast();
                swr_v1_tls_owner_bind(source.owner, identity);
                assert_eq!(
                    swoole_rs_corebase::swr_v1_core_string_set(
                        source.strings[PASSPHRASE],
                        c"before".as_ptr(),
                        6
                    ),
                    0
                );
                assert!(swr_v1_tls_owner_create(source.owner));
                let older = (*source.fields).context;
                assert_eq!(swr_v1_tls_owner_assign(copy.owner, source.owner).status, 0);
                if replace_by_assignment {
                    let replacement = swr_v1_tls_owner_new(warning);
                    assert_eq!(
                        swoole_rs_corebase::swr_v1_core_string_set(
                            replacement.strings[PASSPHRASE],
                            c"after".as_ptr(),
                            5
                        ),
                        0
                    );
                    assert!(swr_v1_tls_owner_create(replacement.owner));
                    assert_eq!(
                        swr_v1_tls_owner_assign(source.owner, replacement.owner).status,
                        0
                    );
                    swr_v1_tls_owner_free(replacement.owner);
                } else {
                    assert_eq!(
                        swoole_rs_corebase::swr_v1_core_string_set(
                            source.strings[PASSPHRASE],
                            c"after".as_ptr(),
                            5
                        ),
                        0
                    );
                    assert!(swr_v1_tls_owner_create(source.owner));
                    assert!(crate::context::password_binding_matches(
                        (*source.fields).context,
                        identity,
                        native_password
                    ));
                }
                let password = (*copy.owner).context_password.get();
                let slot = Arc::as_ptr((&*password).as_ref().unwrap());
                assert!(crate::context::password_binding_matches(
                    older,
                    slot.cast_mut().cast(),
                    slot_password
                ));
                swr_v1_tls_owner_detach(source.owner);
                swr_v1_tls_owner_free(source.owner);
                let mut output = [0u8; 16];
                assert_eq!(
                    slot_password(output.as_mut_ptr().cast(), 16, 0, slot.cast_mut().cast()),
                    5
                );
                assert_eq!(&output[..6], b"after\0");
                swr_v1_tls_owner_free(copy.owner);
            }
        }
    }
    #[test]
    fn copied_ready_context_retains_independent_configuration_and_live_password() {
        unsafe {
            let source = swr_v1_tls_owner_new(warning);
            let destination = swr_v1_tls_owner_new(warning);
            let mut native = [source.owner];
            swr_v1_tls_owner_bind(source.owner, native.as_mut_ptr().cast());
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
            swr_v1_tls_owner_detach(source.owner);
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
            swr_v1_tls_owner_detach(destination.owner);
            swr_v1_tls_owner_free(destination.owner);
        }
    }
    #[test]
    fn shared_context_keeps_live_passphrase_after_native_view_detaches() {
        unsafe {
            let prepared = swr_v1_tls_owner_new(warning);
            let owner = prepared.owner;
            let mut native_header = [owner];
            let identity = native_header.as_mut_ptr().cast();
            swr_v1_tls_owner_bind(owner, identity);
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
            assert!(crate::context::password_binding_matches(
                ctx,
                identity,
                native_password
            ));
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
                        native_password(bytes.as_mut_ptr().cast(), capacity, 0, identity),
                        c_int::try_from(length).unwrap()
                    );
                    assert_eq!(bytes, expected);
                }
            }
            let retained = swr_v1_tls_owner_clone(owner);
            swr_v1_tls_owner_detach(owner);
            let slot = prepared.strings[PASSPHRASE].cast_mut().cast();
            assert!(crate::context::password_binding_matches(
                ctx,
                slot,
                slot_password
            ));
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
    #[test]
    fn detach_preserves_a_custom_openssl_password_binding() {
        unsafe {
            let prepared = swr_v1_tls_owner_new(warning);
            let owner = prepared.owner;
            let mut native_header = [owner];
            swr_v1_tls_owner_bind(owner, native_header.as_mut_ptr().cast());
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
            let mut token = 123;
            let data = ptr::from_mut(&mut token).cast();
            crate::context::bind_password(ctx, data, custom);
            swr_v1_tls_owner_detach(owner);
            assert!(crate::context::password_binding_matches(ctx, data, custom));
            swr_v1_tls_owner_free(owner);
        }
    }
}
