//! `Swoole\Client`: registration, the object and every method, over
//! `network::Client`. The object holds a [`Conn`]: crates/clients' client plus what
//! the PHP layer keeps beside it (the persistent-connection key and count and
//! the `recv` framing buffer). A `SWOOLE_KEEP` connection outlives its object
//! in the pool. Each method parses as upstream's ZPP block does; operations
//! that can run PHP code go through `Cli::run` inside `zend_try`.
use crate::native::{self, Cli, ErrorKind};
use libc::{c_char, c_int};
use std::{
    cell::Cell,
    ffi::{CStr, CString},
    marker::PhantomData,
    ptr,
    rc::Rc,
    sync::{Mutex, MutexGuard, PoisonError},
};
use swoole_rs_clients::{
    client::{self as state, Op},
    pool::Pool,
    swr_v1_clients_port,
};
use swoole_rs_corebase::net_types::{Protocol, SocketAddress};
use swoole_rs_phpruntime::Report;
use swoole_rs_phpstubs::client;
use swoole_rs_phpswoole::{Switch, built_with, display_errors};
use swoole_rs_reactor::{Socket as Net, SocketFields};
use swoole_rs_zend::{
    Call, Class, E_ERROR, E_WARNING, Minit, Object, Owned, OwnedArray, Props, Value, ZStr, ZString,
    bailout::{self, Bailout},
    host::callable::Callable,
    string::ZBuf,
    sys::{
        IS_ARRAY, IS_UNDEF, ZEND_ACC_NOT_SERIALIZABLE, ZEND_ACC_PUBLIC, get_active_class_name,
        get_active_function_name, zend_class_entry, zend_object, zend_object_handlers,
        zend_type_error, zval,
    },
    unset_property_deny,
};
use swoole_rs_zend::{method_table, sys as z};

/// `SW_FLAG_ASYNC`, `SW_FLAG_SYNC`, `SW_FLAG_KEEP` and `SW_SOCK_SSL`.
const FLAG_ASYNC: i64 = 1 << 10;
const FLAG_SYNC: i64 = 1 << 11;
const FLAG_KEEP: i64 = 1 << 12;
pub(crate) const SOCK_SSL: i64 = 1 << 9;
/// `SW_SOCK_*`.
pub(crate) const SOCK_TCP: i64 = 1;
pub(crate) const SOCK_TCP6: i64 = 3;
pub(crate) const SOCK_UNIX_STREAM: i64 = 5;
pub(crate) const SOCK_UNIX_DGRAM: i64 = 6;
/// `SW_PHP_CLIENT_BUFFER_SIZE`, `SW_BUFFER_SIZE_STD` and `SW_BUFFER_SIZE_BIG`.
const PHP_BUFFER_SIZE: i64 = 65535;
const BUFFER_SIZE_STD: usize = 8192;
const BUFFER_SIZE_BIG: usize = 65536;
/// `SW_CLIENT_CONNECT_TIMEOUT`.
pub(crate) const CONNECT_TIMEOUT: f64 = 0.5;
/// `SW_ERROR_CLIENT_NO_CONNECTION` and `SW_ERROR_INVALID_PARAMS`.
const NO_CONNECTION: c_int = 5001;
pub(crate) const INVALID_PARAMS: c_int = 505;

/// One connection: the `network::Client` and what the PHP layer keeps with it
/// (upstream's `keep`, `server_id`, `reuse_count`, `buffer` and
/// `ssl_wait_handshake` client fields).
///
/// The object shares it (`Rc`, cells), so PHP code a method runs can re-enter
/// the object and [`release`] the connection while the method still holds it:
/// the native client then lives until the last holder drops it. `close()` never
/// pools a connection a method holds, so the pool's copy is never in use.
pub(crate) struct Conn {
    pub(crate) cli: Cli,
    /// [`release`] ran while a method held it: the last drop destroys `cli`.
    released: Cell<bool>,
    key: Vec<u8>,
    keep: bool,
    reuse_count: u32,
    /// `cli->buffer`: taken for the length of a framed `recv`.
    frame: Cell<Frame>,
    pub(crate) ssl_wait: Cell<bool>,
}

/// `cli->buffer`'s bytes, and its size (0: no buffer).
#[derive(Default)]
struct Frame {
    buffer: Vec<u8>,
    size: usize,
}

impl Conn {
    pub(crate) fn new(cli: Cli) -> Self {
        Self {
            cli,
            released: Cell::new(false),
            key: Vec::new(),
            keep: false,
            reuse_count: 0,
            frame: Cell::default(),
            ssl_wait: Cell::new(false),
        }
    }

    /// The connection for the pool: the same client and state, apart from any
    /// handle a re-entered method still holds (upstream pools the pointer).
    fn detach(&self) -> Self {
        Self {
            cli: self.cli,
            released: Cell::new(false),
            key: self.key.clone(),
            keep: self.keep,
            reuse_count: self.reuse_count,
            frame: Cell::new(self.frame.take()),
            ssl_wait: Cell::new(self.ssl_wait.get()),
        }
    }

    /// PHP code a call reached released the connection: errCode says it is
    /// not connected, as for a closed client.
    fn released(&self, object: Obj) -> bool {
        if self.released.get() {
            set_last_error(NO_CONNECTION);
            set_err_code(object, NO_CONNECTION);
        }
        self.released.get()
    }

    pub(crate) fn flag(&self, name: u32) -> bool {
        self.cli.get().flag(name)
    }

    pub(crate) fn set_flag(&self, name: u32, value: bool) {
        self.cli.get().set(name, value);
    }

    pub(crate) fn has_socket(&self) -> bool {
        self.cli.net().is_some()
    }

    /// `cli->socket->check_liveness()` of a client that has a socket.
    fn alive(&self) -> bool {
        self.cli
            .net()
            .is_some_and(swoole_rs_tls::upstream::check_liveness)
    }

    pub(crate) fn fd(&self) -> c_int {
        self.cli.net().map_or(-1, Net::fd)
    }

    fn socket_type(&self) -> i64 {
        self.cli
            .net()
            .map_or(-1, |net| i64::from(net.read(|record| record.socket_type)))
    }

    pub(crate) fn is_stream(&self) -> bool {
        matches!(self.socket_type(), SOCK_TCP | SOCK_TCP6 | SOCK_UNIX_STREAM)
    }

    pub(crate) fn has_ssl(&self) -> bool {
        self.cli
            .net()
            .is_some_and(|net| net.read(|record| !record.ssl().is_null()))
    }
}

impl Drop for Conn {
    /// A released connection's last holder destroys the client; the handler
    /// or event that dropped it raises a bailout as its last step.
    fn drop(&mut self) {
        if self.released.get() {
            bailout::defer(native::destroy(self.cli));
        }
    }
}

/// `ClientObject` without its `zend_object`; `Swoole\Async\Client` shares it.
#[repr(C)]
///
/// Cells, so PHP code a method runs can re-enter the same object's methods
/// while it holds the data: nothing borrows it mutably.
pub(crate) struct Data {
    /// The connection; all-zero is none.
    conn: Cell<Option<Rc<Conn>>>,
    /// `getSocket()`'s socket (`IS_UNDEF`: none); zeroed bytes are `IS_UNDEF`.
    zsocket: Cell<Owned>,
    /// The async callbacks, by [`crate::async_net`] slot; all-zero is none.
    pub(crate) callbacks: [Cell<Option<Rc<Callable>>>; 7],
    /// `on()` ran: the async client is initialised.
    pub(crate) listening: Cell<bool>,
}

impl Data {
    /// The object's connection, if it holds one.
    pub(crate) fn conn(&self) -> Option<Rc<Conn>> {
        let held = self.conn.take();
        self.conn.set(held.clone());
        held
    }

    pub(crate) fn has_conn(&self) -> bool {
        let held = self.conn.take();
        let has = held.is_some();
        self.conn.set(held);
        has
    }

    pub(crate) fn set_conn(&self, conn: Option<Rc<Conn>>) {
        drop(self.conn.replace(conn));
    }

    /// A new reference to `getSocket()`'s socket, if it has one.
    fn zsocket(&self) -> Option<Owned> {
        let held = self.zsocket.take();
        let copy = (held.kind() != IS_UNDEF).then(|| held.copy());
        self.zsocket.set(held);
        copy
    }
}

/// A `Swoole\Client` (or `Swoole\Async\Client`, which shares its layout)
/// object, live for `'a`.
#[derive(Clone, Copy)]
pub(crate) struct Obj<'a>(*mut zend_object, PhantomData<&'a zend_object>);

impl<'a> Obj<'a> {
    /// # Safety
    /// `object` is a live client or async client object for `'a`.
    pub(crate) unsafe fn new(object: *mut zend_object) -> Self {
        Self(object, PhantomData)
    }

    /// `$this` of a client method.
    pub(crate) fn this(call: &'a Call) -> Self {
        // SAFETY: client methods are non-static, so `$this` is a client object
        // (or an async one), live for the call.
        unsafe { Self::new(call.this()) }
    }

    pub(crate) fn as_ptr(self) -> *mut zend_object {
        self.0
    }

    pub(crate) fn data(self) -> &'a Data {
        // SAFETY: a live client object's data, only ever shared (its fields are cells).
        unsafe { &*ClientObject::data_ptr(self.0) }
    }

    /// Property writes, under the object's own class.
    pub(crate) fn props(self) -> Props<'a> {
        // SAFETY: a live client object; the client class declares every name written.
        unsafe { Props::new(CLIENT.entry(), self.0) }.own()
    }

    /// `ZSTR_VAL(Z_OBJCE_P(object)->name)`.
    pub(crate) fn class_name(self) -> &'a CStr {
        // SAFETY: a live object; its class outlives it.
        unsafe { swoole_rs_zend::class_name(self.0) }
    }

    /// The declared property `name`.
    fn property(self, name: &CStr) -> &'a Value {
        // SAFETY: a live client object; the client class declares `name`.
        unsafe { swoole_rs_zend::property(self.0, name) }
    }
}

pub(crate) type ClientObject = Object<Data>;

pub(crate) static CLIENT: Class = Class::new();
static EXCEPTION: Class = Class::new();
/// A persistent connection the pool owns until a client takes it back.
struct Pooled(Box<Conn>);
// SAFETY: the pool only moves the connection, whose client is a raw pointer;
// whichever request takes it back owns it, as with upstream's process-wide
// `long_connections`.
unsafe impl Send for Pooled {}

/// The persistent connections (`long_connections`), keyed by server id.
static POOL: Mutex<Option<Pool<Pooled>>> = Mutex::new(None);

fn long_connections() -> MutexGuard<'static, Option<Pool<Pooled>>> {
    POOL.lock().unwrap_or_else(PoisonError::into_inner)
}

unsafe extern "C" fn create(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: Zend creates this class or a subclass; zeroed data is no
    // connection, no socket and no callbacks.
    unsafe { ClientObject::create(&CLIENT, ce) }
}

unsafe extern "C" fn free(std: *mut zend_object) {
    // SAFETY: Zend frees a client object.
    let object = unsafe { Obj::new(std) };
    crate::async_net::free_callbacks(object);
    // SAFETY: as above.
    unsafe { ClientObject::dtor(std) };
    bailout::finish();
}

fn configure(entry: &mut zend_class_entry, handlers: &mut zend_object_handlers) {
    entry.ce_flags |= ZEND_ACC_NOT_SERIALIZABLE;
    handlers.clone_obj = None;
    handlers.unset_property = Some(unset_property_deny);
    ClientObject::install(entry, handlers, create, free);
}

method_table! {
    static METHODS = [
        c"__construct" => zim_swoole_client___construct, &client::arginfo_class_Swoole_Client___construct, z::ZEND_ACC_PUBLIC;
        c"__destruct" => zim_swoole_client___destruct, &client::arginfo_class_Swoole_Client___destruct, z::ZEND_ACC_PUBLIC;
        c"set" => zim_swoole_client_set, &client::arginfo_class_Swoole_Client_set, z::ZEND_ACC_PUBLIC;
        c"connect" => zim_swoole_client_connect, &client::arginfo_class_Swoole_Client_connect, z::ZEND_ACC_PUBLIC;
        c"recv" => zim_swoole_client_recv, &client::arginfo_class_Swoole_Client_recv, z::ZEND_ACC_PUBLIC;
        c"send" => zim_swoole_client_send, &client::arginfo_class_Swoole_Client_send, z::ZEND_ACC_PUBLIC;
        c"sendfile" => zim_swoole_client_sendfile, &client::arginfo_class_Swoole_Client_sendfile, z::ZEND_ACC_PUBLIC;
        c"sendto" => zim_swoole_client_sendto, &client::arginfo_class_Swoole_Client_sendto, z::ZEND_ACC_PUBLIC;
        c"shutdown" => zim_swoole_client_shutdown, &client::arginfo_class_Swoole_Client_shutdown, z::ZEND_ACC_PUBLIC;
        c"enableSSL" => zim_swoole_client_enableSSL, &client::arginfo_class_Swoole_Client_enableSSL, z::ZEND_ACC_PUBLIC;
        c"getPeerCert" => zim_swoole_client_getPeerCert, &client::arginfo_class_Swoole_Client_getPeerCert, z::ZEND_ACC_PUBLIC;
        c"verifyPeerCert" => zim_swoole_client_verifyPeerCert, &client::arginfo_class_Swoole_Client_verifyPeerCert, z::ZEND_ACC_PUBLIC;
        c"isConnected" => zim_swoole_client_isConnected, &client::arginfo_class_Swoole_Client_isConnected, z::ZEND_ACC_PUBLIC;
        c"getsockname" => zim_swoole_client_getsockname, &client::arginfo_class_Swoole_Client_getsockname, z::ZEND_ACC_PUBLIC;
        c"getpeername" => zim_swoole_client_getpeername, &client::arginfo_class_Swoole_Client_getpeername, z::ZEND_ACC_PUBLIC;
        c"close" => zim_swoole_client_close, &client::arginfo_class_Swoole_Client_close, z::ZEND_ACC_PUBLIC;
        // SWOOLE_SOCKETS_SUPPORT builds only; MINIT drops it from the others.
        c"getSocket" => zim_swoole_client_getSocket, &client::arginfo_class_Swoole_Client_getSocket, z::ZEND_ACC_PUBLIC;
    ];
}

/// `swoole_client_ce`: `Swoole\Client`'s class entry, null before MINIT.
#[must_use]
pub fn client_class() -> *mut zend_class_entry {
    CLIENT.entry()
}

/// `Swoole\Client`.
#[must_use]
pub fn client() -> &'static Class {
    &CLIENT
}

swoole_rs_zend::minit!(php_swoole_client_minit => register);

/// `php_swoole_client_minit`, after `Swoole\Exception` is registered.
#[expect(clippy::unnecessary_wraps, reason = "the shape `minit!` takes")]
fn register(minit: &Minit) -> Result<(), Bailout> {
    let methods = if built_with(Switch::Sockets) {
        &METHODS.0
    } else {
        METHODS.without(&[c"getSocket"])
    };
    minit.register(&CLIENT, c"Swoole\\Client", methods, configure);
    minit.register_child(
        &EXCEPTION,
        c"Swoole\\Client\\Exception",
        &[],
        minit.parent(swoole_rs_phpswoole::exception()),
        |_, _| {},
    );
    minit.property_long(&CLIENT, c"errCode", 0, ZEND_ACC_PUBLIC);
    minit.property_long(&CLIENT, c"sock", -1, ZEND_ACC_PUBLIC);
    minit.property_bool(&CLIENT, c"reuse", false, ZEND_ACC_PUBLIC);
    minit.property_long(&CLIENT, c"reuseCount", 0, ZEND_ACC_PUBLIC);
    minit.property_long(&CLIENT, c"type", 0, ZEND_ACC_PUBLIC);
    minit.property_null(&CLIENT, c"id", ZEND_ACC_PUBLIC);
    minit.property_null(&CLIENT, c"setting", ZEND_ACC_PUBLIC);
    for (name, value) in [
        (c"MSG_OOB", libc::MSG_OOB),
        (c"MSG_PEEK", libc::MSG_PEEK),
        (c"MSG_DONTWAIT", libc::MSG_DONTWAIT),
        (c"MSG_WAITALL", libc::MSG_WAITALL),
        (c"SHUT_RDWR", libc::SHUT_RDWR),
        (c"SHUT_RD", libc::SHUT_RD),
        (c"SHUT_WR", libc::SHUT_WR),
    ] {
        minit.class_constant_long(&CLIENT, name, i64::from(value));
    }
    for (name, value) in [
        (c"SWOOLE_SYNC", FLAG_SYNC),
        (c"SWOOLE_ASYNC", FLAG_ASYNC),
        (c"SWOOLE_KEEP", FLAG_KEEP),
    ] {
        minit.long_constant(name, value);
    }
    Ok(())
}

pub(crate) fn this(call: &Call) -> &Data {
    Obj::this(call).data()
}

/// The object's connection.
pub(crate) fn conn(call: &Call) -> Option<Rc<Conn>> {
    this(call).conn()
}

/// `php_swoole_client_get_type`: the socket type without the client flags.
pub(crate) fn socket_type(kind: i64) -> i64 {
    kind & !(FLAG_SYNC | FLAG_ASYNC | FLAG_KEEP | SOCK_SSL)
}

/// `php_swoole_error` and friends (`kind`) at `level`, deferring a bailout.
pub(crate) fn report(kind: ErrorKind, level: c_int, message: &str) {
    let message = CString::new(message).unwrap_or_default();
    let kind = match kind {
        ErrorKind::Error => Report::Swoole,
        ErrorKind::Fatal => Report::Fatal,
        // `php_swoole_core_error`.
        ErrorKind::Core if display_errors() || level == E_ERROR => Report::Docref,
        ErrorKind::Core => return,
    };
    bailout::defer(swoole_rs_phpruntime::report(kind, level, &message));
}

pub(crate) fn warn(kind: ErrorKind, message: &str) {
    report(kind, E_WARNING, message);
}

/// `php_swoole_sys_error(E_WARNING, ...)` of the failure that set `error`.
pub(crate) fn warn_sys(message: &str, error: c_int) {
    set_last_error(error);
    if !display_errors() {
        return;
    }
    let message = format!(
        "{message}, Error: {}[{error}]",
        swoole_rs_phpruntime::strerror(error).to_string_lossy()
    );
    let message = CString::new(message).unwrap_or_default();
    bailout::defer(swoole_rs_phpruntime::report(
        Report::Docref,
        E_WARNING,
        &message,
    ));
}

/// `swoole_set_last_error(code)`.
pub(crate) fn set_last_error(code: c_int) {
    swoole_rs_corebase::log::set_last_error(code);
}

pub(crate) fn last_error() -> c_int {
    swoole_rs_corebase::log::last_error()
}

/// `errCode = code`.
pub(crate) fn set_err_code(object: Obj, code: c_int) {
    object.props().long(c"errCode", i64::from(code));
}

pub(crate) fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// `swoole_strerror(code)`.
pub(crate) fn strerror(code: c_int) -> String {
    swoole_rs_phpruntime::strerror(code)
        .to_string_lossy()
        .into_owned()
}

/// Defer a bailout and say whether there was one.
pub(crate) fn bailed<T>(result: Result<T, Bailout>) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(bailout) => {
            bailout.defer();
            None
        }
    }
}

/// `php_swoole_client_free`: release the connection's timer, length callable,
/// pool entry and client, then the exported socket. The client is destroyed
/// now unless a method that re-entered PHP still holds the connection; then
/// its last drop does it.
pub(crate) fn release(object: Obj, conn: Rc<Conn>) -> Result<(), Bailout> {
    if conn.released.get() {
        return Ok(());
    }
    if conn.keep
        && let Some(pool) = long_connections().as_mut()
    {
        pool.remove_empty(&conn.key);
    }
    let data = object.data();
    data.set_conn(None);
    let destroyed = match Rc::try_unwrap(conn) {
        Ok(conn) => native::destroy(conn.cli),
        Err(held) => {
            held.released.set(true);
            Ok(())
        }
    };
    // The reference the object took in getSocket().
    drop(data.zsocket.take());
    destroyed
}

/// `php_swoole_client_get_cli_safe`: the connected client; else errCode and a
/// warning say it is not connected.
pub(crate) fn connected(call: &Call) -> Option<Rc<Conn>> {
    let object = Obj::this(call);
    if let Some(conn) = conn(call)
        && conn.has_socket()
    {
        if conn.flag(state::ACTIVE) {
            return Some(conn);
        }
        if conn.flag(state::ASYNC_CONNECT) {
            conn.set_flag(state::ASYNC_CONNECT, false);
            let mut error: c_int = -1;
            #[expect(clippy::cast_possible_truncation, reason = "sizeof(int)")]
            let mut length = size_of::<c_int>() as libc::socklen_t;
            // SAFETY: the client's socket; writable locals of the sizes given.
            if unsafe {
                libc::getsockopt(
                    conn.fd(),
                    libc::SOL_SOCKET,
                    libc::SO_ERROR,
                    (&raw mut error).cast(),
                    &raw mut length,
                )
            } == 0
                && error == 0
            {
                conn.set_flag(state::ACTIVE, true);
                return Some(conn);
            }
            bailed(release(object, conn))?;
        }
    }
    set_last_error(NO_CONNECTION);
    set_err_code(object, last_error());
    warn(ErrorKind::Error, "client is not connected to server");
    None
}

/// `__construct`'s type check: the socket type, or a `TypeError` like upstream's.
pub(crate) fn check_type(kind: i64) -> bool {
    if (SOCK_TCP..=SOCK_UNIX_DGRAM).contains(&socket_type(kind)) {
        return true;
    }
    let mut space: *const c_char = ptr::null();
    // SAFETY: inside a method call; Zend returns static names.
    let class = unsafe { get_active_class_name(&raw mut space) };
    // SAFETY: as above.
    let class = unsafe { CStr::from_ptr(class) };
    // SAFETY: as above.
    let space = unsafe { CStr::from_ptr(space) };
    // SAFETY: as above.
    let function = unsafe { get_active_function_name() };
    // SAFETY: as above.
    let function = unsafe { CStr::from_ptr(function) };
    let message = format!(
        "{}{}{}() expects parameter 1 to be client type, unknown type {kind} given",
        class.to_string_lossy(),
        space.to_string_lossy(),
        function.to_string_lossy(),
    );
    let message = CString::new(message).unwrap_or_default();
    // SAFETY: "%s" takes the NUL-terminated message.
    unsafe { zend_type_error(c"%s".as_ptr(), message.as_ptr()) };
    false
}

/// The object's `type` property, or None after upstream's fatal error.
pub(crate) fn type_property(object: Obj, async_message: bool) -> Option<i64> {
    let ztype = object.property(c"type");
    if ztype.is_null() {
        let message = if async_message {
            "failed to get client type"
        } else {
            "failed to get swoole_client->type"
        };
        report(ErrorKind::Fatal, E_ERROR, message);
        return None;
    }
    Some(ztype.long())
}

/// The `setting` property when it is an array.
pub(crate) fn settings(object: Obj) -> Option<*mut zval> {
    let zset = object.property(c"setting");
    (zset.kind() == IS_ARRAY).then(|| zset.as_ptr())
}

/// `php_swoole_client_new`: a pooled or new connection for `host:port`.
fn connection(object: Obj, host: &[u8], port: i64) -> Option<Rc<Conn>> {
    let kind = type_property(object, false)?;
    #[expect(clippy::cast_possible_truncation, reason = "upstream passes an int")]
    let int_port = port as i32;
    if matches!(socket_type(kind), SOCK_TCP | SOCK_TCP6) && swr_v1_clients_port(int_port, 0) == 0 {
        warn(ErrorKind::Fatal, "The port is invalid");
        set_last_error(INVALID_PARAMS);
        return None;
    }
    let id = object.property(c"id").str().map(ZStr::bytes);
    let key = match id {
        Some(id) if !id.is_empty() => id.to_vec(),
        _ => {
            let host = host.split(|&b| b == 0).next().unwrap_or_default();
            let mut key = host.to_vec();
            key.extend_from_slice(format!(":{int_port}").as_bytes());
            key
        }
    };
    let mut conn = None;
    if kind & FLAG_KEEP != 0 {
        let taken = long_connections().as_mut().and_then(|pool| pool.take(&key));
        if let Some(Pooled(mut pooled)) = taken {
            if pooled.alive() {
                pooled.reuse_count += 1;
                object
                    .props()
                    .long(c"reuseCount", i64::from(pooled.reuse_count));
                conn = Some(pooled);
            } else {
                bailed(pooled.cli.run(Op::Close))?;
                bailed(release(object, Rc::new(*pooled)))?;
            }
        }
    }
    let mut connection = if let Some(conn) = conn {
        *conn
    } else {
        #[expect(clippy::cast_possible_truncation, reason = "a socket type")]
        let cli = match native::create(socket_type(kind) as c_int, false) {
            Ok(cli) => cli,
            Err(error) => {
                warn_sys("Client_create() failed", error);
                set_err_code(object, error);
                return None;
            }
        };
        let mut fresh = Conn::new(cli);
        fresh.key = key;
        fresh
    };
    object.props().long(c"sock", i64::from(connection.fd()));
    if kind & FLAG_KEEP != 0 {
        connection.keep = true;
    }
    if kind & SOCK_SSL != 0 {
        bailed(connection.cli.run(Op::EnableSsl))?;
    }
    Some(Rc::new(connection))
}

/// `__construct(int $type, bool $async = false, string $id = '')`.
fn construct(call: &Call) {
    let parsed = call.arity(1, 3).and_then(|()| {
        let kind = call.long(1, 0)?;
        let is_async = call.bool(2, false)?;
        let id = if call.num_args() >= 3 {
            Some(call.string(3, b"")?)
        } else {
            None
        };
        Ok((kind, is_async, id))
    });
    let Ok((kind, is_async, id)) = parsed else {
        call.throw_error(c"socket type param is required");
        return call.set_bool(false);
    };
    if is_async {
        call.throw_error(c"The $async parameter is not supported");
        return call.set_bool(false);
    }
    if !check_type(kind) {
        return call.set_bool(false);
    }
    let props = Obj::this(call).props();
    props.long(c"type", kind);
    if let Some(id) = id {
        props.bytes(c"id", id);
    }
    call.set_bool(true);
}

/// `SW_PREVENT_USER_DESTRUCT`: false (returning null) when user code called it.
pub(crate) fn destructing(call: &Call) -> bool {
    if !call.destructor_called() {
        call.set_null();
        return false;
    }
    true
}

/// `__destruct()`: close a held connection through `Swoole\Client::close`.
fn destruct(call: &Call) {
    if !destructing(call) || !this(call).has_conn() {
        return;
    }
    // SAFETY: a live client object; `Swoole\Client::close` with no arguments.
    bailed(unsafe {
        bailout::call_method(call.this(), CLIENT.entry(), b"close", ptr::null_mut(), &[])
    });
}

/// `set(array $settings): bool`.
fn set(call: &Call) {
    if call.arity(1, 1).is_err() {
        return call.set_bool(false);
    }
    let Some(settings) = call.value(1).filter(|settings| settings.array().is_some()) else {
        return call.set_bool(false);
    };
    merge_setting(call, &CLIENT, settings);
    call.set_bool(true);
}

/// `set()`'s `php_array_merge` of `settings` into `$this->setting` (made an
/// array first, under `scope`); nothing when `settings` is no array.
pub(crate) fn merge_setting(call: &Call, scope: &Class, settings: &Value) {
    if let Some(props) = call.props(Some(scope)) {
        props.merge(c"setting", settings);
    }
}

/// `connect(string $host, int $port = 0, float $timeout = 0.5, int $sock_flag = 0): bool`.
fn connect(call: &Call) {
    let parsed = call.arity(1, 4).and_then(|()| {
        Ok((
            call.string(1, b"")?,
            call.long(2, 0)?,
            call.double(3, CONNECT_TIMEOUT)?,
            call.long(4, 0)?,
        ))
    });
    let Ok((host, port, timeout, flags)) = parsed else {
        return call.set_bool(false);
    };
    if host.is_empty() {
        warn(ErrorKind::Fatal, "The host is empty");
        return call.set_bool(false);
    }
    let object = Obj::this(call);
    if this(call).has_conn() {
        warn(
            ErrorKind::Fatal,
            "connection to the server has already been established",
        );
        return call.set_bool(false);
    }
    let Some(conn) = connection(object, host, port) else {
        return call.set_bool(false);
    };
    this(call).set_conn(Some(conn.clone()));
    if conn.keep && conn.flag(state::ACTIVE) {
        object.props().bool(c"reuse", true);
        return call.set_bool(true);
    } else if conn.flag(state::ACTIVE) {
        warn(
            ErrorKind::Fatal,
            "connection to the server has already been established",
        );
        return call.set_bool(false);
    }
    if let Some(zset) = settings(object) {
        set_last_error(0);
        let Some(applied) = crate::settings::apply(conn.cli, zset) else {
            return call.set_bool(false);
        };
        if !applied {
            set_err_code(object, last_error());
            return call.set_bool(false);
        }
    }
    let connect = Op::Connect {
        host: &native::c_text(host),
        port: int(port),
        timeout,
        flags: int(flags),
    };
    let Some(result) = bailed(conn.cli.run(connect)) else {
        return call.set_bool(false);
    };
    if conn.released(object) {
        return call.set_bool(false);
    }
    if result < 0 {
        set_err_code(object, last_error());
        if conn.flag(state::ASYNC_CONNECT) {
            return call.set_bool(true);
        }
        let error = last_error();
        let host = host.split(|&b| b == 0).next().unwrap_or_default();
        #[expect(
            clippy::cast_possible_truncation,
            reason = "upstream prints (int) port"
        )]
        let message = format!(
            "connect to server[{}:{}] failed. Error: {}[{error}]",
            String::from_utf8_lossy(host),
            port as i32,
            strerror(error),
        );
        warn(ErrorKind::Core, &message);
        bailed(release(object, conn));
        return call.set_bool(false);
    }
    call.set_bool(true);
}

/// `send(string $data, int $flag = 0): int|false`.
fn send(call: &Call) {
    let parsed = call
        .arity(1, 2)
        .and_then(|()| Ok((call.string(1, b"")?, call.long(2, 0)?)));
    let Ok((data, flags)) = parsed else {
        return call.set_bool(false);
    };
    if data.is_empty() {
        warn(ErrorKind::Fatal, "data to send is empty");
        return call.set_bool(false);
    }
    let Some(conn) = connected(call) else {
        return call.set_bool(false);
    };
    set_last_error(0);
    let send = Op::Send {
        data,
        flags: int(flags),
    };
    let Some(sent) = bailed(io(conn.cli, send)) else {
        return call.set_bool(false);
    };
    if sent < 0 {
        let error = errno();
        let message = format!("failed to send({}) {} bytes", conn.fd(), data.len());
        warn_sys(&message, error);
        set_err_code(Obj::this(call), last_error());
        return call.set_bool(false);
    }
    call.set_long(sent);
}

/// `sendto(string $ip, int $port, string $data): bool`.
fn sendto(call: &Call) {
    let parsed = call
        .arity(3, 3)
        .and_then(|()| Ok((call.string(1, b"")?, call.long(2, 0)?, call.string(3, b"")?)));
    let Ok((host, port, data)) = parsed else {
        return call.set_bool(false);
    };
    if data.is_empty() {
        warn(ErrorKind::Error, "data to send is empty");
        return call.set_bool(false);
    }
    let object = Obj::this(call);
    if !this(call).has_conn() {
        let Some(conn) = connection(object, host, port) else {
            return call.set_bool(false);
        };
        conn.set_flag(state::ACTIVE, true);
        this(call).set_conn(Some(conn));
    }
    let Some(conn) = conn(call) else {
        return call.set_bool(false);
    };
    let send = Op::Sendto {
        host: &native::c_text(host),
        port: int(port),
        data,
    };
    let Some(result) = bailed(io(conn.cli, send)) else {
        return call.set_bool(false);
    };
    if result < 0 {
        set_err_code(object, last_error());
    }
    call.set_bool(result >= 0);
}

/// `sendfile(string $filename, int $offset = 0, int $length = 0): bool`.
fn sendfile(call: &Call) {
    let parsed = call
        .arity(1, 3)
        .and_then(|()| Ok((call.string(1, b"")?, call.long(2, 0)?, call.long(3, 0)?)));
    let Ok((file, offset, length)) = parsed else {
        return call.set_bool(false);
    };
    if file.is_empty() {
        warn(ErrorKind::Fatal, "file to send is empty");
        return call.set_bool(false);
    }
    let Some(conn) = connected(call) else {
        return call.set_bool(false);
    };
    if !conn.is_stream() {
        warn(ErrorKind::Error, "dgram socket cannot use sendfile");
        return call.set_bool(false);
    }
    set_last_error(0);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "C++'s (size_t) length"
    )]
    let send = Op::Sendfile {
        file: &native::c_text(file),
        offset,
        length: length as usize,
    };
    let Some(result) = bailed(io(conn.cli, send)) else {
        return call.set_bool(false);
    };
    if result < 0 {
        set_last_error(errno());
        let error = last_error();
        let message = format!("sendfile() failed. Error: {} [{error}]", strerror(error));
        warn(ErrorKind::Fatal, &message);
        set_err_code(Obj::this(call), last_error());
        return call.set_bool(false);
    }
    call.set_bool(true);
}

/// Run an IO `op`: a sync client raises no callbacks, so only an async one
/// needs the `zend_try` frame.
fn io(cli: Cli, op: Op<'_>) -> Result<i64, Bailout> {
    if cli.get().flag(state::ASYNC) {
        cli.run(op)
    } else {
        Ok(cli.run_plain(op))
    }
}

/// `cli->recv(buffer, length, flags)`.
fn receive(cli: Cli, buffer: &mut [u8], flags: i64) -> Result<i64, Bailout> {
    io(
        cli,
        Op::Recv {
            buffer,
            flags: int(flags),
        },
    )
}

/// C++'s implicit conversion of a PHP int to an `int` argument.
#[expect(clippy::cast_possible_truncation, reason = "C++'s (int) conversion")]
pub(crate) fn int(value: i64) -> c_int {
    value as c_int
}

/// [`receive`] of at most `length` bytes into `buffer`'s room.
fn receive_into(conn: &Conn, buffer: &mut ZBuf, length: usize, flags: i64) -> Result<i64, Bailout> {
    let room = native::room(buffer);
    let length = room.len().min(length);
    let read = receive(conn.cli, room.get_mut(..length).unwrap_or_default(), flags)?;
    Ok(native::filled(buffer, read))
}

/// `recv()` failed (`error`): warn, errCode and false.
fn recv_failed(call: &Call, error: c_int) {
    warn_sys("recv() failed", error);
    set_err_code(Obj::this(call), last_error());
    call.set_bool(false);
}

/// `recv` under `open_eof_check`: one package up to and including the EOF,
/// keeping what follows for the next call.
fn recv_eof(call: &Call, conn: &Conn, frame: &mut Frame, framing: &Protocol) {
    let eof = &framing.package_eof;
    let eof = eof
        .get(..usize::from(framing.package_eof_len))
        .unwrap_or(eof);
    let max = framing.package_max_length as usize;
    if frame.size == 0 {
        frame.size = BUFFER_SIZE_BIG;
        frame.buffer.clear();
    }
    let mut find = !frame.buffer.is_empty();
    loop {
        if !find {
            let length = frame.buffer.len();
            let room = (frame.size - length.min(frame.size)).min(BUFFER_SIZE_BIG);
            frame.buffer.resize(length + room, 0);
            let target = frame.buffer.get_mut(length..).unwrap_or_default();
            let read = receive(conn.cli, target, 0);
            let Some(read) = bailed(read) else {
                frame.buffer.truncate(length);
                return call.set_bool(false);
            };
            if read < 0 {
                let error = errno();
                frame.buffer.clear();
                return recv_failed(call, error);
            } else if read == 0 {
                frame.buffer.clear();
                return call.set_empty_string();
            }
            frame
                .buffer
                .truncate(length + usize::try_from(read).unwrap_or(0));
            if frame.buffer.len() < eof.len() {
                continue;
            }
        }
        find = false;
        if let Some(at) = position(&frame.buffer, eof) {
            let end = at + eof.len();
            let package = ZString::new(frame.buffer.get(..end).unwrap_or_default());
            if frame.buffer.len() > end {
                frame.buffer.drain(..end);
                frame.size = BUFFER_SIZE_BIG;
            } else {
                frame.buffer.clear();
                frame.size = 0;
            }
            return call.set_zstring(package);
        }
        if frame.buffer.len() == max {
            warn(ErrorKind::Error, "no package eof");
            frame.buffer.clear();
            return call.set_bool(false);
        } else if frame.buffer.len() == frame.size && frame.size < max {
            frame.size = (frame.size * 2).min(max);
        }
    }
}

/// `swoole_strnpos`: the first `needle` in `haystack`.
fn position(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// `recv` under `open_length_check`: the header, then the rest of the package.
/// Returns the string and its length so far, or None when it has returned.
fn recv_length(
    call: &Call,
    conn: &Conn,
    frame: &mut Frame,
    framing: &Protocol,
    mut length: i64,
) -> Option<(ZBuf, i64)> {
    if frame.size == 0 {
        frame.size = BUFFER_SIZE_STD;
    }
    frame.buffer.clear();
    let header =
        usize::from(framing.package_length_offset) + usize::from(framing.package_length_size);
    loop {
        let have = frame.buffer.len();
        let want = header.saturating_sub(have);
        frame.buffer.resize(have + want, 0);
        let target = frame.buffer.get_mut(have..).unwrap_or_default();
        let read = receive(conn.cli, target, 0);
        let Some(read) = bailed(read) else {
            call.set_bool(false);
            return None;
        };
        if read <= 0 {
            frame.buffer.truncate(have);
            break;
        }
        frame
            .buffer
            .truncate(have + usize::try_from(read).unwrap_or(0));
        let Some(measured) = bailed(conn.cli.run(Op::PackageLength(&frame.buffer))) else {
            call.set_bool(false);
            return None;
        };
        if conn.released(Obj::this(call)) {
            call.set_bool(false);
            return None;
        }
        length = measured;
        if length != 0 {
            break;
        }
    }
    let buffered = frame.buffer.len();
    #[expect(clippy::cast_possible_wrap, reason = "lengths below 4 GiB")]
    let (header_length, have) = (header as i64, buffered as i64);
    if length < 0 {
        call.set_empty_string();
        return None;
    } else if length == header_length {
        frame.buffer.resize(header.max(buffered), 0);
        call.set_zstring(ZString::new(frame.buffer.get(..header).unwrap_or_default()));
        return None;
    } else if length > i64::from(framing.package_max_length) {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "upstream prints (int) buf_len"
        )]
        native::package_too_big(length as c_int);
        call.set_empty_string();
        return None;
    } else if length == have {
        call.set_zstring(ZString::new(&frame.buffer));
        return None;
    } else if length < have {
        let end = usize::try_from(length).unwrap_or(0);
        call.set_zstring(ZString::new(frame.buffer.get(..end).unwrap_or_default()));
        frame.buffer.drain(..end);
        return None;
    }
    let total = usize::try_from(length).unwrap_or(0);
    let mut package = ZBuf::with_capacity(total.max(header));
    package.extend_from_slice(&frame.buffer);
    set_last_error(0);
    let rest = total.saturating_sub(buffered);
    let read = receive_into(conn, &mut package, rest, i64::from(libc::MSG_WAITALL));
    let Some(mut read) = bailed(read) else {
        call.set_bool(false);
        return None;
    };
    if read > 0 {
        read += header_length;
        if read != length {
            read = 0;
        }
    }
    Some((package, read))
}

/// `recv(int $size = 65535, int $flag = 0): string|false`.
fn recv(call: &Call) {
    let parsed = call
        .arity(0, 2)
        .and_then(|()| Ok((call.long(1, PHP_BUFFER_SIZE)?, call.long(2, 0)?)));
    let Ok((mut length, mut flags)) = parsed else {
        return call.set_bool(false);
    };
    if flags == 1 {
        flags = i64::from(libc::MSG_WAITALL);
    }
    let Some(conn) = connected(call) else {
        return call.set_bool(false);
    };
    let (eof_check, length_check, framing) = {
        let record = conn.cli.record();
        (
            record.open_eof_check,
            record.open_length_check,
            record.protocol,
        )
    };
    let (package, read) = if eof_check {
        let mut frame = conn.frame.take();
        recv_eof(call, &conn, &mut frame, &framing);
        return conn.frame.set(frame);
    } else if length_check {
        let mut frame = conn.frame.take();
        let received = recv_length(call, &conn, &mut frame, &framing, length);
        conn.frame.set(frame);
        let Some(received) = received else {
            return;
        };
        received
    } else {
        if flags & i64::from(libc::MSG_WAITALL) == 0 && length > PHP_BUFFER_SIZE {
            length = PHP_BUFFER_SIZE;
        }
        // A negative size reads nothing.
        let size = usize::try_from(length).unwrap_or(0);
        let mut package = ZBuf::with_capacity(size);
        set_last_error(0);
        let Some(read) = bailed(receive_into(&conn, &mut package, size, flags)) else {
            return call.set_bool(false);
        };
        (package, read)
    };
    if read <= 0 {
        let error = errno();
        if read < 0 {
            return recv_failed(call, error);
        }
        return call.set_empty_string();
    }
    call.set_zstring(package.into_zstring());
}

/// `isConnected(): bool`.
pub(crate) fn is_connected(call: &Call) {
    let Some(conn) = conn(call) else {
        return call.set_bool(false);
    };
    if !conn.has_socket() {
        return call.set_bool(false);
    }
    call.set_bool(conn.flag(state::ACTIVE));
}

/// `getsockname(): array|false`.
fn getsockname(call: &Call) {
    let Some(conn) = connected(call) else {
        return call.set_bool(false);
    };
    let Some(socket) = conn.cli.net() else {
        return call.set_bool(false);
    };
    if socket.edit(SocketFields::get_name) < 0 {
        let error = errno();
        warn_sys("getsockname() failed", error);
        set_err_code(Obj::this(call), error);
        return call.set_bool(false);
    }
    let (host, port) = native::net_address(socket);
    address_array(call, port, &host);
}

/// `['port' => $port, 'host' => $host]`.
fn address_array(call: &Call, port: c_int, host: &CStr) {
    let mut array = OwnedArray::default();
    array.insert_long(b"port", i64::from(port));
    array.insert_bytes(b"host", Some(host.to_bytes()));
    call.set_owned(array.into_owned());
}

/// `getpeername(): array|false`.
fn getpeername(call: &Call) {
    let Some(conn) = connected(call) else {
        return call.set_bool(false);
    };
    let mut address = SocketAddress::default();
    if conn.cli.handle().peer_name(&mut address) < 0 {
        let error = errno();
        warn_sys("getpeername() failed", error);
        set_err_code(Obj::this(call), error);
        return call.set_bool(false);
    }
    let (host, port) = native::address(&address);
    address_array(call, port, host);
}

/// `getSocket()`: the client's socket as an ext/sockets `Socket`.
fn get_socket(call: &Call) {
    if let Some(zsocket) = this(call).zsocket() {
        // `RETURN_ZVAL(zsocket, 1, 0)`: a new reference for the return.
        return call.set_owned(zsocket);
    }
    let Some(conn) = connected(call) else {
        return call.set_bool(false);
    };
    if conn.keep {
        warn(
            ErrorKind::Fatal,
            "the 'getSocket' method can't be used on persistent connection",
        );
        return call.set_bool(false);
    }
    let Some(socket) = native::export_socket(conn.cli) else {
        return call.set_bool(false);
    };
    // The object keeps a second reference, as upstream's dup does.
    call.set_owned(socket.copy());
    this(call).zsocket.set(socket);
}

/// `close(bool $force = false): bool`.
fn close(call: &Call) {
    let Ok(force) = call.arity(0, 1).and_then(|()| call.bool(1, false)) else {
        return call.set_bool(false);
    };
    let object = Obj::this(call);
    let Some(conn) = this(call).conn().filter(|conn| conn.has_socket()) else {
        warn(ErrorKind::Fatal, "client is not connected to the server");
        return call.set_bool(false);
    };
    if conn.flag(state::CLOSED) {
        warn(ErrorKind::Error, "client socket is closed");
        return call.set_bool(false);
    }
    // Only a live connection with no shutdown direction, which no method that
    // re-entered PHP holds (the object and this call hold two), returns to the pool.
    if force
        || Rc::strong_count(&conn) > 2
        || !conn.keep
        || conn.flag(state::SHUTDOWN_READ)
        || conn.flag(state::SHUTDOWN_WRITE)
        || !conn.alive()
    {
        let Some(result) = bailed(conn.cli.run(Op::Close)) else {
            return call.set_bool(false);
        };
        bailed(release(object, conn));
        return call.set_bool(result >= 0);
    }
    let pushed = {
        let mut pool = long_connections();
        if pool.is_none() {
            *pool = Pool::create();
        }
        pool.as_mut()
            .map(|pool| pool.push(&conn.key, Pooled(Box::new(conn.detach()))))
    };
    if !matches!(pushed, Some(Ok(()))) {
        std::process::abort();
    }
    this(call).set_conn(None);
    call.set_bool(true);
}

/// `php_swoole_client_enable_ssl_encryption`.
pub(crate) fn enable_ssl(call: &Call, conn: &Conn) -> bool {
    if !matches!(conn.socket_type(), SOCK_TCP | SOCK_TCP6) {
        warn(ErrorKind::Fatal, "cannot use enableSSL");
        return false;
    }
    if conn.has_ssl() {
        warn(ErrorKind::Fatal, "SSL has been enabled");
        return false;
    }
    if let Some(zset) = settings(Obj::this(call)) {
        crate::settings::apply_ssl(conn.cli, zset);
    }
    bailed(conn.cli.run(Op::EnableSsl)).is_some_and(|result| result == 0)
}

/// `enableSSL(?callable $onSslReady = null): bool`.
fn enable_ssl_method(call: &Call) {
    if call.arity(0, 1).is_err() {
        return call.set_bool(false);
    }
    if !call.zval(1).is_null() {
        call.throw(
            swoole_rs_phpswoole::exception(),
            c"sync client does not support `onSslReady` callback",
            i64::from(INVALID_PARAMS),
        );
        return call.set_bool(false);
    }
    let Some(conn) = connected(call) else {
        return call.set_bool(false);
    };
    if !enable_ssl(call, &conn) {
        return call.set_bool(false);
    }
    call.set_bool(bailed(conn.cli.run(Op::SslHandshake)) == Some(0));
}

/// `getPeerCert(): string|false`.
fn get_peer_cert(call: &Call) {
    let Some(conn) = connected(call) else {
        return call.set_bool(false);
    };
    if !conn.has_ssl() {
        warn(ErrorKind::Fatal, "SSL is not ready");
        return call.set_bool(false);
    }
    match conn.cli.net().and_then(native::peer_cert) {
        Some(cert) => call.set_zstring(cert),
        None => call.set_bool(false),
    }
}

/// `verifyPeerCert(bool $allow_self_signed = false): bool`.
fn verify_peer_cert(call: &Call) {
    let Some(conn) = connected(call) else {
        return call.set_bool(false);
    };
    if !conn.has_ssl() {
        warn(ErrorKind::Fatal, "SSL is not ready");
        return call.set_bool(false);
    }
    let Ok(allow) = call.arity(0, 1).and_then(|()| call.bool(1, false)) else {
        return call.set_bool(false);
    };
    let verify = conn.cli.run(Op::SslVerify(allow));
    call.set_bool(bailed(verify).is_some_and(|result| result >= 0));
}

/// `shutdown(int $how): bool`.
fn shutdown(call: &Call) {
    let Some(conn) = connected(call) else {
        return call.set_bool(false);
    };
    let Ok(how) = call.arity(1, 1).and_then(|()| call.long(1, 0)) else {
        return call.set_bool(false);
    };
    let shutdown = conn.cli.run(Op::Shutdown(int(how)));
    call.set_bool(bailed(shutdown).is_some_and(|result| result >= 0));
}

swoole_rs_zend::handlers! {
    zim_swoole_client___construct => construct;
    zim_swoole_client___destruct => destruct;
    zim_swoole_client_set => set;
    zim_swoole_client_connect => connect;
    zim_swoole_client_recv => recv;
    zim_swoole_client_send => send;
    zim_swoole_client_sendfile => sendfile;
    zim_swoole_client_sendto => sendto;
    zim_swoole_client_enableSSL => enable_ssl_method;
    zim_swoole_client_getPeerCert => get_peer_cert;
    zim_swoole_client_verifyPeerCert => verify_peer_cert;
    zim_swoole_client_isConnected => is_connected;
    zim_swoole_client_getsockname => getsockname;
    zim_swoole_client_getpeername => getpeername;
    zim_swoole_client_close => close;
    zim_swoole_client_shutdown => shutdown;
    zim_swoole_client_getSocket => get_socket;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eof_search_finds_the_first_match() {
        assert_eq!(position(b"ab\r\ncd\r\n", b"\r\n"), Some(2), "first");
        assert_eq!(position(b"abc", b"\r\n"), None, "absent");
        assert_eq!(position(b"\r", b"\r\n"), None, "shorter than the EOF");
        assert_eq!(position(b"abc", b""), None, "no EOF");
    }

    #[test]
    fn socket_type_drops_the_client_flags() {
        assert_eq!(
            socket_type(SOCK_TCP | FLAG_KEEP | SOCK_SSL),
            SOCK_TCP,
            "keep and ssl"
        );
        assert_eq!(
            socket_type(SOCK_UNIX_DGRAM | FLAG_SYNC | FLAG_ASYNC),
            SOCK_UNIX_DGRAM,
            "sync and async"
        );
    }
}
