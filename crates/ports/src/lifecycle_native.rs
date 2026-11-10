//! The effects of `ListenPort`'s lifecycle runs ([`crate::lifecycle`]), over the
//! records Rust lays out: the port's settings and TLS state, its `SSLContext`s'
//! Rust owners, the socket record and OpenSSL. Logs name the upstream function
//! that wrote them.
//!
//! `PortConfig::ssl_context` holds one reference to the port's own context's
//! owner, and its TLS state (`PortConfig::tls`) what upstream's `ListenPort` kept
//! in C++ containers: the SNI contexts (`sni_contexts`) and the DTLS sessions
//! (`dtls_sessions`), each with a reference to its context.
use core::ffi::{c_int, c_void};
use core::ptr;
use std::collections::HashMap;

use openssl_sys::{
    SSL, SSL_CTX_set_tlsext_servername_callback__fixed_rust, SSL_get_ex_data, SSL_get_servername,
    SSL_set_SSL_CTX, SSL_set_ex_data, TLSEXT_NAMETYPE_host_name,
};
use swoole_rs_clients::address::{swr_v1_clients_address_host, swr_v1_clients_address_port};
use swoole_rs_corebase::constants::{SW_SSL_CIPHER_LIST, SW_SSL_ECDH_CURVE};
use swoole_rs_corebase::{log, swr_v1_core_string_set};
use swoole_rs_reactor::{SocketRecord, swr_v1_socket_type};
use swoole_rs_server::port_config::{self, PortConfig};
use swoole_rs_tls::upstream::socket_ssl_create;
use swoole_rs_tls::{
    DtlsSession, OwnedTlsContext, OwnedTlsFields, SslCreateFlag, SslVersion, Text, has_files,
    set_text, ssl_context_create, swr_v1_tls_dtls_free, swr_v1_tls_dtls_init, swr_v1_tls_dtls_new,
    swr_v1_tls_owner_clone, swr_v1_tls_owner_free, swr_v1_tls_owner_new, swr_v1_tls_owner_view,
    swr_v1_tls_port_index, swr_v1_tls_warning,
};

use crate::lifecycle::{
    CONTEXT_PREFER_SERVER_CIPHERS, CONTEXT_SESSION_TICKETS, CONTEXT_STAPLING, IMPORT_ACCEPTCONN,
    IMPORT_NAME, IMPORT_TYPE, OP_ACCEPTCONN, OP_ADOPT, OP_ALLOW_EMPTY_CERT, OP_CERT_EMPTY,
    OP_CONTEXT_CIPHERS, OP_CONTEXT_DTLS, OP_CONTEXT_FLAG, OP_CONTEXT_NEW, OP_CREATE,
    OP_DELETE_DTLS, OP_DTLS_SUPPORTED, OP_FREE_SOCKET, OP_GET_ACCEPTCONN, OP_GET_NAME, OP_GET_TYPE,
    OP_HAS_CONTEXT, OP_HAS_SNI, OP_KEY_EMPTY, OP_LOG_REQUIRE_CERT, OP_RESET_CONTEXT,
    OP_SERVER_NAME, OP_SET_CREATE_ERROR, OP_SET_EX_DATA, OP_SET_HTTP, OP_SET_HTTP2,
    OP_SET_SNI_CALLBACK, OP_SNI_EMPLACE, OP_SNI_EMPTY, OP_SNI_NEXT, OP_SNI_SELECT, OP_SOCKET_FAIL,
    OP_SOCKET_NEW, OP_SOCKET_SSL, OP_SSL_PORT, OP_UNLINK, OP_WARN_CREATE, OP_WARN_EX_DATA,
    OP_WARN_IMPORT, OP_WARN_NO_DTLS, RUN_SNI_ADD,
};
use crate::methods::{FD_DGRAM_SERVER, FD_STREAM_SERVER, socket_free};

/// `SW_LOG_WARNING`, `SW_LOG_ERROR`, `SW_ERROR_WRONG_OPERATION` and
/// `SW_ERROR_SSL_CREATE_SESSION_FAILED`.
const LOG_WARNING: c_int = 4;
const LOG_ERROR: c_int = 5;
const WRONG_OPERATION: c_int = 509;
const SSL_CREATE_SESSION_FAILED: c_int = 1020;

/// What `ListenPort` kept in C++ containers.
struct TlsState {
    /// `sni_contexts`, in insertion order: the name (NUL-terminated) and a
    /// reference to its context.
    sni: Vec<(Vec<u8>, *const OwnedTlsContext)>,
    /// `dtls_sessions` by fd: each session and its reference to the port's context.
    dtls: HashMap<c_int, (*mut DtlsSession, *const OwnedTlsContext)>,
}

/// The port's TLS state, made on first use.
fn state(config: &mut PortConfig) -> &mut TlsState {
    if config.tls.is_null() {
        let state = TlsState {
            sni: Vec::new(),
            dtls: HashMap::new(),
        };
        config.tls = Box::into_raw(Box::new(state)).cast();
    }
    // SAFETY: the port's own box, made above or earlier, borrowed with its record.
    unsafe { &mut *config.tls.cast::<TlsState>() }
}

/// How many SNI contexts the port has.
fn sni_count(config: &PortConfig) -> usize {
    if config.tls.is_null() {
        return 0;
    }
    // SAFETY: the port's own box, borrowed with its record.
    unsafe { &*config.tls.cast::<TlsState>() }.sni.len()
}

/// Releases the port's context and its TLS state with the SNI contexts.
pub(crate) fn tls_free(config: &mut PortConfig) {
    // SAFETY: the port's reference (or null), released once.
    unsafe { swr_v1_tls_owner_free(core::mem::replace(&mut config.ssl_context, ptr::null())) };
    if config.tls.is_null() {
        return;
    }
    // SAFETY: the port's own box, taken once.
    let state = unsafe { Box::from_raw(config.tls.cast::<TlsState>()) };
    config.tls = ptr::null_mut();
    for (_, context) in state.sni {
        // SAFETY: a reference the state held, released once.
        unsafe { swr_v1_tls_owner_free(context) };
    }
}

/// `ListenPort::dup_ssl_context()`: a new context with the port's settings, and
/// one reference to it.
///
/// # Safety
/// `port` is a live `ListenPort` with a context.
pub unsafe fn context_dup(port: *const c_void) -> *const OwnedTlsContext {
    // SAFETY: as the caller guarantees; only read.
    let source = unsafe { port_config::of(port.cast_mut()) }.ssl_context;
    let copy = swr_v1_tls_owner_new(swr_v1_tls_warning).owner;
    // SAFETY: two live owners; no native view caches either's strings.
    if unsafe { swoole_rs_tls::swr_v1_tls_owner_assign(copy, source) }.status != 0 {
        std::process::abort();
    }
    copy
}

/// `ListenPort::ssl_add_sni_cert(name, ctx)`: creates the context (both files
/// required) and stores it under `name` unless the name is taken. Takes the
/// caller's reference to `context`.
///
/// # Safety
/// `port` is a live `ListenPort`, and `context` a live owner the caller holds a
/// reference to.
pub unsafe fn sni_add(port: *mut c_void, name: &[u8], context: *const OwnedTlsContext) -> bool {
    let mut key = name.to_vec();
    key.push(0);
    let mut run = Run::new(port);
    run.target = context;
    run.sni = Some(key);
    // SAFETY: as the caller guarantees.
    let added = unsafe { run.run(RUN_SNI_ADD) } != 0;
    if run.sni.is_some() {
        // SAFETY: not stored, so still the caller's reference, released once.
        unsafe { swr_v1_tls_owner_free(context) };
    }
    added
}

/// The DTLS session on `fd`; null if none.
pub(crate) fn dtls_session(config: &mut PortConfig, fd: c_int) -> *mut DtlsSession {
    let dtls = &state(config).dtls;
    dtls.get(&fd).map_or(ptr::null_mut(), |entry| entry.0)
}

/// `delete` of the DTLS session on `fd`, removed from the port; nothing if none.
/// Nothing uses the session afterwards.
pub(crate) fn dtls_free(config: &mut PortConfig, fd: c_int) {
    let dtls = &mut state(config).dtls;
    if let Some((session, context)) = dtls.remove(&fd) {
        // SAFETY: the session and reference the port owned, released once.
        unsafe { swr_v1_tls_dtls_free(session) };
        // SAFETY: as above.
        unsafe { swr_v1_tls_owner_free(context) };
    }
}

/// `ListenPort::create_dtls_session(sock)`: a session over the port's context,
/// stored under the socket's fd; null when it fails to initialize.
pub(crate) fn dtls_create(port: *mut c_void, sock: *mut SocketRecord) -> *mut DtlsSession {
    // SAFETY: a live DTLS port of the server, which `PortMethods` takes.
    let config = unsafe { port_config::of(port) };
    let context = config.ssl_context;
    let state = state(config);
    let session = swr_v1_tls_dtls_new(sock);
    // SAFETY: the fresh session over the caller's live socket, and the port's live context.
    if !unsafe { swr_v1_tls_dtls_init(session, context) } {
        // SAFETY: the session made above, which nothing else holds.
        unsafe { swr_v1_tls_dtls_free(session) };
        return ptr::null_mut();
    }
    // SAFETY: the port's live context.
    let shared = unsafe { swr_v1_tls_owner_clone(context) };
    // SAFETY: the live socket `PortMethods` takes.
    let fd = unsafe { (*sock).fd };
    state.dtls.insert(fd, (session, shared));
    session
}

/// The fields of `context`.
///
/// # Safety
/// `context` is a live owner, and nothing else uses its fields while the borrow lasts.
unsafe fn fields<'a>(context: *const OwnedTlsContext) -> &'a mut OwnedTlsFields {
    // SAFETY: as the caller guarantees.
    let fields = unsafe { swr_v1_tls_owner_view(context) }.fields;
    // SAFETY: the owner's fields live as long as the reference.
    unsafe { &mut *fields }
}

/// A native `int`, as the `u64` [`crate::lifecycle`] reads back signed.
fn int(value: c_int) -> u64 {
    i64::from(value).cast_unsigned()
}

/// One lifecycle run: its port and arguments, and what its steps found.
pub(crate) struct Run {
    /// The `ListenPort`; for the SNI callback, the one the `SSL` names.
    pub(crate) port: *mut c_void,
    /// The `SSLContext` `ssl_context_create` works on.
    pub(crate) target: *const OwnedTlsContext,
    pub(crate) allow_empty_cert: bool,
    pub(crate) sock: *mut SocketRecord,
    pub(crate) ssl: *mut SSL,
    pub(crate) fd: c_int,
    /// The SNI name `RUN_SNI_ADD` stores `target` under; taken once stored.
    sni: Option<Vec<u8>>,
    so_type: c_int,
    acceptconn: c_int,
    /// The SNI entry `OP_SNI_NEXT` reached.
    entry: Option<usize>,
}

impl Run {
    pub(crate) fn new(port: *mut c_void) -> Self {
        Self {
            port,
            target: ptr::null(),
            allow_empty_cert: false,
            sock: ptr::null_mut(),
            ssl: ptr::null_mut(),
            fd: -1,
            sni: None,
            so_type: 0,
            acceptconn: 0,
            entry: None,
        }
    }

    /// The lifecycle operation `kind` over this run.
    ///
    /// # Safety
    /// `port` is null only for `RUN_SNI`, else a live `ListenPort`; the rest are
    /// what `kind` reads.
    pub(crate) unsafe fn run(&mut self, kind: u32) -> i32 {
        let config = if self.port.is_null() {
            ptr::null()
        } else {
            // SAFETY: as the caller guarantees.
            ptr::from_ref::<PortConfig>(unsafe { port_config::of(self.port) })
        };
        // SAFETY: `call` answers over this live run, and `config` is its port's record.
        unsafe { crate::lifecycle::run(kind, (&raw mut *self).cast(), call, config) }
    }

    /// The port's record, which only this run uses meanwhile.
    fn config<'a>(&self) -> &'a mut PortConfig {
        // SAFETY: a run's port is live (the SNI callback's once `OP_SSL_PORT` found it).
        unsafe { port_config::of(self.port) }
    }

    /// The port's own context.
    fn context(&self) -> *const OwnedTlsContext {
        self.config().ssl_context
    }
}

/// OpenSSL's servername callback, as `ListenPort::ssl_server_sni_callback`.
unsafe extern "C" fn sni_callback(ssl: *mut SSL, _al: *mut c_int, _arg: *mut c_void) -> c_int {
    // SAFETY: OpenSSL's live handshake.
    unsafe { crate::methods::ssl_server_sni_callback(ssl.cast()) }
}

/// `getsockopt(fd, SOL_SOCKET, option)` into `value`: 0 or -1.
fn get_option(fd: c_int, option: c_int, value: &mut c_int) -> c_int {
    let mut size = libc::socklen_t::try_from(size_of::<c_int>()).unwrap_or_default();
    let value = (&raw mut *value).cast();
    // SAFETY: a live c_int of `size` bytes.
    unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, option, value, &raw mut size) }
}

/// One fact or effect of a run (a [`crate::read::PortsCall`]).
///
/// # Safety
/// `context` is a live [`Run`] whose pointers are what `op` reads.
unsafe extern "C" fn call(context: *mut c_void, op: u32, arg: u64) -> u64 {
    // SAFETY: as the caller guarantees.
    let run = unsafe { &mut *context.cast::<Run>() };
    match op {
        OP_CERT_EMPTY..=OP_ALLOW_EMPTY_CERT => create_op(run, op),
        OP_CONTEXT_NEW..=OP_SNI_SELECT => context_op(run, op, arg),
        _ => socket_op(run, op, arg),
    }
}

/// Whether the run's target context has a certificate file and a key file.
fn target_files(run: &Run) -> (bool, bool) {
    // SAFETY: the run's live target context.
    unsafe { has_files(run.target) }
}

/// `ssl_context_create`'s, `ssl_add_sni_cert`'s and `ssl_create`'s ops.
fn create_op(run: &mut Run, op: u32) -> u64 {
    match op {
        OP_CERT_EMPTY => u64::from(!target_files(run).0),
        OP_KEY_EMPTY => u64::from(!target_files(run).1),
        OP_LOG_REQUIRE_CERT => {
            log::error_log(
                LOG_ERROR,
                WRONG_OPERATION,
                c"ListenPort::ssl_context_create",
                format_args!("require `ssl_cert_file` and `ssl_key_file` options"),
            );
            0
        }
        OP_SET_HTTP => {
            // SAFETY: as above.
            unsafe { fields(run.target) }.http = 1;
            0
        }
        OP_SET_HTTP2 => {
            // SAFETY: as above.
            unsafe { fields(run.target) }.http_v2 = 1;
            0
        }
        // SAFETY: as above.
        OP_CREATE => u64::from(unsafe { ssl_context_create(run.target) }),
        OP_WARN_CREATE => {
            log::warning(
                c"ListenPort::ssl_context_create",
                format_args!("failed to create ssl content"),
            );
            0
        }
        OP_SNI_EMPLACE => {
            let state = state(run.config());
            let fresh = |name: &mut Vec<u8>| !state.sni.iter().any(|stored| stored.0 == *name);
            if let Some(name) = run.sni.take_if(fresh) {
                state.sni.push((name, run.target));
            }
            0
        }
        OP_HAS_SNI => u64::from(sni_count(run.config()) != 0),
        OP_SET_SNI_CALLBACK => {
            // SAFETY: the port's live context.
            let context = unsafe { fields(run.context()) }.context;
            // SAFETY: its live SSL_CTX and a callback of OpenSSL's signature.
            unsafe {
                SSL_CTX_set_tlsext_servername_callback__fixed_rust(context, Some(sni_callback))
            };
            0
        }
        OP_SOCKET_SSL => {
            let flags = SslCreateFlag::SW_SSL_SERVER as c_int;
            // SAFETY: the run's live socket and the port's live context.
            int(unsafe { socket_ssl_create(run.sock, run.context(), flags) })
        }
        OP_SET_CREATE_ERROR => {
            log::set_last_error(SSL_CREATE_SESSION_FAILED);
            0
        }
        OP_SET_EX_DATA => {
            // SAFETY: the run's live socket, whose SSL `OP_SOCKET_SSL` made.
            let ssl = unsafe { (*run.sock).ssl }.cast::<SSL>();
            // SAFETY: a live SSL; the port outlives its connections.
            int(unsafe { SSL_set_ex_data(ssl, swr_v1_tls_port_index(), run.port) })
        }
        OP_WARN_EX_DATA => {
            log::warning(
                c"ListenPort::ssl_create",
                format_args!("SSL_set_ex_data() failed"),
            );
            0
        }
        OP_ALLOW_EMPTY_CERT => u64::from(run.allow_empty_cert),
        _ => std::process::abort(),
    }
}

/// `ssl_context_init`'s ops and the SNI callback's.
fn context_op(run: &mut Run, op: u32, arg: u64) -> u64 {
    match op {
        OP_CONTEXT_NEW => {
            let context = swr_v1_tls_owner_new(swr_v1_tls_warning).owner;
            let previous = core::mem::replace(&mut run.config().ssl_context, context);
            // SAFETY: the replaced reference (or null), released once.
            unsafe { swr_v1_tls_owner_free(previous) };
            0
        }
        OP_CONTEXT_FLAG => {
            // SAFETY: the port's live context.
            let fields = unsafe { fields(run.context()) };
            let value = u8::from(arg & 1 != 0);
            match arg >> 1 {
                CONTEXT_PREFER_SERVER_CIPHERS => fields.prefer_server_ciphers = value,
                CONTEXT_SESSION_TICKETS => fields.session_tickets = value,
                CONTEXT_STAPLING => fields.stapling = value,
                _ => fields.stapling_verify = value,
            }
            0
        }
        OP_CONTEXT_CIPHERS => {
            let owner = run.context();
            // SAFETY: the port's live context, which only this run writes.
            unsafe { set_text(owner, Text::Ciphers, SW_SSL_CIPHER_LIST.as_bytes()) };
            // SAFETY: as above.
            unsafe { set_text(owner, Text::EcdhCurve, SW_SSL_ECDH_CURVE.as_bytes()) };
            0
        }
        // crates/tls requires OpenSSL 3, which has DTLS.
        OP_DTLS_SUPPORTED => 1,
        OP_CONTEXT_DTLS => {
            // SAFETY: the port's live context.
            unsafe { fields(run.context()) }.protocols = SslVersion::SW_SSL_DTLS as u32;
            state(run.config()).dtls.clear();
            0
        }
        OP_WARN_NO_DTLS => {
            log::warning(
                c"ListenPort::ssl_context_init",
                format_args!("DTLS support require openssl-1.1 or later"),
            );
            0
        }
        OP_SERVER_NAME => {
            // SAFETY: the handshake's live SSL.
            let name = unsafe { SSL_get_servername(run.ssl, TLSEXT_NAMETYPE_host_name) };
            name.addr() as u64
        }
        OP_SSL_PORT => {
            // SAFETY: as above; `ssl_create` stored its port.
            run.port = unsafe { SSL_get_ex_data(run.ssl, swr_v1_tls_port_index()) };
            0
        }
        OP_SNI_EMPTY => u64::from(sni_count(run.config()) == 0),
        OP_SNI_NEXT => {
            let next = run.entry.map_or(0, |entry| entry + 1);
            run.entry = Some(next);
            let sni = &state(run.config()).sni;
            sni.get(next)
                .map_or(0, |entry| entry.0.as_ptr().addr() as u64)
        }
        OP_SNI_SELECT => {
            let entry = run.entry.unwrap_or_default();
            if let Some(&(_, context)) = state(run.config()).sni.get(entry) {
                // SAFETY: a stored live context.
                let ctx = unsafe { fields(context) }.context;
                // SAFETY: the handshake's live SSL and that context's SSL_CTX.
                unsafe { SSL_set_SSL_CTX(run.ssl, ctx) };
            }
            0
        }
        _ => std::process::abort(),
    }
}

/// `import`'s and `close`'s ops.
fn socket_op(run: &mut Run, op: u32, arg: u64) -> u64 {
    match op {
        OP_SOCKET_NEW => {
            run.sock = SocketRecord::with_fd(run.fd, 0).into_raw();
            run.config().socket = run.sock.cast();
            0
        }
        OP_GET_TYPE => int(get_option(run.fd, libc::SO_TYPE, &mut run.so_type)),
        // SAFETY: the run's live socket.
        OP_GET_NAME => int(unsafe { (*run.sock).get_name() }),
        OP_GET_ACCEPTCONN => int(get_option(run.fd, libc::SO_ACCEPTCONN, &mut run.acceptconn)),
        OP_ACCEPTCONN => int(run.acceptconn),
        OP_WARN_IMPORT => {
            warn_import(run.fd, arg);
            0
        }
        OP_SOCKET_FAIL => {
            // The fd stays the caller's. Upstream's port keeps pointing at the
            // freed record; this one forgets it, which deleting the port frees.
            // SAFETY: the record `OP_SOCKET_NEW` made.
            unsafe { (*run.sock).fd = -1 };
            // SAFETY: as above, freed once.
            unsafe { SocketRecord::free(run.sock) };
            run.config().socket = ptr::null_mut();
            0
        }
        OP_ADOPT => {
            adopt(run);
            0
        }
        OP_HAS_CONTEXT => u64::from(!run.context().is_null()),
        OP_RESET_CONTEXT => {
            let context = core::mem::replace(&mut run.config().ssl_context, ptr::null());
            // SAFETY: the port's reference, released once.
            unsafe { swr_v1_tls_owner_free(context) };
            0
        }
        OP_DELETE_DTLS => {
            state(run.config()).dtls.clear();
            0
        }
        OP_FREE_SOCKET => {
            let config = run.config();
            // SAFETY: the port's live socket, which nothing uses after.
            unsafe { socket_free(config.socket.cast()) };
            config.socket = ptr::null_mut();
            0
        }
        OP_UNLINK => {
            // SAFETY: the host's NUL-terminated path.
            unsafe { libc::unlink(run.config().host().data) };
            0
        }
        _ => std::process::abort(),
    }
}

fn warn_import(fd: c_int, which: u64) {
    let origin = c"ListenPort::import";
    match which {
        IMPORT_TYPE => log::sys_warning(
            origin,
            format_args!("getsockopt({fd}, SOL_SOCKET, SO_TYPE) failed"),
        ),
        IMPORT_NAME => log::sys_warning(origin, format_args!("getsockname({fd}) failed")),
        IMPORT_ACCEPTCONN => log::sys_warning(
            origin,
            format_args!("getsockopt({fd}, SOL_SOCKET, SO_ACCEPTCONN) failed"),
        ),
        _ => log::error_log(
            LOG_WARNING,
            libc::EINVAL,
            origin,
            format_args!("the socket[{fd}] is not a listening socket"),
        ),
    }
}

/// The port takes the imported socket's type, host and port, listens, and
/// marks the socket a server socket.
fn adopt(run: &Run) {
    let config = run.config();
    // SAFETY: the record `OP_SOCKET_NEW` made, which `getsockname` filled.
    let socket = unsafe { &mut *run.sock };
    // SAFETY: every member of the address union starts with the family.
    let family = c_int::from(unsafe { socket.info.addr.ss.sa_family });
    let kind = swr_v1_socket_type(family, run.so_type);
    socket.socket_type = kind;
    socket.info.kind = kind;
    config.r#type = kind;
    // SAFETY: the socket's address, typed above.
    let host = unsafe { swr_v1_clients_address_host(&raw const socket.info) };
    // SAFETY: a NUL-terminated string.
    let length = unsafe { libc::strlen(host) };
    // SAFETY: the port's host slot and `length` readable bytes.
    if unsafe { swr_v1_core_string_set(&raw const *config.host, host, length) } != 0 {
        std::process::abort();
    }
    // SAFETY: as above.
    config.port = unsafe { swr_v1_clients_address_port(&raw const socket.info) };
    config.listening = true;
    socket.fd_type = if matches!(kind, 2 | 4 | 6) {
        FD_DGRAM_SERVER
    } else {
        FD_STREAM_SERVER
    };
    socket.removed = 1;
}
