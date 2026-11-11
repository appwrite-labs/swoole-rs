//! `Swoole\Coroutine\Socket` and its exception: registration, the socket
//! constants, the object and its methods (`getOption`, `setOption` and `import`
//! are in `socket_options`).
//! Each method parses as upstream's ZPP block does, then makes one call into the
//! C++ socket (`crate::native`).
use crate::{
    constants,
    native::{self, Co, Socket, Transfer},
};
use libc::{c_int, iovec};
use std::{
    ffi::{CStr, CString},
    ptr,
    sync::atomic::{AtomicUsize, Ordering},
};
use swoole_rs_phpstubs::socket_coro;
use swoole_rs_phpswoole::{Switch, built_with};
use swoole_rs_zend::{
    Array, Call, Class, Invalid, Minit, Object, OwnedArray, Value, ZString,
    bailout::Bailout,
    set_object,
    string::ZBuf,
    sys::{
        E_WARNING, ZEND_ACC_NOT_SERIALIZABLE, ZEND_ACC_PUBLIC, ZEND_ACC_READONLY, module_registry,
        zend_class_entry, zend_get_type_by_const, zend_hash_str_find, zend_object,
        zend_object_handlers, zval,
    },
    unset_property_deny, value,
};
use swoole_rs_zend::{method_table, sys as z};

/// `SW_BUFFER_SIZE_BIG`.
const BUFFER_SIZE_BIG: i64 = 65536;
/// `SW_BACKLOG`.
const BACKLOG: i64 = 512;
#[path = "socket_owner.rs"]
pub(crate) mod owner;

/// `SW_EVENT_READ` and `SW_EVENT_WRITE`.
const EVENT_READ: i64 = 1 << 9;
const EVENT_WRITE: i64 = 1 << 10;
/// `IOV_MAX` on Linux and Apple.
const IOV_MAX: u32 = 1024;

/// Rust owns the core pointer and imported stream. Native SDK
/// facades access them only through the typed Rust entry points below.
#[repr(C)]
pub(crate) struct Data {
    socket: Socket,
    stream: zval,
}

type SocketObject = Object<Data>;

static SOCKET: Class = Class::new();
static EXCEPTION: Class = Class::new();

unsafe extern "C" fn create(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: Zend creates this class or a subclass with zero-initializable data.
    let object = match unsafe { SocketObject::try_create(&SOCKET, ce) } {
        Ok(object) => object,
        Err(error) => {
            error.defer();
            ptr::null_mut()
        }
    };
    swoole_rs_zend::bailout::finish();
    object
}

/// Upstream does not close here: the engine may be shutting down, and closing
/// would resume other coroutines.
unsafe extern "C" fn free(std: *mut zend_object) {
    // SAFETY: Zend transfers the object's core/resource custody to this final release.
    let released = unsafe { owner::release(std) };
    let result = released.and_then(|()| {
        // SAFETY: core custody has finished; now run the object's property destructors.
        unsafe { SocketObject::try_dtor(std) }
    });
    swoole_rs_zend::bailout::defer(result);
    swoole_rs_zend::bailout::finish();
}

fn configure(entry: &mut zend_class_entry, handlers: &mut zend_object_handlers) {
    entry.ce_flags |= ZEND_ACC_NOT_SERIALIZABLE;
    handlers.clone_obj = None;
    handlers.unset_property = Some(unset_property_deny);
    SocketObject::install(entry, handlers, create, free);
}

method_table! {
    static METHODS = [
        c"__construct" => zim_swoole_socket_coro___construct, &socket_coro::arginfo_class_Swoole_Coroutine_Socket___construct, z::ZEND_ACC_PUBLIC;
        c"bind" => zim_swoole_socket_coro_bind, &socket_coro::arginfo_class_Swoole_Coroutine_Socket_bind, z::ZEND_ACC_PUBLIC;
        c"listen" => zim_swoole_socket_coro_listen, &socket_coro::arginfo_class_Swoole_Coroutine_Socket_listen, z::ZEND_ACC_PUBLIC;
        c"accept" => zim_swoole_socket_coro_accept, &socket_coro::arginfo_class_Swoole_Coroutine_Socket_accept, z::ZEND_ACC_PUBLIC;
        c"connect" => zim_swoole_socket_coro_connect, &socket_coro::arginfo_class_Swoole_Coroutine_Socket_connect, z::ZEND_ACC_PUBLIC;
        c"checkLiveness" => zim_swoole_socket_coro_checkLiveness, &socket_coro::arginfo_class_Swoole_Coroutine_Socket_checkLiveness, z::ZEND_ACC_PUBLIC;
        c"getBoundCid" => zim_swoole_socket_coro_getBoundCid, &socket_coro::arginfo_class_Swoole_Coroutine_Socket_getBoundCid, z::ZEND_ACC_PUBLIC;
        c"peek" => zim_swoole_socket_coro_peek, &socket_coro::arginfo_class_Swoole_Coroutine_Socket_peek, z::ZEND_ACC_PUBLIC;
        c"recv" => zim_swoole_socket_coro_recv, &socket_coro::arginfo_class_Swoole_Coroutine_Socket_recv, z::ZEND_ACC_PUBLIC;
        c"recvAll" => zim_swoole_socket_coro_recvAll, &socket_coro::arginfo_class_Swoole_Coroutine_Socket_recvAll, z::ZEND_ACC_PUBLIC;
        c"recvLine" => zim_swoole_socket_coro_recvLine, &socket_coro::arginfo_class_Swoole_Coroutine_Socket_recvLine, z::ZEND_ACC_PUBLIC;
        c"recvWithBuffer" => zim_swoole_socket_coro_recvWithBuffer, &socket_coro::arginfo_class_Swoole_Coroutine_Socket_recvWithBuffer, z::ZEND_ACC_PUBLIC;
        c"recvPacket" => zim_swoole_socket_coro_recvPacket, &socket_coro::arginfo_class_Swoole_Coroutine_Socket_recvPacket, z::ZEND_ACC_PUBLIC;
        c"send" => zim_swoole_socket_coro_send, &socket_coro::arginfo_class_Swoole_Coroutine_Socket_send, z::ZEND_ACC_PUBLIC;
        c"readVector" => zim_swoole_socket_coro_readVector, &socket_coro::arginfo_class_Swoole_Coroutine_Socket_readVector, z::ZEND_ACC_PUBLIC;
        c"readVectorAll" => zim_swoole_socket_coro_readVectorAll, &socket_coro::arginfo_class_Swoole_Coroutine_Socket_readVectorAll, z::ZEND_ACC_PUBLIC;
        c"writeVector" => zim_swoole_socket_coro_writeVector, &socket_coro::arginfo_class_Swoole_Coroutine_Socket_writeVector, z::ZEND_ACC_PUBLIC;
        c"writeVectorAll" => zim_swoole_socket_coro_writeVectorAll, &socket_coro::arginfo_class_Swoole_Coroutine_Socket_writeVectorAll, z::ZEND_ACC_PUBLIC;
        c"sendFile" => zim_swoole_socket_coro_sendFile, &socket_coro::arginfo_class_Swoole_Coroutine_Socket_sendFile, z::ZEND_ACC_PUBLIC;
        c"sendAll" => zim_swoole_socket_coro_sendAll, &socket_coro::arginfo_class_Swoole_Coroutine_Socket_sendAll, z::ZEND_ACC_PUBLIC;
        c"recvfrom" => zim_swoole_socket_coro_recvfrom, &socket_coro::arginfo_class_Swoole_Coroutine_Socket_recvfrom, z::ZEND_ACC_PUBLIC;
        c"sendto" => zim_swoole_socket_coro_sendto, &socket_coro::arginfo_class_Swoole_Coroutine_Socket_sendto, z::ZEND_ACC_PUBLIC;
        c"getOption" => crate::socket_options::zim_swoole_socket_coro_getOption, &socket_coro::arginfo_class_Swoole_Coroutine_Socket_getOption, z::ZEND_ACC_PUBLIC;
        c"setProtocol" => zim_swoole_socket_coro_setProtocol, &socket_coro::arginfo_class_Swoole_Coroutine_Socket_setProtocol, z::ZEND_ACC_PUBLIC;
        c"setOption" => crate::socket_options::zim_swoole_socket_coro_setOption, &socket_coro::arginfo_class_Swoole_Coroutine_Socket_setOption, z::ZEND_ACC_PUBLIC;
        c"sslHandshake" => zim_swoole_socket_coro_sslHandshake, &socket_coro::arginfo_class_Swoole_Coroutine_Socket_sslHandshake, z::ZEND_ACC_PUBLIC;
        c"shutdown" => zim_swoole_socket_coro_shutdown, &socket_coro::arginfo_class_Swoole_Coroutine_Socket_shutdown, z::ZEND_ACC_PUBLIC;
        c"cancel" => zim_swoole_socket_coro_cancel, &socket_coro::arginfo_class_Swoole_Coroutine_Socket_cancel, z::ZEND_ACC_PUBLIC;
        c"close" => zim_swoole_socket_coro_close, &socket_coro::arginfo_class_Swoole_Coroutine_Socket_close, z::ZEND_ACC_PUBLIC;
        c"getpeername" => zim_swoole_socket_coro_getpeername, &socket_coro::arginfo_class_Swoole_Coroutine_Socket_getpeername, z::ZEND_ACC_PUBLIC;
        c"getsockname" => zim_swoole_socket_coro_getsockname, &socket_coro::arginfo_class_Swoole_Coroutine_Socket_getsockname, z::ZEND_ACC_PUBLIC;
        c"isClosed" => zim_swoole_socket_coro_isClosed, &socket_coro::arginfo_class_Swoole_Coroutine_Socket_isClosed, z::ZEND_ACC_PUBLIC;
        c"import" => crate::socket_options::zim_swoole_socket_coro_import, &socket_coro::arginfo_class_Swoole_Coroutine_Socket_import, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
    ];
}

/// `swoole_socket_coro_ce`: `Swoole\Coroutine\Socket`'s class entry, null
/// before MINIT.
#[must_use]
pub fn socket_class() -> *mut zend_class_entry {
    SOCKET.entry()
}

/// `Swoole\Coroutine\Socket`.
#[must_use]
pub fn socket() -> &'static Class {
    &SOCKET
}

swoole_rs_zend::minit!(socket_minit => register);

/// MINIT, as `php_swoole_socket_coro_minit` did, after `Swoole\Exception`: both
/// classes, their short names when `shortname`, the properties (with the
/// `ext/sockets` bridge ones when the build has it), and the socket constants
/// unless `ext/sockets` is loaded (the `AI_*` ones that configure found).
fn register(minit: &Minit) -> Result<(), Bailout> {
    let shortname = swoole_rs_phpswoole::globals().use_shortname.get();
    minit.register(&SOCKET, c"Swoole\\Coroutine\\Socket", &METHODS.0, configure);
    if shortname {
        minit.alias(&SOCKET, c"Co\\Socket")?;
    }
    let readonly = ZEND_ACC_PUBLIC | ZEND_ACC_READONLY;
    for name in [c"fd", c"domain", c"type", c"protocol"] {
        let default = if name == c"fd" { -1 } else { 0 };
        minit.property_long(&SOCKET, name, default, readonly);
    }
    minit.property_long(&SOCKET, c"errCode", 0, ZEND_ACC_PUBLIC);
    minit.property_string(&SOCKET, c"errMsg", c"", ZEND_ACC_PUBLIC);
    if built_with(Switch::Sockets) {
        minit.property_bool(&SOCKET, c"__ext_sockets_nonblock", false, ZEND_ACC_PUBLIC);
        minit.property_long(&SOCKET, c"__ext_sockets_timeout", 0, ZEND_ACC_PUBLIC);
    }
    minit.register_child(
        &EXCEPTION,
        c"Swoole\\Coroutine\\Socket\\Exception",
        &[],
        minit.parent(swoole_rs_phpswoole::exception()),
        |_, _| {},
    );
    if shortname {
        minit.alias(&EXCEPTION, c"Co\\Socket\\Exception")?;
    }
    // SAFETY: the module registry is complete before any MINIT runs.
    let loaded = unsafe { zend_hash_str_find(&raw const module_registry, c"sockets".as_ptr(), 7) };
    if loaded.is_null() {
        register_constants(minit);
    }
    for (name, value) in constants::ALWAYS {
        minit.long_constant(name, i64::from(*value));
    }
    Ok(())
}

/// `socket_coro_register_constants`.
fn register_constants(minit: &Minit) {
    let ai = [
        built_with(Switch::AiV4mapped).then_some(constants::AI_V4MAPPED),
        built_with(Switch::AiAll).then_some(constants::AI_ALL),
        Some(constants::AI_ADDRCONFIG),
    ];
    let idn = if built_with(Switch::AiIdn) {
        constants::AI_IDN
    } else {
        &[][..]
    };
    let all = constants::SOCKET
        .iter()
        .chain(constants::MULTICAST)
        .chain(constants::ERRNO)
        .copied()
        .chain(ai.into_iter().flatten())
        .chain(idn.iter().copied())
        .chain([constants::AI_NUMERICSERV]);
    for (name, value) in all {
        minit.long_constant(name, i64::from(value));
    }
}

/// `php_swoole_get_socket`: the C++ socket of a `Coroutine\Socket` object.
/// # Safety
/// `object` is a live `Coroutine\Socket` object.
pub unsafe fn socket_of(object: *mut zend_object) -> Socket {
    // SAFETY: per the caller.
    unsafe { SocketObject::data(object) }.socket
}

/// `$this`'s data.
pub(crate) fn this<'a>(call: &Call) -> &'a mut Data {
    // SAFETY: socket methods are non-static, so `$this` is a socket object, and
    // no other reference to its data is live during the call.
    unsafe { SocketObject::data(call.this()) }
}

/// `swoole_get_socket_coro`: the constructed, open socket of `$this`; else
/// errCode/errMsg say EBADF and the method returns false.
pub(crate) fn open(call: &Call) -> Option<Co> {
    let socket = constructed(call)?;
    if socket.is_closed() {
        set_error(call, libc::EBADF, strerror(libc::EBADF));
        call.set_bool(false);
        return None;
    }
    Some(socket)
}

/// `strerror(code)`.
pub(crate) fn strerror<'a>(code: c_int) -> &'a CStr {
    // SAFETY: a pure query; the text stays valid until the next call.
    let text = unsafe { libc::strerror(code) };
    // SAFETY: strerror returns a NUL-terminated string.
    unsafe { CStr::from_ptr(text) }
}

/// The constructed socket of `$this`; else a fatal error, and the method
/// returns false.
fn constructed(call: &Call) -> Option<Co> {
    let socket = this(call).socket;
    if socket.is_null() {
        // SAFETY: a fatal error; the handler owns nothing it skips.
        unsafe { native::unconstructed() };
        call.set_bool(false);
        return None;
    }
    // SAFETY: a constructed socket stays live while its object is.
    Some(unsafe { Co::new(socket) })
}

/// errCode and errMsg.
fn set_error(call: &Call, code: c_int, message: &CStr) {
    // SAFETY: the registered class and current method object remain live through both property updates.
    swoole_rs_zend::bailout::defer(unsafe {
        owner::error_properties(SOCKET.entry(), call.this(), code, message)
    });
}

/// `socket_coro_sync_properties`: errCode and errMsg from the socket.
fn sync(call: &Call, socket: Co) {
    let (code, message) = socket.error();
    set_error(call, code, message);
}

/// `php_swoole_error(E_WARNING, ...)`, or with `fatal` `php_swoole_fatal_error`.
pub(crate) fn warn(fatal: bool, message: &str) {
    let message = CString::new(message).unwrap_or_default();
    #[expect(clippy::cast_possible_wrap, reason = "E_WARNING is 2")]
    // SAFETY: C++'s reporter, from a handler.
    swoole_rs_zend::bailout::defer(unsafe {
        swoole_rs_zend::bailout::report_flag(
            native::swoole_rs_php_error,
            E_WARNING as c_int,
            fatal,
            &message,
        )
    });
}

/// `zend_throw_exception_ex(swoole_socket_coro_exception_ce, code, ...)`.
fn throw(code: c_int, message: &str) {
    let message = CString::new(message).unwrap_or_default();
    // SAFETY: the registered exception and owned diagnostic string remain live through the trapped throw.
    if unsafe { z::swr_zend_try_facade_throw(EXCEPTION.entry(), message.as_ptr(), i64::from(code)) }
        != 0
    {
        Bailout.defer();
    }
}

/// `swoole_pagesize()`.
fn pagesize() -> usize {
    static SIZE: AtomicUsize = AtomicUsize::new(0);
    let size = SIZE.load(Ordering::Relaxed);
    if size != 0 {
        return size;
    }
    // SAFETY: a pure query.
    let size = usize::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).unwrap_or(4096);
    SIZE.store(size, Ordering::Relaxed);
    size
}

/// `sw_zend_string_recycle`: end the buffer at its written bytes, giving the
/// memory back when it wasted more than half of a buffer over a page.
fn recycle(mut buffer: ZBuf) -> ZString {
    let (allocated, used) = (buffer.capacity(), buffer.len());
    if allocated != used && allocated > pagesize() && allocated > used * 2 {
        buffer.shrink_to_fit();
    }
    buffer.into_zstring()
}

/// Return what a transfer of `bytes` wrote into `buffer`: false when
/// negative, '' when zero.
fn set_received(call: &Call, buffer: ZBuf, bytes: isize) {
    match bytes {
        1.. => call.set_zstring(recycle(buffer)),
        0 => call.set_empty_string(),
        _ => call.set_bool(false),
    }
}

/// `__construct(int $domain, int $type, int $protocol = IPPROTO_IP)`.
fn construct(call: &Call) {
    let parsed = call.arity(2, 3).and_then(|()| {
        Ok((
            call.long(1, 0)?,
            call.long(2, 0)?,
            call.long(3, i64::from(libc::IPPROTO_IP))?,
        ))
    });
    let Ok((domain, kind, protocol)) = parsed else {
        return call.set_bool(false);
    };
    if !this(call).socket.is_null() {
        call.throw_constructed_twice();
        return call.set_bool(false);
    }
    // SAFETY: the native constructor arguments are parsed and no mutable object borrow spans effects.
    let (socket, error) = match unsafe { owner::construct_core(domain, kind, protocol) } {
        Ok(result) => result,
        Err(error) => return error.defer(),
    };
    if socket.is_null() {
        let text = strerror(error).to_string_lossy();
        throw(
            error,
            &format!("new Socket() failed. Error: {text} [{error}]"),
        );
        return call.set_bool(false);
    }
    this(call).socket = socket;
    // SAFETY: `$this` now holds its socket.
    swoole_rs_zend::bailout::defer(unsafe { init(call.this()) });
}

/// `socket_coro_init`: zero copy and the zend string allocator, then the fd,
/// domain, type and protocol properties.
/// # Safety
/// `object` is a live socket object holding its socket.
unsafe fn init(object: *mut zend_object) -> Result<(), Bailout> {
    // SAFETY: per the caller.
    let socket = unsafe { SocketObject::data(object) }.socket;
    // SAFETY: as above.
    let socket = unsafe { Co::new(socket) };
    let fields = socket.adopt();
    for (name, field) in [c"fd", c"domain", c"type", c"protocol"]
        .into_iter()
        .zip(fields)
    {
        // SAFETY: a live instance of the registered class; each update contains PHP bailouts.
        unsafe { owner::update_long(SOCKET.entry(), object, name, field) }?;
    }
    Ok(())
}

/// `php_swoole_init_socket_object(zobject, socket)`: a `Coroutine\Socket`
/// object that owns `socket`, as upstream's coroutine HTTP server makes for
/// each connection.
/// # Errors
/// Making the object bailed out; `zobject` is untouched.
/// # Safety
/// `zobject` is writable and `socket` a live coroutine socket; called after
/// MINIT, from PHP's thread.
pub unsafe fn init_socket_object(zobject: *mut zval, socket: Socket) -> Result<(), Bailout> {
    // SAFETY: per the caller.
    let object = unsafe { owner::make_object(socket) }?;
    // SAFETY: per the caller.
    unsafe { set_object(zobject, object) };
    Ok(())
}

/// `bind(string $address, int $port = 0): bool`.
fn bind(call: &Call) {
    let parsed = call
        .arity(1, 2)
        .and_then(|()| Ok((call.string(1, b"")?, call.long(2, 0)?)));
    let Ok((address, port)) = parsed else {
        return call.set_bool(false);
    };
    let Some(socket) = open(call) else { return };
    if socket.bind(address, port) {
        call.set_bool(true);
    } else {
        sync(call, socket);
        call.set_bool(false);
    }
}

/// `listen(int $backlog = SW_BACKLOG): bool`.
fn listen(call: &Call) {
    let Ok(backlog) = call.arity(0, 1).and_then(|()| call.long(1, BACKLOG)) else {
        return call.set_bool(false);
    };
    let Some(socket) = open(call) else { return };
    if socket.listen(backlog) {
        call.set_bool(true);
    } else {
        sync(call, socket);
        call.set_bool(false);
    }
}

/// `accept(float $timeout = 0): Socket|false`.
fn accept(call: &Call) {
    let Ok(timeout) = call.arity(0, 1).and_then(|()| call.double(1, 0.0)) else {
        return call.set_bool(false);
    };
    let Some(socket) = open(call) else { return };
    let connection = socket.accept(timeout);
    if connection.is_null() {
        sync(call, socket);
        return call.set_bool(false);
    }
    // SAFETY: a new object of the registered class.
    let client = match unsafe { SocketObject::try_create(&SOCKET, SOCKET.entry()) } {
        Ok(client) => client,
        Err(error) => return error.defer(),
    };
    // SAFETY: the object just created, not yet shared.
    unsafe { SocketObject::data(client) }.socket = connection;
    // SAFETY: the return slot takes the new object's only reference.
    unsafe { set_object(call.return_value(), client) };
    // SAFETY: the object holds its socket.
    swoole_rs_zend::bailout::defer(unsafe { init(client) });
}

/// `connect(string $host, int $port = 0, float $timeout = 0): bool`.
fn connect(call: &Call) {
    let parsed = call
        .arity(1, 3)
        .and_then(|()| Ok((call.string(1, b"")?, call.long(2, 0)?, call.double(3, 0.0)?)));
    let Ok((host, port, timeout)) = parsed else {
        return call.set_bool(false);
    };
    let Some(socket) = open(call) else { return };
    if socket.port_required() {
        if call.num_args() == 1 {
            warn(
                false,
                "Socket of type AF_INET/AF_INET6 requires port argument",
            );
            return call.set_bool(false);
        } else if port == 0 || port >= 65536 {
            warn(false, &format!("Invalid port argument[{port}]"));
            return call.set_bool(false);
        }
    }
    if socket.connect(host, port, timeout) {
        call.set_bool(true);
    } else {
        sync(call, socket);
        call.set_bool(false);
    }
}

/// `checkLiveness(): bool`.
fn check_liveness(call: &Call) {
    let Some(socket) = open(call) else { return };
    let alive = socket.check_liveness();
    sync(call, socket);
    call.set_bool(alive);
}

/// `getBoundCid(int $event): int`.
fn bound_cid(call: &Call) {
    let Ok(event) = call.arity(1, 1).and_then(|()| call.long(1, 0)) else {
        return call.set_bool(false);
    };
    let Some(socket) = open(call) else { return };
    call.set_long(socket.bound_cid(event));
}

/// A length argument: non-positive means `SW_BUFFER_SIZE_BIG`.
fn buffer_length(length: i64) -> usize {
    usize::try_from(if length <= 0 { BUFFER_SIZE_BIG } else { length }).unwrap_or(usize::MAX)
}

/// `peek(int $length = 65536): string|false`.
fn peek(call: &Call) {
    let Ok(length) = call
        .arity(0, 1)
        .and_then(|()| call.long(1, BUFFER_SIZE_BIG))
    else {
        return call.set_bool(false);
    };
    let length = buffer_length(length);
    let Some(socket) = open(call) else { return };
    receive(call, socket, Transfer::Peek, length, 0.0);
}

/// Fill a new string of `length` bytes with one transfer and return it.
fn receive(call: &Call, socket: Co, op: Transfer, length: usize, timeout: f64) {
    let mut buffer = ZBuf::with_capacity(length);
    let bytes = socket.receive(op, &mut buffer, timeout);
    sync(call, socket);
    set_received(call, buffer, bytes);
}

/// `recv`, `recvAll`, `recvLine` and `recvWithBuffer`:
/// `(int $length = 65536, float $timeout = 0): string|false`.
fn recv_as(call: &Call, op: Transfer) {
    let parsed = call
        .arity(0, 2)
        .and_then(|()| Ok((call.long(1, BUFFER_SIZE_BIG)?, call.double(2, 0.0)?)));
    let Ok((length, timeout)) = parsed else {
        return call.set_bool(false);
    };
    let length = buffer_length(length);
    let Some(socket) = open(call) else { return };
    receive(call, socket, op, length, timeout);
}

fn recv(call: &Call) {
    recv_as(call, Transfer::Recv);
}

fn recv_all(call: &Call) {
    recv_as(call, Transfer::RecvAll);
}

fn recv_line(call: &Call) {
    recv_as(call, Transfer::RecvLine);
}

fn recv_with_buffer(call: &Call) {
    recv_as(call, Transfer::RecvWithBuffer);
}

/// `recvPacket(float $timeout = 0): string|false`.
fn recv_packet(call: &Call) {
    let Ok(timeout) = call.arity(0, 1).and_then(|()| call.double(1, 0.0)) else {
        return call.set_bool(false);
    };
    let Some(socket) = open(call) else { return };
    let bytes = socket.recv_packet(timeout, true);
    sync(call, socket);
    let Ok(length) = usize::try_from(bytes) else {
        return call.set_bool(false);
    };
    if length == 0 {
        return call.set_empty_string();
    }
    match socket.pop_packet(length) {
        Some(packet) => call.set_zstring(packet),
        None => call.set_bool(false),
    }
}

/// `send` and `sendAll`: `(string $data, float $timeout = 0): int|false`.
fn send_as(call: &Call, op: Transfer) {
    let parsed = call
        .arity(1, 2)
        .and_then(|()| Ok((call.string(1, b"")?, call.double(2, 0.0)?)));
    let Ok((data, timeout)) = parsed else {
        return call.set_bool(false);
    };
    let Some(socket) = open(call) else { return };
    let sent = socket.send(op, data, timeout);
    sync(call, socket);
    if sent < 0 {
        call.set_bool(false);
    } else {
        call.set_long(sent as i64);
    }
}

fn send(call: &Call) {
    send_as(call, Transfer::Send);
}

fn send_all(call: &Call) {
    send_as(call, Transfer::SendAll);
}

/// The vectors argument: its element count, or false (with errCode EINVAL)
/// past `IOV_MAX`.
fn vector_count(socket: Co, array: &Array) -> Option<usize> {
    let count = array.count();
    if count > IOV_MAX {
        let message = format!("The maximum of iov count is {IOV_MAX}");
        socket.set_error(libc::EINVAL, message.as_bytes());
        return None;
    }
    usize::try_from(count).ok()
}

/// `zend_get_type_by_const(Z_TYPE_P(item))`.
fn type_name(item: &Value) -> String {
    #[expect(clippy::cast_possible_wrap, reason = "zval types are below 32")]
    // SAFETY: Zend returns a static name or null.
    let name = unsafe { zend_get_type_by_const(item.kind() as c_int) };
    if name.is_null() {
        return String::new();
    }
    // SAFETY: a static NUL-terminated name.
    unsafe { CStr::from_ptr(name) }
        .to_string_lossy()
        .into_owned()
}

/// `(array $io_vector, float $timeout = 0)`.
fn vector_arguments(call: &Call) -> Result<(&Array, f64), Invalid> {
    call.arity(1, 2)?;
    Ok((call.array_ref(1)?, call.double(2, 0.0)?))
}

/// `writeVector` and `writeVectorAll`: `(array $io_vector, float $timeout = 0): int|false`.
fn write_vector_as(call: &Call, op: c_int) {
    let Ok((array, timeout)) = vector_arguments(call) else {
        return call.set_bool(false);
    };
    let Some(socket) = open(call) else { return };
    write_vector(call, socket, op, array, timeout);
    sync(call, socket);
}

fn write_vector(call: &Call, socket: Co, op: c_int, array: &Array, timeout: f64) {
    let Some(count) = vector_count(socket, array) else {
        return call.set_bool(false);
    };
    let mut vectors = Vec::with_capacity(count);
    for (index, (_, item)) in array.entries().enumerate() {
        let Some(bytes) = item.bytes() else {
            let given = type_name(item);
            throw(
                libc::EINVAL,
                &format!("Item #[{index}] must be of type string, {given} given"),
            );
            return call.set_bool(false);
        };
        if bytes.is_empty() {
            throw(
                libc::EINVAL,
                &format!("Item #[{index}] cannot be empty string"),
            );
            return call.set_bool(false);
        }
        vectors.push(iovec {
            iov_base: bytes.as_ptr().cast_mut().cast(),
            iov_len: bytes.len(),
        });
    }
    // SAFETY: the writes only read the elements' bytes.
    let (written, _, _) = unsafe { socket.vector(op, &vectors, timeout) };
    if written < 0 {
        call.set_bool(false);
    } else {
        call.set_long(written as i64);
    }
}

fn write_vector_once(call: &Call) {
    write_vector_as(call, 2);
}

fn write_vector_all(call: &Call) {
    write_vector_as(call, 3);
}

/// `readVector` and `readVectorAll`: `(array $io_vector, float $timeout = 0): array|false`.
fn read_vector_as(call: &Call, op: c_int) {
    let Ok((array, timeout)) = vector_arguments(call) else {
        return call.set_bool(false);
    };
    let Some(socket) = open(call) else { return };
    read_vector(call, socket, op, array, timeout);
    sync(call, socket);
}

fn read_vector(call: &Call, socket: Co, op: c_int, array: &Array, timeout: f64) {
    let Some(count) = vector_count(socket, array) else {
        return call.set_bool(false);
    };
    let mut buffers: Vec<ZBuf> = Vec::with_capacity(count);
    for (index, (_, item)) in array.entries().enumerate() {
        let Some(length) = item.as_long() else {
            let given = type_name(item);
            throw(
                libc::EINVAL,
                &format!("Item #[{index}] must be of type int, {given} given"),
            );
            return call.set_bool(false);
        };
        let Ok(length) = usize::try_from(length) else {
            throw(
                libc::EINVAL,
                &format!("Item #[{index}] must be greater than 0"),
            );
            return call.set_bool(false);
        };
        buffers.push(ZBuf::with_capacity(length));
    }
    let total: usize = buffers.iter().map(ZBuf::capacity).sum();
    let vectors: Vec<iovec> = buffers
        .iter_mut()
        .map(|buffer| iovec {
            iov_base: buffer.as_mut_ptr().cast(),
            iov_len: buffer.capacity(),
        })
        .collect();
    // SAFETY: unshared buffers of the listed lengths.
    let (read, stopped, offset) = unsafe { socket.vector(op, &vectors, timeout) };
    let read = match usize::try_from(read) {
        Ok(0) => return call.set_empty_array(),
        Err(_) => return call.set_bool(false),
        Ok(read) => read,
    };
    // A short read stopped `offset` bytes into vector `stopped`: keep that much
    // of it and drop the vectors after it.
    let stopped = usize::try_from(stopped).unwrap_or(0);
    let stopped = (read < total && stopped < buffers.len()).then_some(stopped);
    buffers.truncate(stopped.map_or(buffers.len(), |stopped| stopped + 1));
    let mut result = OwnedArray::default();
    for (index, mut buffer) in buffers.into_iter().enumerate() {
        let len = if stopped == Some(index) {
            offset.min(buffer.capacity())
        } else {
            buffer.capacity()
        };
        // SAFETY: the read filled each vector before the one it stopped in.
        unsafe { buffer.set_len(len) };
        result.push(recycle(buffer).into());
    }
    call.set_owned(result.into_owned());
}

fn read_vector_once(call: &Call) {
    read_vector_as(call, 0);
}

fn read_vector_all(call: &Call) {
    read_vector_as(call, 1);
}

/// `sendFile(string $file, int $offset = 0, int $length = 0): bool`.
fn send_file(call: &Call) {
    let parsed = call
        .arity(1, 3)
        .and_then(|()| Ok((call.string(1, b"")?, call.long(2, 0)?, call.long(3, 0)?)));
    let Ok((file, offset, length)) = parsed else {
        return call.set_bool(false);
    };
    if file.is_empty() {
        warn(true, "file to send is empty");
        return call.set_bool(false);
    }
    let Some(socket) = open(call) else { return };
    // SAFETY: a string argument's bytes are NUL-terminated.
    if unsafe { socket.sendfile(file.as_ptr().cast(), offset, length) } {
        call.set_bool(true);
    } else {
        sync(call, socket);
        call.set_bool(false);
    }
}

/// `recvfrom(&$peername, float $timeout = 0): string|false`.
fn recvfrom(call: &Call) {
    let parsed = call.arity(1, 2).and_then(|()| call.double(2, 0.0));
    let Ok(timeout) = parsed else {
        return call.set_bool(false);
    };
    let Some(socket) = open(call) else { return };
    let length = buffer_length(BUFFER_SIZE_BIG);
    let mut buffer = ZBuf::with_capacity(length);
    let bytes = socket.recvfrom(&mut buffer, timeout);
    sync(call, socket);
    if bytes <= 0 {
        return set_received(call, buffer, bytes);
    }
    // SAFETY: nothing borrows argument 1.
    unsafe { call.assign(1, address(socket).into_owned()) };
    call.set_zstring(buffer.into_zstring());
}

/// `["address" => .., "port" => ..]` of the socket's last address.
fn address(socket: Co) -> OwnedArray {
    let (address, port) = socket.address();
    endpoint(&address, port)
}

/// `["address" => address, "port" => port]`.
fn endpoint(address: &CStr, port: c_int) -> OwnedArray {
    let mut array = OwnedArray::default();
    array.insert_bytes(b"address", Some(address.to_bytes()));
    array.insert_long(b"port", i64::from(port));
    array
}

/// `sendto(string $addr, int $port, string $data): int|false`.
fn sendto(call: &Call) {
    let parsed = call
        .arity(3, 3)
        .and_then(|()| Ok((call.string(1, b"")?, call.long(2, 0)?, call.string(3, b"")?)));
    let Ok((address, port, data)) = parsed else {
        return call.set_bool(false);
    };
    let Some(socket) = open(call) else { return };
    let sent = socket.sendto(address, port, data);
    sync(call, socket);
    if sent < 0 {
        call.set_bool(false);
    } else {
        call.set_long(sent as i64);
    }
}

/// `shutdown(int $how = SHUT_RDWR): bool`.
fn shutdown(call: &Call) {
    let Ok(how) = call
        .arity(0, 1)
        .and_then(|()| call.long(1, i64::from(libc::SHUT_RDWR)))
    else {
        return call.set_bool(false);
    };
    let Some(socket) = open(call) else { return };
    if socket.shutdown(how) {
        call.set_bool(true);
    } else {
        sync(call, socket);
        call.set_bool(false);
    }
}

/// `close(): bool`.
fn close(call: &Call) {
    if open(call).is_none() {
        return;
    }
    // SAFETY: the current PHP method keeps this open, owned socket object live.
    match unsafe { owner::close(call.this()) } {
        Ok(()) => call.set_bool(true),
        Err(error) => error.defer(),
    }
}

/// `getsockname(): array|false`.
fn getsockname(call: &Call) {
    let Some(socket) = open(call) else { return };
    if !socket.getsockname() {
        sync(call, socket);
        return call.set_bool(false);
    }
    call.set_owned(address(socket).into_owned());
}

/// `getpeername(): array|false`.
fn getpeername(call: &Call) {
    let Some(socket) = open(call) else { return };
    let Some((address, port)) = socket.getpeername() else {
        sync(call, socket);
        return call.set_bool(false);
    };
    call.set_owned(endpoint(&address, port).into_owned());
}

/// `cancel(int $event = SW_EVENT_READ): bool`; the socket is checked first.
fn cancel(call: &Call) {
    let Some(socket) = open(call) else { return };
    let Ok(event) = call.arity(0, 1).and_then(|()| call.long(1, EVENT_READ)) else {
        return call.set_bool(false);
    };
    let event = if event == EVENT_READ {
        EVENT_READ
    } else {
        EVENT_WRITE
    };
    call.set_bool(socket.cancel(c_int::try_from(event).unwrap_or(0)));
}

/// `setProtocol(array $settings): bool`; the socket is checked first.
fn set_protocol(call: &Call) {
    let Some(socket) = open(call) else { return };
    let Ok(settings) = call.arity(1, 1).and_then(|()| call.array_ref(1)) else {
        return call.set_bool(false);
    };
    if settings.count() == 0 {
        return call.set_bool(false);
    }
    call.set_bool(crate::socket_options::apply(
        socket,
        settings,
        crate::socket_options::PROTOCOL,
    ));
}

/// `sslHandshake(): bool`.
fn ssl_handshake(call: &Call) {
    let Some(socket) = open(call) else { return };
    call.set_bool(socket.ssl_handshake());
}

/// `isClosed(): bool`: unconstructed counts as closed.
fn is_closed(call: &Call) {
    let socket = this(call).socket;
    // SAFETY: a constructed socket is live.
    call.set_bool(socket.is_null() || unsafe { Co::new(socket) }.is_closed());
}

swoole_rs_zend::handlers! {
    zim_swoole_socket_coro___construct => construct;
    zim_swoole_socket_coro_bind => bind;
    zim_swoole_socket_coro_listen => listen;
    zim_swoole_socket_coro_accept => accept;
    zim_swoole_socket_coro_connect => connect;
    zim_swoole_socket_coro_checkLiveness => check_liveness;
    zim_swoole_socket_coro_getBoundCid => bound_cid;
    zim_swoole_socket_coro_peek => peek;
    zim_swoole_socket_coro_recv => recv;
    zim_swoole_socket_coro_recvAll => recv_all;
    zim_swoole_socket_coro_recvLine => recv_line;
    zim_swoole_socket_coro_recvWithBuffer => recv_with_buffer;
    zim_swoole_socket_coro_recvPacket => recv_packet;
    zim_swoole_socket_coro_send => send;
    zim_swoole_socket_coro_sendAll => send_all;
    zim_swoole_socket_coro_readVector => read_vector_once;
    zim_swoole_socket_coro_readVectorAll => read_vector_all;
    zim_swoole_socket_coro_writeVector => write_vector_once;
    zim_swoole_socket_coro_writeVectorAll => write_vector_all;
    zim_swoole_socket_coro_sendFile => send_file;
    zim_swoole_socket_coro_recvfrom => recvfrom;
    zim_swoole_socket_coro_sendto => sendto;
    zim_swoole_socket_coro_shutdown => shutdown;
    zim_swoole_socket_coro_close => close;
    zim_swoole_socket_coro_getsockname => getsockname;
    zim_swoole_socket_coro_getpeername => getpeername;
    zim_swoole_socket_coro_cancel => cancel;
    zim_swoole_socket_coro_setProtocol => set_protocol;
    zim_swoole_socket_coro_sslHandshake => ssl_handshake;
    zim_swoole_socket_coro_isClosed => is_closed;
}

/// A created object, or null. Null with a bailout pending is one the caller
/// stops on, leaving the raise to its handler's last step.
fn created(result: Result<*mut zend_object, Bailout>) -> *mut zend_object {
    result.unwrap_or_else(|error| {
        error.defer();
        ptr::null_mut()
    })
}

/// `php_swoole_create_socket(type)`: a new socket object of a native Swoole
/// socket kind, or null with `errno` or a bailout pending.
#[must_use]
pub fn create_socket(kind: i32) -> *mut zend_object {
    created(owner::create_kind(kind))
}

/// `php_swoole_create_socket_from_fd(fd, type)`, null as [`create_socket`].
/// # Safety
/// The caller gives up `fd`, which the new object owns.
#[must_use]
pub unsafe fn create_from_fd(fd: i32, kind: i32) -> *mut zend_object {
    // SAFETY: per the caller.
    created(unsafe { owner::from_fd(fd, kind) })
}

/// `php_swoole_dup_socket(fd, type)`: an object over a duplicate of `fd`,
/// which stays the caller's; null as [`create_socket`].
#[must_use]
pub fn dup(fd: i32, kind: i32) -> *mut zend_object {
    created(owner::duplicate(fd, kind))
}

/// `php_swoole_get_socket(zobject)`: the object's core socket, or null.
/// # Safety
/// `value` is a live `Coroutine\Socket` object zval.
#[must_use]
pub unsafe fn get_socket(value: *const zval) -> Socket {
    // SAFETY: per the caller.
    let object = unsafe { value::obj_of(value) };
    // SAFETY: a live object with the registered SocketObject layout.
    unsafe { socket_of(object) }
}

/// `php_swoole_socket_is_closed(zobject)`.
/// # Safety
/// As [`get_socket`], constructed or not.
#[must_use]
pub unsafe fn is_socket_closed(value: *const zval) -> bool {
    // SAFETY: per the caller.
    let socket = unsafe { get_socket(value) };
    // SAFETY: a non-null core socket stays live while its object is.
    socket.is_null() || unsafe { Co::new(socket) }.is_closed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_positive_lengths_read_a_big_buffer() {
        assert_eq!(buffer_length(0), 65536, "zero means SW_BUFFER_SIZE_BIG");
        assert_eq!(
            buffer_length(-5),
            65536,
            "negative means SW_BUFFER_SIZE_BIG"
        );
        assert_eq!(buffer_length(10), 10, "positive lengths are kept");
    }

    #[test]
    fn data_matches_socket_object() {
        // SocketImpl *, zval, then the zend_object.
        assert_eq!(
            SocketObject::OFFSET,
            size_of::<Socket>() + size_of::<zval>(),
            "std follows the socket and stream"
        );
    }
}
