//! `Swoole\Client`: registration, the object and every method, over
//! `network::Client`. The object holds a [`Conn`]: the C++ client plus what
//! the PHP layer keeps beside it (the persistent-connection key and count and
//! the `recv` framing buffer). A `SWOOLE_KEEP` connection outlives its object
//! in the pool. Each method parses as upstream's ZPP block does; operations
//! that can run PHP code go through `swoole_client_run` inside `zend_try`.
use crate::native::{self, Cli, ClientCall, ErrorKind, Framing, Op};
use libc::{c_char, c_int, c_void};
use std::{
    ffi::{CStr, CString},
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
};
use swoole_rs_clients::{pool, state, swr_v1_clients_port};
use swoole_rs_zend::{
    Call, Class, E_ERROR, E_WARNING, Object,
    bailout::{self, Bailout},
    declare_bool, declare_long, long_zval, register_long_constant, string,
    sys::{
        IS_ARRAY, IS_NULL, IS_STRING, IS_UNDEF, ZEND_ACC_NOT_SERIALIZABLE, ZEND_ACC_PUBLIC,
        get_active_class_name, get_active_function_name, swr_zend_property_not_null,
        swr_zend_property_slot, swr_zval_try_addref, zend_class_entry, zend_function_entry,
        zend_object, zend_object_handlers, zend_string, zend_type_error, zval, zval_ptr_dtor,
    },
    throw_error, throw_exception, type_of, unset_property_deny, update_property, value,
};

/// `SW_FLAG_ASYNC`, `SW_FLAG_SYNC`, `SW_FLAG_KEEP` and `SW_SOCK_SSL`.
const FLAG_ASYNC: i64 = 1 << 10;
const FLAG_SYNC: i64 = 1 << 11;
const FLAG_KEEP: i64 = 1 << 12;
const SOCK_SSL: i64 = 1 << 9;
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
/// `SW_CLOSE`.
const CLOSE: c_int = 1;

/// One connection: the `network::Client` and what the PHP layer keeps with it
/// (upstream's `keep`, `server_id`, `reuse_count`, `buffer` and
/// `ssl_wait_handshake` client fields).
pub(crate) struct Conn {
    pub(crate) cli: Cli,
    key: Vec<u8>,
    keep: bool,
    reuse_count: u32,
    /// `cli->buffer`'s bytes, and its size (0: no buffer).
    buffer: Vec<u8>,
    buffer_size: usize,
    pub(crate) ssl_wait: bool,
}

impl Conn {
    pub(crate) fn new(cli: Cli) -> Self {
        Self {
            cli,
            key: Vec::new(),
            keep: false,
            reuse_count: 0,
            buffer: Vec::new(),
            buffer_size: 0,
            ssl_wait: false,
        }
    }

    fn state(&self) -> *mut native::ClientState {
        // SAFETY: a live client.
        unsafe { native::swoole_client_state(self.cli) }.cast()
    }

    pub(crate) fn flag(&self, name: u32) -> bool {
        // SAFETY: the client's live state.
        unsafe { state::swr_v1_clients_flag(self.state(), name) }
    }

    pub(crate) fn set_flag(&self, name: u32, value: bool) {
        // SAFETY: the client's live state, not borrowed elsewhere.
        unsafe { state::swr_v1_clients_set_flag(self.state(), name, value) };
    }

    pub(crate) fn call(&self, op: Op) -> ClientCall {
        ClientCall::new(self.cli, op)
    }

    /// Run an IO call: a sync client raises no callbacks, so only an async
    /// one needs the `zend_try` frame.
    fn run(&self, call: &mut ClientCall) -> Result<i64, Bailout> {
        if self.flag(state::ASYNC) {
            call.run()
        } else {
            Ok(call.run_plain())
        }
    }

    pub(crate) fn fd(&self) -> c_int {
        // SAFETY: a live client with a socket.
        unsafe { native::swoole_client_fd(self.cli) }
    }

    fn socket_type(&self) -> i64 {
        // SAFETY: as above.
        i64::from(unsafe { native::swoole_client_socket_type(self.cli) })
    }

    pub(crate) fn is_stream(&self) -> bool {
        matches!(self.socket_type(), SOCK_TCP | SOCK_TCP6 | SOCK_UNIX_STREAM)
    }

    pub(crate) fn has_ssl(&self) -> bool {
        // SAFETY: as above.
        unsafe { native::swoole_client_has_ssl(self.cli) }
    }
}

/// `ClientObject` without its `zend_object`; `Swoole\Async\Client` shares it.
#[repr(C)]
pub(crate) struct Data {
    pub(crate) conn: *mut Conn,
    /// `getSocket()`'s socket (`IS_UNDEF`: none).
    zsocket: zval,
    /// The async callbacks (`zend::Callable *`, by [`crate::async_net`] slot).
    pub(crate) callbacks: *mut [*mut c_void; 7],
}

pub(crate) type ClientObject = Object<Data>;

pub(crate) static CLIENT: Class = Class::new();
static EXCEPTION: Class = Class::new();
/// The persistent connections (`long_connections`), keyed by server id.
static POOL: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

unsafe extern "C" fn create(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: Zend creates this class or a subclass; zeroed data is no
    // connection, no socket and no callbacks.
    unsafe { ClientObject::create(&CLIENT, ce) }
}

unsafe extern "C" fn free(std: *mut zend_object) {
    // SAFETY: Zend frees a client object.
    if !unsafe { ClientObject::data(std) }.callbacks.is_null() {
        bailout::defer(crate::async_net::free_callbacks(std));
    }
    // SAFETY: as above.
    unsafe { ClientObject::dtor(std) };
}

fn configure(entry: &mut zend_class_entry, handlers: &mut zend_object_handlers) {
    entry.ce_flags |= ZEND_ACC_NOT_SERIALIZABLE;
    handlers.clone_obj = None;
    handlers.unset_property = Some(unset_property_deny);
    ClientObject::install(entry, handlers, create, free);
}

/// MINIT, as `php_swoole_client_minit` did.
/// # Safety
/// Called once, at MINIT, with upstream's static method table, Swoole's
/// exception class and a writable class-entry slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpclients_net_minit(
    methods: *const zend_function_entry,
    exception: *mut zend_class_entry,
    module: c_int,
    client_class: *mut *mut zend_class_entry,
) {
    // SAFETY: MINIT, with the caller's table.
    unsafe { CLIENT.register(c"Swoole\\Client", methods, configure) };
    // SAFETY: MINIT; the parent is registered.
    unsafe {
        EXCEPTION.register_child(
            c"Swoole\\Client\\Exception",
            ptr::null(),
            exception,
            |_, _| {},
        );
    }
    let ce = CLIENT.entry();
    // SAFETY: MINIT, with the class just registered (each call below too).
    unsafe { declare_long(ce, c"errCode", 0, ZEND_ACC_PUBLIC) };
    // SAFETY: as above.
    unsafe { declare_long(ce, c"sock", -1, ZEND_ACC_PUBLIC) };
    // SAFETY: as above.
    unsafe { declare_bool(ce, c"reuse", false, ZEND_ACC_PUBLIC) };
    // SAFETY: as above.
    unsafe { declare_long(ce, c"reuseCount", 0, ZEND_ACC_PUBLIC) };
    // SAFETY: as above.
    unsafe { declare_long(ce, c"type", 0, ZEND_ACC_PUBLIC) };
    // SAFETY: as above.
    unsafe { CLIENT.property_null(c"id") };
    // SAFETY: as above.
    unsafe { CLIENT.property_null(c"setting") };
    for (name, value) in [
        (c"MSG_OOB", libc::MSG_OOB),
        (c"MSG_PEEK", libc::MSG_PEEK),
        (c"MSG_DONTWAIT", libc::MSG_DONTWAIT),
        (c"MSG_WAITALL", libc::MSG_WAITALL),
        (c"SHUT_RDWR", libc::SHUT_RDWR),
        (c"SHUT_RD", libc::SHUT_RD),
        (c"SHUT_WR", libc::SHUT_WR),
    ] {
        // SAFETY: as above.
        unsafe { CLIENT.constant_long(name, i64::from(value)) };
    }
    for (name, value) in [
        (c"SWOOLE_SYNC", FLAG_SYNC),
        (c"SWOOLE_ASYNC", FLAG_ASYNC),
        (c"SWOOLE_KEEP", FLAG_KEEP),
    ] {
        // SAFETY: MINIT, with the extension's module number.
        unsafe { register_long_constant(name, value, module) };
    }
    EXCEPTION_PARENT.store(exception, Ordering::Release);
    // SAFETY: per the caller, a writable slot.
    unsafe { *client_class = ce };
}

pub(crate) fn data<'a>(object: *mut zend_object) -> &'a mut Data {
    // SAFETY: client methods are non-static, so `$this` is a client object (or
    // an async one, which shares the layout), and no other reference to its
    // data is live during the call.
    unsafe { ClientObject::data(object) }
}

pub(crate) fn this<'a>(call: &Call) -> &'a mut Data {
    data(call.this())
}

/// The object's connection.
pub(crate) fn conn<'a>(call: &Call) -> Option<&'a mut Conn> {
    // SAFETY: the object owns its connection while it holds it.
    unsafe { this(call).conn.as_mut() }
}

/// `php_swoole_client_get_type`: the socket type without the client flags.
pub(crate) fn socket_type(kind: i64) -> i64 {
    kind & !(FLAG_SYNC | FLAG_ASYNC | FLAG_KEEP | SOCK_SSL)
}

/// `php_swoole_error` and friends (`kind`) at `level`, deferring a bailout.
pub(crate) fn report(kind: ErrorKind, level: c_int, message: &str) {
    let message = CString::new(message).unwrap_or_default();
    // SAFETY: C++'s reporter, from a handler.
    bailout::defer(unsafe {
        bailout::report_code(
            native::swoole_client_php_error,
            kind as c_int,
            level,
            &message,
        )
    });
}

pub(crate) fn warn(kind: ErrorKind, message: &str) {
    report(kind, E_WARNING, message);
}

/// `php_swoole_sys_error(E_WARNING, ...)` of the failure that set `error`.
pub(crate) fn warn_sys(message: &str, error: c_int) {
    let message = CString::new(message).unwrap_or_default();
    set_errno(error);
    // SAFETY: C++'s reporter, from a handler; it reads errno, just set.
    bailout::defer(unsafe {
        bailout::report_code(
            native::swoole_client_php_error,
            ErrorKind::Sys as c_int,
            E_WARNING,
            &message,
        )
    });
}

fn set_errno(error: c_int) {
    #[cfg(target_os = "linux")]
    // SAFETY: a pure query for this thread's errno.
    let slot = unsafe { libc::__errno_location() };
    #[cfg(not(target_os = "linux"))]
    // SAFETY: as above.
    let slot = unsafe { libc::__error() };
    // SAFETY: this thread's errno, live for the thread.
    unsafe { *slot = error };
}

/// `swoole_set_last_error(code)`.
pub(crate) fn set_last_error(code: c_int) {
    // SAFETY: a plain store into Swoole's thread error.
    unsafe { native::swoole_rs_set_last_error(code) };
}

pub(crate) fn last_error() -> c_int {
    // SAFETY: a plain read of Swoole's thread error.
    unsafe { native::swoole_client_last_error() }
}

/// `errCode = code`.
pub(crate) fn set_err_code(object: *mut zend_object, code: c_int) {
    // SAFETY: a live client object.
    let ce = unsafe { (*object).ce };
    // SAFETY: as above.
    unsafe { value::update_long(ce, object, c"errCode", i64::from(code)) };
}

pub(crate) fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// `swoole_strerror(code)`.
pub(crate) fn strerror(code: c_int) -> String {
    extern "C" fn system(code: c_int) -> *const c_char {
        crate::socket::strerror(code).as_ptr()
    }
    // SAFETY: known and system messages are static; the unknown-message buffer
    // is only used on the PHP thread, as upstream's is.
    let message = unsafe { swoole_rs_error::swr_v1_error_strerror(code, Some(system)) };
    if message.is_null() {
        return String::new();
    }
    // SAFETY: a NUL-terminated message.
    unsafe { CStr::from_ptr(message) }
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

fn pool() -> *mut c_void {
    let handle = POOL.load(Ordering::Acquire);
    if !handle.is_null() {
        return handle;
    }
    let handle = pool::swr_v1_clients_pool_create();
    POOL.store(handle, Ordering::Release);
    handle
}

/// `php_swoole_client_free`: release the connection's timer, length callable,
/// pool entry and client, then the exported socket.
pub(crate) fn release(object: *mut zend_object, conn: *mut Conn) -> Result<(), Bailout> {
    // SAFETY: the connection the object held or the pool gave back, released once.
    let conn = unsafe { Box::from_raw(conn) };
    if conn.keep {
        let handle = POOL.load(Ordering::Acquire);
        // SAFETY: the pool, null or live; the key is this connection's.
        if unsafe { pool::swr_v1_clients_pool_state(handle, conn.key.as_ptr(), conn.key.len()) }
            .kind
            == 1
        {
            // SAFETY: as above.
            unsafe {
                pool::swr_v1_clients_pool_remove_empty(handle, conn.key.as_ptr(), conn.key.len())
            };
        }
    }
    let destroyed = conn.call(Op::Destroy).run().map(drop);
    let data = data(object);
    // SAFETY: the object's own zval.
    if unsafe { type_of(&raw const data.zsocket) } != IS_UNDEF {
        // SAFETY: the reference the object took in getSocket().
        unsafe { zval_ptr_dtor(&raw mut data.zsocket) };
        data.zsocket.u1.type_info = IS_UNDEF;
    }
    data.conn = ptr::null_mut();
    destroyed
}

/// `php_swoole_client_get_cli_safe`: the connected client; else errCode and a
/// warning say it is not connected.
pub(crate) fn connected<'a>(call: &Call) -> Option<&'a mut Conn> {
    let object = call.this();
    if let Some(conn) = conn(call) {
        // SAFETY: a live client.
        if unsafe { native::swoole_client_has_socket(conn.cli) } {
            if conn.flag(state::ACTIVE) {
                return Some(conn);
            }
            if conn.flag(state::ASYNC_CONNECT) {
                conn.set_flag(state::ASYNC_CONNECT, false);
                let mut error: c_int = -1;
                // SAFETY: a live client; `error` is a writable local.
                if unsafe { native::swoole_client_socket_error(conn.cli, &raw mut error) }
                    && error == 0
                {
                    conn.set_flag(state::ACTIVE, true);
                    return Some(conn);
                }
                let conn = ptr::from_mut(conn);
                bailed(release(object, conn))?;
            }
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
pub(crate) fn type_property(object: *mut zend_object, async_message: bool) -> Option<i64> {
    // SAFETY: a live object with a declared `type` property.
    let ztype = unsafe { swr_zend_property_slot(object, c"type".as_ptr(), 4) };
    // SAFETY: Zend's property slot.
    if ztype.is_null() || unsafe { type_of(ztype) } == IS_NULL {
        let message = if async_message {
            "failed to get client type"
        } else {
            "failed to get swoole_client->type"
        };
        report(ErrorKind::Fatal, E_ERROR, message);
        return None;
    }
    // SAFETY: `Z_LVAL_P`, as upstream reads it.
    Some(unsafe { value::long_of(ztype) })
}

/// The `setting` property when it is an array.
pub(crate) fn settings(object: *mut zend_object) -> Option<*mut zval> {
    // SAFETY: a live object with a declared `setting` property.
    let zset = unsafe { swr_zend_property_slot(object, c"setting".as_ptr(), 7) };
    // SAFETY: Zend's property slot.
    (!zset.is_null() && unsafe { type_of(zset) } == IS_ARRAY).then_some(zset)
}

/// `php_swoole_client_new`: a pooled or new connection for `host:port`.
fn connection(object: *mut zend_object, host: &[u8], port: i64) -> Option<*mut Conn> {
    let kind = type_property(object, false)?;
    #[expect(clippy::cast_possible_truncation, reason = "upstream passes an int")]
    let int_port = port as i32;
    if matches!(socket_type(kind), SOCK_TCP | SOCK_TCP6) && swr_v1_clients_port(int_port, 0) == 0 {
        warn(ErrorKind::Fatal, "The port is invalid");
        set_last_error(INVALID_PARAMS);
        return None;
    }
    // SAFETY: a live object with a declared `id` property.
    let zid = unsafe { swr_zend_property_not_null(object, c"id".as_ptr(), 2) };
    // SAFETY: a live property zval when not null.
    let id = (!zid.is_null() && unsafe { type_of(zid) } == IS_STRING).then(|| {
        // SAFETY: a string zval.
        let id = unsafe { value::str_of(zid) };
        // SAFETY: a live string the property holds.
        unsafe { string::bytes(id) }
    });
    let key = match id {
        Some(id) if !id.is_empty() => id.to_vec(),
        _ => {
            let host = host.split(|&b| b == 0).next().unwrap_or_default();
            let mut key = host.to_vec();
            key.extend_from_slice(format!(":{int_port}").as_bytes());
            key
        }
    };
    let mut conn: *mut Conn = ptr::null_mut();
    if kind & FLAG_KEEP != 0 {
        let handle = POOL.load(Ordering::Acquire);
        // SAFETY: the pool, null or live; `key` is initialized.
        if unsafe { pool::swr_v1_clients_pool_state(handle, key.as_ptr(), key.len()) }.kind == 2 {
            // SAFETY: as above; the pool holds connections this module pushed.
            conn =
                unsafe { pool::swr_v1_clients_pool_take(handle, key.as_ptr(), key.len()) }.cast();
            // SAFETY: a pooled connection, now this object's.
            let pooled = unsafe { &mut *conn };
            // SAFETY: a live client with a socket.
            if unsafe { native::swoole_client_check_liveness(pooled.cli) } {
                pooled.reuse_count += 1;
                // SAFETY: a live client object.
                let ce = unsafe { (*object).ce };
                // SAFETY: as above.
                unsafe {
                    value::update_long(ce, object, c"reuseCount", i64::from(pooled.reuse_count));
                };
            } else {
                bailed(pooled.call(Op::Close).run())?;
                bailed(release(object, conn))?;
                conn = ptr::null_mut();
            }
        }
    }
    if conn.is_null() {
        let mut error = 0;
        #[expect(clippy::cast_possible_truncation, reason = "a socket type")]
        // SAFETY: a socket type within range (checked by the constructor).
        let cli = unsafe {
            native::swoole_client_create(socket_type(kind) as c_int, false, &raw mut error)
        };
        if cli.is_null() {
            warn_sys("Client_create() failed", error);
            set_err_code(object, error);
            return None;
        }
        let mut fresh = Box::new(Conn::new(cli));
        fresh.key = key;
        conn = Box::into_raw(fresh);
    }
    // SAFETY: the connection just taken or made.
    let connection = unsafe { &mut *conn };
    // SAFETY: a live client object.
    let ce = unsafe { (*object).ce };
    // SAFETY: as above.
    unsafe { value::update_long(ce, object, c"sock", i64::from(connection.fd())) };
    if kind & FLAG_KEEP != 0 {
        connection.keep = true;
    }
    if kind & SOCK_SSL != 0 {
        bailed(connection.call(Op::EnableSsl).run())?;
    }
    Some(conn)
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
        // SAFETY: from a handler.
        unsafe { throw_error(c"socket type param is required") };
        return call.set_bool(false);
    };
    if is_async {
        // SAFETY: from a handler.
        unsafe { throw_error(c"The $async parameter is not supported") };
        return call.set_bool(false);
    }
    if !check_type(kind) {
        return call.set_bool(false);
    }
    let object = call.this();
    // SAFETY: `$this` is a live instance of the registered class.
    unsafe { value::update_long(CLIENT.entry(), object, c"type", kind) };
    if let Some(id) = id {
        let id = string::copy(id);
        let mut zid = long_zval(0);
        // SAFETY: a fresh string; the zval takes its reference.
        unsafe { value::set_str(&raw mut zid, id) };
        // SAFETY: `$this` is live; Zend copies the value.
        unsafe { update_property(CLIENT.entry(), object, c"id", &raw mut zid) };
        // SAFETY: our reference.
        unsafe { zval_ptr_dtor(&raw mut zid) };
    }
    call.set_bool(true);
}

/// `IS_OBJ_DESTRUCTOR_CALLED`.
const DESTRUCTOR_CALLED: u32 = 1 << 8;

/// `SW_PREVENT_USER_DESTRUCT`: false (returning null) when user code called it.
pub(crate) fn destructing(call: &Call) -> bool {
    // SAFETY: a method's live `$this`.
    let info = unsafe { (*call.this()).gc.u };
    // SAFETY: a refcounted header always sets `type_info`.
    if unsafe { info.type_info } & DESTRUCTOR_CALLED == 0 {
        call.set_null();
        return false;
    }
    true
}

/// `__destruct()`: close a held connection through `Swoole\Client::close`.
fn destruct(call: &Call) {
    if !destructing(call) || this(call).conn.is_null() {
        return;
    }
    let mut close = ClientCall::new(call.this().cast(), Op::Destruct);
    bailed(close.run());
}

/// `set(array $settings): bool`.
fn set(call: &Call) {
    if call.arity(1, 1).is_err() {
        return call.set_bool(false);
    }
    let settings = call.zval(1);
    // SAFETY: a passed argument.
    if unsafe { type_of(settings) } != IS_ARRAY {
        return call.set_bool(false);
    }
    // SAFETY: `$this` is live; `settings` is a live array.
    unsafe { native::swoole_client_merge_setting(call.this(), settings) };
    call.set_bool(true);
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
    let object = call.this();
    if !this(call).conn.is_null() {
        warn(
            ErrorKind::Fatal,
            "connection to the server has already been established",
        );
        return call.set_bool(false);
    }
    let Some(conn) = connection(object, host, port) else {
        return call.set_bool(false);
    };
    this(call).conn = conn;
    // SAFETY: the object's connection.
    let conn = unsafe { &mut *conn };
    if conn.keep && conn.flag(state::ACTIVE) {
        // SAFETY: `$this` is a live client.
        unsafe { value::update_bool(CLIENT.entry(), object, c"reuse", true) };
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
        let mut apply = conn.call(Op::Settings);
        apply.settings = zset;
        let Some(applied) = bailed(apply.run()) else {
            return call.set_bool(false);
        };
        if applied == 0 {
            set_err_code(object, last_error());
            return call.set_bool(false);
        }
    }
    let mut connect = conn.call(Op::Connect);
    connect.data = host.as_ptr().cast();
    connect.a = port;
    connect.timeout = timeout;
    connect.b = flags;
    let Some(result) = bailed(connect.run()) else {
        return call.set_bool(false);
    };
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
        bailed(release(object, ptr::from_mut(conn)));
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
    let mut send = conn.call(Op::Send);
    send.data = data.as_ptr().cast();
    send.length = data.len();
    send.a = flags;
    let Some(sent) = bailed(conn.run(&mut send)) else {
        return call.set_bool(false);
    };
    if sent < 0 {
        let error = errno();
        let message = format!("failed to send({}) {} bytes", conn.fd(), data.len());
        warn_sys(&message, error);
        set_err_code(call.this(), last_error());
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
    let object = call.this();
    if this(call).conn.is_null() {
        let Some(conn) = connection(object, host, port) else {
            return call.set_bool(false);
        };
        // SAFETY: the connection just made.
        unsafe { &*conn }.set_flag(state::ACTIVE, true);
        this(call).conn = conn;
    }
    let Some(conn) = conn(call) else {
        return call.set_bool(false);
    };
    let mut send = conn.call(Op::Sendto);
    send.data = host.as_ptr().cast();
    send.length = host.len();
    send.a = port;
    send.buffer = data.as_ptr().cast_mut().cast();
    #[expect(clippy::cast_possible_wrap, reason = "a string's length")]
    let length = data.len() as i64;
    send.b = length;
    let Some(result) = bailed(conn.run(&mut send)) else {
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
    let mut send = conn.call(Op::Sendfile);
    send.data = file.as_ptr().cast();
    send.a = offset;
    send.b = length;
    let Some(result) = bailed(conn.run(&mut send)) else {
        return call.set_bool(false);
    };
    if result < 0 {
        set_last_error(errno());
        let error = last_error();
        let message = format!("sendfile() failed. Error: {} [{error}]", strerror(error));
        warn(ErrorKind::Fatal, &message);
        set_err_code(call.this(), last_error());
        return call.set_bool(false);
    }
    call.set_bool(true);
}

/// `cli->recv(buffer, length, flags)`.
fn receive(conn: &Conn, buffer: *mut u8, length: usize, flags: i64) -> Result<i64, Bailout> {
    let mut recv = conn.call(Op::Recv);
    recv.buffer = buffer.cast();
    recv.length = length;
    recv.a = flags;
    conn.run(&mut recv)
}

/// `recv()` failed (`error`): warn, errCode and false.
fn recv_failed(call: &Call, error: c_int) {
    warn_sys("recv() failed", error);
    set_err_code(call.this(), last_error());
    call.set_bool(false);
}

/// `recv` under `open_eof_check`: one package up to and including the EOF,
/// keeping what follows for the next call.
fn recv_eof(call: &Call, conn: &mut Conn, framing: &Framing) {
    let eof = &framing.eof[..usize::from(framing.eof_length).min(framing.eof.len())];
    let max = framing.max_length as usize;
    if conn.buffer_size == 0 {
        conn.buffer_size = BUFFER_SIZE_BIG;
        conn.buffer.clear();
    }
    let mut find = !conn.buffer.is_empty();
    loop {
        if !find {
            let length = conn.buffer.len();
            let room = (conn.buffer_size - length.min(conn.buffer_size)).min(BUFFER_SIZE_BIG);
            conn.buffer.resize(length + room, 0);
            let target = conn.buffer.as_mut_ptr().wrapping_add(length);
            let read = receive(conn, target, room, 0);
            let Some(read) = bailed(read) else {
                conn.buffer.truncate(length);
                return call.set_bool(false);
            };
            if read < 0 {
                let error = errno();
                conn.buffer.clear();
                return recv_failed(call, error);
            } else if read == 0 {
                conn.buffer.clear();
                return call.set_empty_string();
            }
            conn.buffer
                .truncate(length + usize::try_from(read).unwrap_or(0));
            if conn.buffer.len() < eof.len() {
                continue;
            }
        }
        find = false;
        if let Some(at) = position(&conn.buffer, eof) {
            let end = at + eof.len();
            let package = string::copy(&conn.buffer[..end]);
            if conn.buffer.len() > end {
                conn.buffer.drain(..end);
                conn.buffer_size = BUFFER_SIZE_BIG;
            } else {
                conn.buffer.clear();
                conn.buffer_size = 0;
            }
            // SAFETY: a fresh string whose reference the return slot takes.
            return unsafe { call.set_string(package) };
        }
        if conn.buffer.len() == max {
            warn(ErrorKind::Error, "no package eof");
            conn.buffer.clear();
            return call.set_bool(false);
        } else if conn.buffer.len() == conn.buffer_size && conn.buffer_size < max {
            conn.buffer_size = (conn.buffer_size * 2).min(max);
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
    conn: &mut Conn,
    framing: &Framing,
    mut length: i64,
) -> Option<(*mut zend_string, i64)> {
    if conn.buffer_size == 0 {
        conn.buffer_size = BUFFER_SIZE_STD;
    }
    conn.buffer.clear();
    let header = usize::from(framing.length_offset) + usize::from(framing.length_size);
    loop {
        let have = conn.buffer.len();
        let want = header.saturating_sub(have);
        conn.buffer.resize(have + want, 0);
        let target = conn.buffer.as_mut_ptr().wrapping_add(have);
        let read = receive(conn, target, want, 0);
        let Some(read) = bailed(read) else {
            call.set_bool(false);
            return None;
        };
        if read <= 0 {
            conn.buffer.truncate(have);
            break;
        }
        conn.buffer
            .truncate(have + usize::try_from(read).unwrap_or(0));
        let mut measure = conn.call(Op::PackageLength);
        measure.buffer = conn.buffer.as_mut_ptr().cast();
        measure.length = conn.buffer.len();
        let Some(measured) = bailed(measure.run()) else {
            call.set_bool(false);
            return None;
        };
        length = measured;
        if length != 0 {
            break;
        }
    }
    let buffered = conn.buffer.len();
    #[expect(clippy::cast_possible_wrap, reason = "lengths below 4 GiB")]
    let (header_length, have) = (header as i64, buffered as i64);
    if length < 0 {
        call.set_empty_string();
        return None;
    } else if length == header_length {
        conn.buffer.resize(header.max(buffered), 0);
        let package = string::copy(&conn.buffer[..header]);
        // SAFETY: a fresh string whose reference the return slot takes.
        unsafe { call.set_string(package) };
        return None;
    } else if length > i64::from(framing.max_length) {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "upstream prints (int) buf_len"
        )]
        // SAFETY: Swoole's logger.
        unsafe {
            native::swoole_client_package_too_big(length as c_int);
        };
        call.set_empty_string();
        return None;
    } else if length == have {
        // SAFETY: a fresh string whose reference the return slot takes.
        unsafe { call.set_string(string::copy(&conn.buffer)) };
        return None;
    } else if length < have {
        let end = usize::try_from(length).unwrap_or(0);
        // SAFETY: a fresh string whose reference the return slot takes.
        unsafe { call.set_string(string::copy(&conn.buffer[..end])) };
        conn.buffer.drain(..end);
        return None;
    }
    let total = usize::try_from(length).unwrap_or(0);
    let package = string::alloc(total.max(header));
    let target = string::val(package);
    // SAFETY: `package` has room for the buffered bytes (fewer than `length`).
    unsafe { ptr::copy_nonoverlapping(conn.buffer.as_ptr(), target, buffered) };
    set_last_error(0);
    let rest = total.saturating_sub(buffered);
    let read = receive(
        conn,
        target.wrapping_add(header),
        rest,
        i64::from(libc::MSG_WAITALL),
    );
    let Some(mut read) = bailed(read) else {
        // SAFETY: our unshared string.
        unsafe { string::free(package) };
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
    let mut framing = Framing::default();
    // SAFETY: a live client; `framing` is a writable local.
    unsafe { native::swoole_client_framing(conn.cli, &raw mut framing) };
    let (package, read) = if framing.eof_check {
        return recv_eof(call, conn, &framing);
    } else if framing.length_check {
        let Some(received) = recv_length(call, conn, &framing, length) else {
            return;
        };
        received
    } else {
        if flags & i64::from(libc::MSG_WAITALL) == 0 && length > PHP_BUFFER_SIZE {
            length = PHP_BUFFER_SIZE;
        }
        // A negative size reads nothing.
        let size = usize::try_from(length).unwrap_or(0);
        let package = string::alloc(size);
        set_last_error(0);
        let Some(read) = bailed(receive(conn, string::val(package), size, flags)) else {
            // SAFETY: our unshared string.
            unsafe { string::free(package) };
            return call.set_bool(false);
        };
        (package, read)
    };
    if read <= 0 {
        let error = errno();
        // SAFETY: our unshared string.
        unsafe { string::free(package) };
        if read < 0 {
            return recv_failed(call, error);
        }
        return call.set_empty_string();
    }
    // SAFETY: our string holds at least `read` bytes.
    unsafe { string::truncate(package, usize::try_from(read).unwrap_or(0)) };
    // SAFETY: the return slot takes our reference.
    unsafe { call.set_string(package) };
}

/// `isConnected(): bool`.
pub(crate) fn is_connected(call: &Call) {
    let Some(conn) = conn(call) else {
        return call.set_bool(false);
    };
    // SAFETY: a live client.
    if !unsafe { native::swoole_client_has_socket(conn.cli) } {
        return call.set_bool(false);
    }
    call.set_bool(conn.flag(state::ACTIVE));
}

/// `getsockname(): array|false`.
fn getsockname(call: &Call) {
    let Some(conn) = connected(call) else {
        return call.set_bool(false);
    };
    let mut port = 0;
    // SAFETY: a live client; `port` is a writable local.
    let address = unsafe { native::swoole_client_sockname(conn.cli, &raw mut port) };
    if address.is_null() {
        let error = errno();
        warn_sys("getsockname() failed", error);
        set_err_code(call.this(), error);
        return call.set_bool(false);
    }
    // SAFETY: the socket's NUL-terminated address.
    let address = unsafe { CStr::from_ptr(address) };
    address_array(call, port, address);
}

/// `['port' => $port, 'host' => $host]`.
fn address_array(call: &Call, port: c_int, host: &CStr) {
    call.set_array();
    call.add_assoc_long(c"port", i64::from(port));
    // SAFETY: the return slot is an array.
    unsafe { value::add_string(call.return_value(), c"host", host) };
}

/// `getpeername(): array|false`.
fn getpeername(call: &Call) {
    let Some(conn) = connected(call) else {
        return call.set_bool(false);
    };
    let mut address = [0 as c_char; 128];
    let mut port = 0;
    // SAFETY: a live client; writable locals.
    if !unsafe {
        native::swoole_client_peername(conn.cli, address.as_mut_ptr(), address.len(), &raw mut port)
    } {
        let error = errno();
        warn_sys("getpeername() failed", error);
        set_err_code(call.this(), error);
        return call.set_bool(false);
    }
    // SAFETY: the shim wrote a NUL-terminated address.
    let address = unsafe { CStr::from_ptr(address.as_ptr()) };
    address_array(call, port, address);
}

/// `getSocket()`: the client's socket as an ext/sockets `Socket`.
fn get_socket(call: &Call) {
    let data = this(call);
    // SAFETY: the object's own zval.
    if unsafe { type_of(&raw const data.zsocket) } != IS_UNDEF {
        let mut socket = data.zsocket;
        // SAFETY: `RETURN_ZVAL(zsocket, 1, 0)`: a new reference for the return.
        unsafe { swr_zval_try_addref(&raw mut socket) };
        return call.set_value(socket);
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
    let mut socket = long_zval(0);
    // SAFETY: a live client; `socket` is a writable local.
    if !unsafe { native::swoole_client_export_socket(conn.cli, &raw mut socket) } {
        return call.set_bool(false);
    }
    call.set_value(socket);
    // SAFETY: the object keeps a second reference, as upstream's dup does.
    unsafe { swr_zval_try_addref(&raw mut socket) };
    this(call).zsocket = socket;
}

/// `close(bool $force = false): bool`.
fn close(call: &Call) {
    let Ok(force) = call.arity(0, 1).and_then(|()| call.bool(1, false)) else {
        return call.set_bool(false);
    };
    let object = call.this();
    let conn_ptr = this(call).conn;
    // SAFETY: the object's connection, if any.
    let Some(conn) = (unsafe { conn_ptr.as_mut() }).filter(|conn| {
        // SAFETY: a live client.
        unsafe { native::swoole_client_has_socket(conn.cli) }
    }) else {
        warn(ErrorKind::Fatal, "client is not connected to the server");
        return call.set_bool(false);
    };
    if conn.flag(state::CLOSED) {
        warn(ErrorKind::Error, "client socket is closed");
        return call.set_bool(false);
    }
    // SAFETY: a live client.
    if force
        || !conn.keep
        // SAFETY: a live client.
        || unsafe { native::swoole_client_catch_error(conn.cli, last_error()) } == CLOSE
    {
        let Some(result) = bailed(conn.call(Op::Close).run()) else {
            return call.set_bool(false);
        };
        bailed(release(object, conn_ptr));
        return call.set_bool(result >= 0);
    }
    let handle = pool();
    // SAFETY: the live pool; the connection moves into it.
    if unsafe {
        pool::swr_v1_clients_pool_push(handle, conn.key.as_ptr(), conn.key.len(), conn_ptr.cast())
    } != 0
    {
        std::process::abort();
    }
    this(call).conn = ptr::null_mut();
    call.set_bool(true);
}

/// `php_swoole_client_enable_ssl_encryption`.
pub(crate) fn enable_ssl(call: &Call, conn: &mut Conn) -> bool {
    if !matches!(conn.socket_type(), SOCK_TCP | SOCK_TCP6) {
        warn(ErrorKind::Fatal, "cannot use enableSSL");
        return false;
    }
    if conn.has_ssl() {
        warn(ErrorKind::Fatal, "SSL has been enabled");
        return false;
    }
    let mut enable = conn.call(Op::EnableSsl);
    if let Some(zset) = settings(call.this()) {
        enable.settings = zset;
    }
    bailed(enable.run()).is_some_and(|result| result == 0)
}

/// `enableSSL(?callable $onSslReady = null): bool`.
fn enable_ssl_method(call: &Call) {
    if call.arity(0, 1).is_err() {
        return call.set_bool(false);
    }
    if !call.zval(1).is_null() {
        // SAFETY: from a handler, with Swoole's exception class.
        unsafe {
            throw_exception(
                EXCEPTION_PARENT.load(Ordering::Acquire),
                c"sync client does not support `onSslReady` callback",
                i64::from(INVALID_PARAMS),
            );
        }
        return call.set_bool(false);
    }
    let Some(conn) = connected(call) else {
        return call.set_bool(false);
    };
    if !enable_ssl(call, conn) {
        return call.set_bool(false);
    }
    call.set_bool(bailed(conn.call(Op::SslHandshake).run()) == Some(0));
}

/// `swoole_exception_ce`.
pub(crate) static EXCEPTION_PARENT: AtomicPtr<zend_class_entry> = AtomicPtr::new(ptr::null_mut());

/// `getPeerCert(): string|false`.
fn get_peer_cert(call: &Call) {
    let Some(conn) = connected(call) else {
        return call.set_bool(false);
    };
    if !conn.has_ssl() {
        warn(ErrorKind::Fatal, "SSL is not ready");
        return call.set_bool(false);
    }
    // SAFETY: a live client with TLS.
    let cert = unsafe { native::swoole_client_peer_cert(conn.cli) };
    if cert.is_null() {
        return call.set_bool(false);
    }
    // SAFETY: a fresh string whose reference the return slot takes.
    unsafe { call.set_string(cert) };
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
    let mut verify = conn.call(Op::SslVerify);
    verify.a = i64::from(allow);
    call.set_bool(bailed(verify.run()).is_some_and(|result| result >= 0));
}

/// `shutdown(int $how): bool`.
fn shutdown(call: &Call) {
    let Some(conn) = connected(call) else {
        return call.set_bool(false);
    };
    let Ok(how) = call.arity(1, 1).and_then(|()| call.long(1, 0)) else {
        return call.set_bool(false);
    };
    let mut shutdown = conn.call(Op::Shutdown);
    shutdown.a = how;
    call.set_bool(bailed(shutdown.run()).is_some_and(|result| result >= 0));
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
