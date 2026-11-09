//! `Swoole\Coroutine\Socket` and its exception: registration, the socket
//! constants, the object and its methods (`getOption`, `setOption` and `import`
//! are in `socket_options`).
//! Each method parses as upstream's ZPP block does, then makes one call into the
//! C++ socket (`crate::native`).
use crate::{
    constants,
    native::{self, Co, Socket, Transfer},
};
use libc::{c_char, c_int, iovec};
use std::{
    ffi::{CStr, CString},
    ptr,
    sync::atomic::{AtomicUsize, Ordering},
};
use swoole_rs_phpstubs::socket_coro;
use swoole_rs_phpswoole::{Switch, built_with};
use swoole_rs_zend::{
    Call, Class, Invalid, Object, declare_bool, declare_long, declare_string,
    register_long_constant, set_object, string,
    sys::{
        E_WARNING, HashPosition, IS_LONG, IS_STRING, ZEND_ACC_NOT_SERIALIZABLE, ZEND_ACC_PUBLIC,
        ZEND_ACC_READONLY, module_registry, zend_class_entry, zend_get_type_by_const,
        zend_hash_get_current_data_ex, zend_hash_internal_pointer_reset_ex,
        zend_hash_move_forward_ex, zend_hash_str_find, zend_object, zend_object_handlers,
        zend_string, zval, zval_ptr_dtor,
    },
    throw_constructed_twice, type_of, unset_property_deny, value,
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

/// `Swoole\Coroutine\Socket`'s class entry, set at MINIT.
#[unsafe(no_mangle)]
pub static mut swoole_socket_coro_ce: *mut zend_class_entry = ptr::null_mut();

/// MINIT, as `php_swoole_socket_coro_minit` did: both classes, their short
/// names when `shortname`, the properties (with the `ext/sockets` bridge ones
/// when the build has it), and the socket constants unless `ext/sockets` is
/// loaded (the `AI_*` ones that configure found).
/// # Safety
/// Called once, at MINIT of module `module`, with Swoole's exception class.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpclients_socket_minit(
    exception: *mut zend_class_entry,
    shortname: bool,
    module: c_int,
) {
    let sockets = built_with(Switch::Sockets);
    // SAFETY: the native MINIT entry lends these original startup arguments.
    let result = unsafe { init_phpclients_socket_minit(exception, shortname, sockets, module) };
    match result {
        Ok(value) => value,
        Err(error) => {
            error.defer();
            swoole_rs_zend::bailout::finish();
        }
    }
}

unsafe fn init_phpclients_socket_minit(
    exception: *mut zend_class_entry,
    shortname: bool,
    sockets: bool,
    module: c_int,
) -> Result<(), swoole_rs_zend::bailout::Bailout> {
    // SAFETY: MINIT, with a static table.
    unsafe { SOCKET.register(c"Swoole\\Coroutine\\Socket", METHODS.as_ptr(), configure) };
    let ce = SOCKET.entry();
    if shortname {
        // SAFETY: MINIT, with the class just registered.
        unsafe { SOCKET.alias(c"Co\\Socket") }?;
    }
    let readonly = ZEND_ACC_PUBLIC | ZEND_ACC_READONLY;
    for name in [c"fd", c"domain", c"type", c"protocol"] {
        let default = if name == c"fd" { -1 } else { 0 };
        // SAFETY: as above.
        unsafe { declare_long(ce, name, default, readonly) };
    }
    // SAFETY: as above.
    unsafe { declare_long(ce, c"errCode", 0, ZEND_ACC_PUBLIC) };
    // SAFETY: as above.
    unsafe { declare_string(ce, c"errMsg", c"", ZEND_ACC_PUBLIC) };
    if sockets {
        // SAFETY: as above.
        unsafe { declare_bool(ce, c"__ext_sockets_nonblock", false, ZEND_ACC_PUBLIC) };
        // SAFETY: as above.
        unsafe { declare_long(ce, c"__ext_sockets_timeout", 0, ZEND_ACC_PUBLIC) };
    }
    // SAFETY: MINIT, extending Swoole's registered exception class.
    unsafe {
        EXCEPTION.register_child(
            c"Swoole\\Coroutine\\Socket\\Exception",
            ptr::null(),
            exception,
            |_, _| {},
        );
    }
    if shortname {
        // SAFETY: as above.
        unsafe { EXCEPTION.alias(c"Co\\Socket\\Exception") }?;
    }
    // SAFETY: the module registry is complete before any MINIT runs.
    let loaded = unsafe { zend_hash_str_find(&raw const module_registry, c"sockets".as_ptr(), 7) };
    if loaded.is_null() {
        // SAFETY: MINIT of `module`.
        unsafe { register_constants(module) };
    }
    for (name, value) in constants::ALWAYS {
        // SAFETY: as above.
        unsafe { register_long_constant(name, i64::from(*value), module) };
    }
    crate::net::EXCEPTION_PARENT.store(exception, std::sync::atomic::Ordering::Release);
    // SAFETY: MINIT, once; nothing reads the class entry yet.
    unsafe { swoole_socket_coro_ce = ce };

    Ok(())
}

/// `socket_coro_register_constants`.
/// # Safety
/// MINIT of `module`.
unsafe fn register_constants(module: c_int) {
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
        // SAFETY: per the caller.
        unsafe { register_long_constant(name, i64::from(value), module) };
    }
}

/// `php_swoole_get_socket`: the C++ socket of a `Coroutine\Socket` object.
/// # Safety
/// `object` is a live `Coroutine\Socket` object.
pub(crate) unsafe fn socket_of(object: *mut zend_object) -> Socket {
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
        swoole_rs_zend::bailout::Bailout.defer();
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

/// `sw_zend_string_recycle`: shrink a buffer string to `used` bytes, giving the
/// memory back when it wasted more than half of a buffer over a page.
/// # Safety
/// `buffer` is an unshared string of `allocated` bytes, `used` of them filled.
unsafe fn recycle(buffer: *mut zend_string, allocated: usize, used: usize) -> *mut zend_string {
    let mut buffer = buffer;
    if allocated != used && allocated > pagesize() && allocated > used * 2 {
        // SAFETY: per the caller.
        buffer = unsafe { string::realloc(buffer, used) };
    }
    // SAFETY: per the caller.
    unsafe { string::truncate(buffer, used) };
    buffer
}

/// Return the first `bytes` of `buffer` (`length` allocated): false when
/// negative, '' when zero.
fn set_received(call: &Call, buffer: *mut zend_string, length: usize, bytes: isize) {
    match usize::try_from(bytes) {
        Ok(used) if used > 0 => {
            // SAFETY: an unshared buffer string the transfer filled `used` bytes of.
            let buffer = unsafe { recycle(buffer, length, used) };
            // SAFETY: the return value takes the buffer's only reference.
            unsafe { call.set_string(buffer) };
        }
        result => {
            // SAFETY: the buffer was never shared.
            unsafe { string::free(buffer) };
            if result.is_ok() {
                call.set_empty_string();
            } else {
                call.set_bool(false);
            }
        }
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
        // SAFETY: from a handler, with `$this` live.
        unsafe { throw_constructed_twice(call.this()) };
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
unsafe fn init(object: *mut zend_object) -> Result<(), swoole_rs_zend::bailout::Bailout> {
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
pub unsafe fn init_socket_object(
    zobject: *mut zval,
    socket: Socket,
) -> Result<(), swoole_rs_zend::bailout::Bailout> {
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
    let buffer = string::alloc(length);
    // SAFETY: a buffer of `length` bytes.
    let bytes = unsafe { socket.transfer(op, string::val(buffer).cast(), length, timeout) };
    sync(call, socket);
    set_received(call, buffer, length, bytes);
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
    let packet = socket.pop_packet();
    if packet.is_null() {
        return call.set_bool(false);
    }
    // `zend::assign_zend_string_by_val`: the packet is the value of a zend
    // string of at least `length` bytes the caller now owns.
    let packet = string::from_val(packet.cast());
    // SAFETY: as above.
    unsafe { string::truncate(packet, length) };
    // SAFETY: as above.
    unsafe { call.set_string(packet) };
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

/// The values of the array in `array`, in order (`SW_HASHTABLE_FOREACH_START`).
/// # Safety
/// `array` is a live array zval that does not change while the result is used.
unsafe fn values(array: *mut zval) -> impl Iterator<Item = *mut zval> {
    // SAFETY: per the caller.
    let table = unsafe { value::table(array) };
    let mut position: HashPosition = 0;
    // SAFETY: a live table and a writable position.
    unsafe { zend_hash_internal_pointer_reset_ex(table, &raw mut position) };
    std::iter::from_fn(move || {
        // SAFETY: as above.
        let item = unsafe { zend_hash_get_current_data_ex(table, &raw mut position) };
        if item.is_null() {
            return None;
        }
        // SAFETY: as above.
        unsafe { zend_hash_move_forward_ex(table, &raw mut position) };
        Some(item)
    })
}

/// The vectors argument: its element count, or false (with errCode EINVAL)
/// past `IOV_MAX`.
fn vector_count(socket: Co, array: *mut zval) -> Option<usize> {
    // SAFETY: a live array argument.
    let count = unsafe { value::count(array) };
    if count > IOV_MAX {
        let message = format!("The maximum of iov count is {IOV_MAX}");
        socket.set_error(libc::EINVAL, message.as_bytes());
        return None;
    }
    usize::try_from(count).ok()
}

/// `zend_get_type_by_const(Z_TYPE_P(item))`.
fn type_name(item: *mut zval) -> String {
    // SAFETY: a live element.
    let kind = unsafe { type_of(item) };
    #[expect(clippy::cast_possible_wrap, reason = "zval types are below 32")]
    // SAFETY: Zend returns a static name or null.
    let name = unsafe { zend_get_type_by_const(kind as c_int) };
    if name.is_null() {
        return String::new();
    }
    // SAFETY: a static NUL-terminated name.
    unsafe { CStr::from_ptr(name) }
        .to_string_lossy()
        .into_owned()
}

/// `(array $io_vector, float $timeout = 0)`.
fn vector_arguments(call: &Call) -> Result<(*mut zval, f64), Invalid> {
    call.arity(1, 2)?;
    Ok((call.array(1)?, call.double(2, 0.0)?))
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

fn write_vector(call: &Call, socket: Co, op: c_int, array: *mut zval, timeout: f64) {
    let Some(count) = vector_count(socket, array) else {
        return call.set_bool(false);
    };
    let mut vectors = Vec::with_capacity(count);
    // SAFETY: a live array argument the call does not change.
    for (index, item) in unsafe { values(array) }.enumerate() {
        // SAFETY: a live element.
        if unsafe { type_of(item) } != IS_STRING {
            let given = type_name(item);
            throw(
                libc::EINVAL,
                &format!("Item #[{index}] must be of type string, {given} given"),
            );
            return call.set_bool(false);
        }
        // SAFETY: an IS_STRING element.
        let text = unsafe { value::str_of(item) };
        // SAFETY: as above.
        let bytes = unsafe { string::bytes(text) };
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

fn read_vector(call: &Call, socket: Co, op: c_int, array: *mut zval, timeout: f64) {
    let Some(count) = vector_count(socket, array) else {
        return call.set_bool(false);
    };
    let mut vectors: Vec<iovec> = Vec::with_capacity(count);
    let mut total = 0usize;
    let release = |vectors: &[iovec]| {
        for vector in vectors {
            // SAFETY: each buffer is a string allocated below and never shared.
            unsafe { string::free(string::from_val(vector.iov_base.cast())) };
        }
    };
    // SAFETY: a live array argument the call does not change.
    for (index, item) in unsafe { values(array) }.enumerate() {
        // SAFETY: a live element.
        if unsafe { type_of(item) } != IS_LONG {
            let given = type_name(item);
            throw(
                libc::EINVAL,
                &format!("Item #[{index}] must be of type int, {given} given"),
            );
            release(&vectors);
            return call.set_bool(false);
        }
        // SAFETY: an IS_LONG element.
        let Ok(length) = usize::try_from(unsafe { value::long_of(item) }) else {
            throw(
                libc::EINVAL,
                &format!("Item #[{index}] must be greater than 0"),
            );
            release(&vectors);
            return call.set_bool(false);
        };
        vectors.push(iovec {
            iov_base: string::val(string::alloc(length)).cast(),
            iov_len: length,
        });
        total += length;
    }
    // SAFETY: unshared buffers of the listed lengths.
    let (read, stopped, offset) = unsafe { socket.vector(op, &vectors, timeout) };
    let read = match usize::try_from(read) {
        Ok(0) | Err(_) => {
            release(&vectors);
            return if read == 0 {
                call.set_empty_array();
            } else {
                call.set_bool(false);
            };
        }
        Ok(read) => read,
    };
    // SAFETY: the return slot is writable and holds nothing yet.
    unsafe { value::array_init(call.return_value()) };
    let mut filled = vectors.len();
    if read < total {
        // The read stopped `offset` bytes into vector `stopped`: keep that
        // much of it and free the vectors after it.
        let stopped = usize::try_from(stopped).unwrap_or(0);
        if let Some(vector) = vectors.get_mut(stopped) {
            filled = stopped + 1;
            // SAFETY: a buffer string allocated above, `offset` bytes of it filled.
            let kept = unsafe {
                recycle(
                    string::from_val(vector.iov_base.cast()),
                    vector.iov_len,
                    offset,
                )
            };
            vector.iov_base = string::val(kept).cast();
            vector.iov_len = offset;
        }
        release(vectors.get(filled..).unwrap_or_default());
    }
    for vector in vectors.iter().take(filled) {
        let buffer = string::from_val(vector.iov_base.cast());
        // SAFETY: an unshared buffer string of at least `iov_len` bytes.
        unsafe { string::truncate(buffer, vector.iov_len) };
        // SAFETY: the array takes the buffer's only reference.
        unsafe { value::push_str(call.return_value(), buffer) };
    }
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
    let parsed = call
        .arity(1, 2)
        .and_then(|()| Ok((call.zval_deref(1), call.double(2, 0.0)?)));
    let Ok((peer, timeout)) = parsed else {
        return call.set_bool(false);
    };
    let Some(socket) = open(call) else { return };
    let length = buffer_length(BUFFER_SIZE_BIG);
    let buffer = string::alloc(length);
    // SAFETY: a buffer of `length` bytes.
    let bytes = unsafe { socket.recvfrom(string::val(buffer).cast(), length, timeout) };
    sync(call, socket);
    let Ok(used @ 1..) = usize::try_from(bytes) else {
        // SAFETY: the buffer was never shared.
        unsafe { string::free(buffer) };
        return if bytes == 0 {
            call.set_empty_string();
        } else {
            call.set_bool(false);
        };
    };
    // SAFETY: the dereferenced by-reference argument: release what it held and
    // make it the peer's address.
    unsafe { zval_ptr_dtor(peer) };
    // SAFETY: as above.
    unsafe { value::array_init(peer) };
    add_address(peer, socket);
    // SAFETY: an unshared buffer string `used` bytes of which were filled.
    unsafe { string::truncate(buffer, used) };
    // SAFETY: the return value takes the buffer's only reference.
    unsafe { call.set_string(buffer) };
}

/// `address` and `port` of the socket's last address into `array`.
pub(crate) fn add_address(array: *mut zval, socket: Co) {
    let (address, port) = socket.address();
    // SAFETY: a live array.
    unsafe { value::add_string(array, c"address", address) };
    // SAFETY: as above.
    unsafe { value::add_long(array, c"port", i64::from(port)) };
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
    // SAFETY: the return slot is writable and holds nothing yet.
    unsafe { value::array_init(call.return_value()) };
    add_address(call.return_value(), socket);
}

/// `getpeername(): array|false`.
fn getpeername(call: &Call) {
    let Some(socket) = open(call) else { return };
    let Some((address, port)) = socket.getpeername() else {
        sync(call, socket);
        return call.set_bool(false);
    };
    let address = address.as_c_str();
    let array = call.return_value();
    // SAFETY: the return slot is writable and holds nothing yet.
    unsafe { value::array_init(array) };
    // SAFETY: a live array.
    unsafe { value::add_string(array, c"address", address) };
    // SAFETY: as above.
    unsafe { value::add_long(array, c"port", i64::from(port)) };
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
    let Ok(settings) = call.arity(1, 1).and_then(|()| call.array(1)) else {
        return call.set_bool(false);
    };
    // SAFETY: a live array argument.
    if unsafe { value::count(settings) } == 0 {
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

/// # Safety
/// A native Swoole socket kind; the resulting object owns its new core socket.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpclients_socket_create(kind: i32) -> *mut zend_object {
    // SAFETY: the SDK caller supplies its original constructor kind.
    let result = match unsafe { owner::create_kind(kind) } {
        Ok(object) => object,
        Err(error) => {
            error.defer();
            ptr::null_mut()
        }
    };
    swoole_rs_zend::bailout::finish();
    result
}
/// `php_swoole_create_socket(type)`.
/// # Safety
/// As [`swr_v1_phpclients_socket_create`].
#[unsafe(export_name = "_Z24php_swoole_create_socket12swSocketType")]
#[must_use]
pub unsafe extern "C" fn php_swoole_create_socket(kind: i32) -> *mut zend_object {
    // SAFETY: per the caller.
    unsafe { swr_v1_phpclients_socket_create(kind) }
}
/// `php_swoole_create_socket_from_fd(fd, type)`.
/// # Safety
/// Transfer one descriptor classified by its native Swoole socket kind.
#[unsafe(export_name = "_Z32php_swoole_create_socket_from_fdi12swSocketType")]
#[must_use]
pub unsafe extern "C" fn php_swoole_create_socket_from_fd(fd: i32, kind: i32) -> *mut zend_object {
    // SAFETY: the SDK caller transfers this descriptor to its new object.
    let result = match unsafe { owner::from_fd(fd, kind) } {
        Ok(object) => object,
        Err(error) => {
            error.defer();
            ptr::null_mut()
        }
    };
    swoole_rs_zend::bailout::finish();
    result
}
/// `php_swoole_create_socket_from_fd(fd, domain, type, protocol)`.
/// # Safety
/// Transfer the descriptor with its native domain/type/protocol classification.
#[unsafe(export_name = "_Z32php_swoole_create_socket_from_fdiiii")]
#[must_use]
pub unsafe extern "C" fn php_swoole_create_socket_from_fd_family(
    fd: i32,
    domain: i32,
    kind: i32,
    protocol: i32,
) -> *mut zend_object {
    // SAFETY: the native SDK lends the original constructor arguments and transfers the fd.
    let result = match unsafe { owner::from_fd_family(fd, domain, kind, protocol) } {
        Ok(object) => object,
        Err(error) => {
            error.defer();
            ptr::null_mut()
        }
    };
    swoole_rs_zend::bailout::finish();
    result
}
/// `php_swoole_dup_socket(fd, type)`.
/// # Safety
/// Borrow a valid source descriptor, as the original SDK dup helper does.
#[unsafe(export_name = "_Z21php_swoole_dup_socketi12swSocketType")]
#[must_use]
pub unsafe extern "C" fn php_swoole_dup_socket(fd: i32, kind: i32) -> *mut zend_object {
    // SAFETY: the SDK caller keeps the source descriptor live through dup.
    let result = match unsafe { owner::duplicate(fd, kind) } {
        Ok(object) => object,
        Err(error) => {
            error.defer();
            ptr::null_mut()
        }
    };
    swoole_rs_zend::bailout::finish();
    result
}
/// # Safety
/// The argument is a live Coroutine Socket object zval.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpclients_socket_pointer(value: *const zval) -> *mut libc::c_void {
    // SAFETY: the native SDK supplies an initialized object zval of this class.
    let object = unsafe { value::obj_of(value) };
    // SAFETY: this live object has the registered SocketObject layout.
    unsafe { socket_of(object) }
}
/// `php_swoole_get_socket(zobject)`.
/// # Safety
/// As [`swr_v1_phpclients_socket_pointer`].
#[unsafe(export_name = "_Z21php_swoole_get_socketPK12_zval_struct")]
#[must_use]
pub unsafe extern "C" fn php_swoole_get_socket(value: *const zval) -> Socket {
    // SAFETY: per the caller.
    unsafe { swr_v1_phpclients_socket_pointer(value) }
}
/// `php_swoole_socket_is_closed(zobject)`.
/// # Safety
/// The argument is a live Coroutine Socket object zval, constructed or unconstructed.
#[unsafe(export_name = "_Z27php_swoole_socket_is_closedPK12_zval_struct")]
#[must_use]
pub unsafe extern "C" fn php_swoole_socket_is_closed(value: *const zval) -> bool {
    // SAFETY: the SDK argument is a live object zval of this class.
    let object = unsafe { value::obj_of(value) };
    // SAFETY: copy the object's optional core socket pointer.
    let socket = unsafe { socket_of(object) };
    // SAFETY: a non-null core socket remains live while this object is borrowed.
    socket.is_null() || unsafe { Co::new(socket) }.is_closed()
}

/// `php_swoole_socket_set_error_properties(zobject, socket)`: its error.
/// # Safety
/// A live socket object zval and a live socket.
#[unsafe(export_name = "_Z38php_swoole_socket_set_error_propertiesPK12_zval_structPKN6swoole9coroutine6SocketE")]
pub unsafe extern "C" fn php_swoole_socket_set_error_properties_socket(
    value: *const zval,
    socket: Socket,
) {
    // SAFETY: per the caller.
    let (code, message) = unsafe { Co::new(socket) }.error();
    // SAFETY: as above; the message lives in the socket.
    unsafe { php_swoole_socket_set_error_properties(value, code, message.as_ptr()) };
}
/// `php_swoole_socket_set_error_properties(zobject, code)`: `swoole_strerror(code)`.
/// # Safety
/// A live socket object zval.
#[unsafe(export_name = "_Z38php_swoole_socket_set_error_propertiesPK12_zval_structi")]
pub unsafe extern "C" fn php_swoole_socket_set_error_properties_code(
    value: *const zval,
    code: i32,
) {
    // SAFETY: per the caller; a static message.
    unsafe { php_swoole_socket_set_error_properties(value, code, native::swoole_strerror(code)) };
}
/// `php_swoole_socket_set_error_properties(zobject, code, message)`.
/// # Safety
/// A live SDK socket object zval and borrowed diagnostic message, as the original helper requires.
#[unsafe(export_name = "_Z38php_swoole_socket_set_error_propertiesPK12_zval_structiPKc")]
pub unsafe extern "C" fn php_swoole_socket_set_error_properties(
    value: *const zval,
    code: i32,
    message: *const c_char,
) {
    swoole_rs_corebase::log::set_last_error(code);
    // SAFETY: the SDK supplies an initialized object zval of this class.
    let object = unsafe { value::obj_of(value) };
    // SAFETY: the live object's actual class is the original SDK helper's scope.
    let scope = unsafe { (*object).ce };
    let message = if message.is_null() {
        c""
    } else {
        // SAFETY: a non-null diagnostic message is a borrowed NUL-terminated native string.
        unsafe { CStr::from_ptr(message) }
    };
    // SAFETY: scope, object and message satisfy the SDK helper's live-argument contract.
    swoole_rs_zend::bailout::defer(unsafe {
        owner::error_properties(scope, object, code, message)
    });
    swoole_rs_zend::bailout::finish();
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
