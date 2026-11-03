//! `Swoole\Coroutine\Client`: registration, the object and every method. The
//! client owns a `Coroutine\Socket` (`zsocket`), created on connect by C++
//! (`swoole_client_coro_create_socket`, which also hooks the socket's close);
//! each method parses as upstream's ZPP block does and makes one call into
//! the C++ socket (`crate::native`).
use crate::{
    native::{self, Socket, Transfer},
    socket::{socket_of, strerror, warn},
};
use libc::{c_char, c_int};
use std::{
    ffi::{CStr, CString},
    mem::MaybeUninit,
    ptr,
};
use swoole_rs_error::swr_v1_error_strerror;
use swoole_rs_phpstubs::client_coro;
use swoole_rs_zend::{
    Call, Class, Object, declare_bool, declare_long, declare_string, set_object, string,
    sys::{
        IS_ARRAY, IS_OBJECT, ZEND_ACC_NOT_SERIALIZABLE, ZEND_ACC_PUBLIC, get_active_class_name,
        get_active_function_name, zend_class_entry, zend_object, zend_object_handlers, zend_string,
        zend_type_error, zval, zval_ptr_dtor,
    },
    type_of, unset_property_deny, value,
};
use swoole_rs_zend::{method_table, sys as z};

/// `SW_FLAG_ASYNC`, `SW_FLAG_SYNC`, `SW_FLAG_KEEP` and `SW_SOCK_SSL`: flags a
/// client type carries besides its socket type.
const TYPE_FLAGS: i64 = (1 << 10) | (1 << 11) | (1 << 12) | (1 << 9);
/// `SW_SOCK_*`.
const SOCK_TCP: i64 = 1;
const SOCK_TCP6: i64 = 3;
const SOCK_UNIX_STREAM: i64 = 5;
const SOCK_UNIX_DGRAM: i64 = 6;
/// `SW_PHP_CLIENT_BUFFER_SIZE`.
const BUFFER_SIZE: i64 = 65535;
/// `SW_ERROR_CLIENT_NO_CONNECTION`.
const NO_CONNECTION: c_int = 5001;

/// `struct ClientCoroObject { SocketImpl *socket; zval zsocket; zval zobject; zend_object std; }`.
#[repr(C)]
pub(crate) struct Data {
    socket: Socket,
    zsocket: zval,
    zobject: zval,
}

type ClientObject = Object<Data>;

static CLIENT: Class = Class::new();

unsafe extern "C" fn create(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: Zend creates this class or a subclass; zeroed data is no socket.
    let std = unsafe { ClientObject::create(&CLIENT, ce) };
    // SAFETY: the object just created.
    let data = unsafe { ClientObject::data(std) };
    // SAFETY: `zobject` names the object itself, without a reference.
    unsafe { set_object(&raw mut data.zobject, std) };
    // `zsocket` starts as null (IS_NULL is 1; zeroed is IS_UNDEF).
    data.zsocket.u1.type_info = swoole_rs_zend::sys::IS_NULL;
    std
}

unsafe extern "C" fn free(std: *mut zend_object) {
    // SAFETY: Zend frees a client object.
    let socket = unsafe { ClientObject::data(std) }.socket;
    if !socket.is_null() {
        // SAFETY: a live socket the client still holds.
        unsafe { native::swoole_client_coro_close(socket) };
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

method_table! {
    static METHODS = [
        c"__construct" => zim_swoole_client_coro___construct, &client_coro::arginfo_class_Swoole_Coroutine_Client___construct, z::ZEND_ACC_PUBLIC;
        c"__destruct" => zim_swoole_client_coro___destruct, &client_coro::arginfo_class_Swoole_Coroutine_Client___destruct, z::ZEND_ACC_PUBLIC;
        c"set" => zim_swoole_client_coro_set, &client_coro::arginfo_class_Swoole_Coroutine_Client_set, z::ZEND_ACC_PUBLIC;
        c"connect" => zim_swoole_client_coro_connect, &client_coro::arginfo_class_Swoole_Coroutine_Client_connect, z::ZEND_ACC_PUBLIC;
        c"recv" => zim_swoole_client_coro_recv, &client_coro::arginfo_class_Swoole_Coroutine_Client_recv, z::ZEND_ACC_PUBLIC;
        c"peek" => zim_swoole_client_coro_peek, &client_coro::arginfo_class_Swoole_Coroutine_Client_peek, z::ZEND_ACC_PUBLIC;
        c"send" => zim_swoole_client_coro_send, &client_coro::arginfo_class_Swoole_Coroutine_Client_send, z::ZEND_ACC_PUBLIC;
        c"sendfile" => zim_swoole_client_coro_sendfile, &client_coro::arginfo_class_Swoole_Coroutine_Client_sendfile, z::ZEND_ACC_PUBLIC;
        c"sendto" => zim_swoole_client_coro_sendto, &client_coro::arginfo_class_Swoole_Coroutine_Client_sendto, z::ZEND_ACC_PUBLIC;
        c"recvfrom" => zim_swoole_client_coro_recvfrom, &client_coro::arginfo_class_Swoole_Coroutine_Client_recvfrom, z::ZEND_ACC_PUBLIC;
        c"enableSSL" => zim_swoole_client_coro_enableSSL, &client_coro::arginfo_class_Swoole_Coroutine_Client_enableSSL, z::ZEND_ACC_PUBLIC;
        c"getPeerCert" => zim_swoole_client_coro_getPeerCert, &client_coro::arginfo_class_Swoole_Coroutine_Client_getPeerCert, z::ZEND_ACC_PUBLIC;
        c"verifyPeerCert" => zim_swoole_client_coro_verifyPeerCert, &client_coro::arginfo_class_Swoole_Coroutine_Client_verifyPeerCert, z::ZEND_ACC_PUBLIC;
        c"isConnected" => zim_swoole_client_coro_isConnected, &client_coro::arginfo_class_Swoole_Coroutine_Client_isConnected, z::ZEND_ACC_PUBLIC;
        c"getsockname" => zim_swoole_client_coro_getsockname, &client_coro::arginfo_class_Swoole_Coroutine_Client_getsockname, z::ZEND_ACC_PUBLIC;
        c"getpeername" => zim_swoole_client_coro_getpeername, &client_coro::arginfo_class_Swoole_Coroutine_Client_getpeername, z::ZEND_ACC_PUBLIC;
        c"close" => zim_swoole_client_coro_close, &client_coro::arginfo_class_Swoole_Coroutine_Client_close, z::ZEND_ACC_PUBLIC;
        c"exportSocket" => zim_swoole_client_coro_exportSocket, &client_coro::arginfo_class_Swoole_Coroutine_Client_exportSocket, z::ZEND_ACC_PUBLIC;
    ];
}

/// MINIT, as `php_swoole_client_coro_minit` did.
/// # Safety
/// Called once, at MINIT, with upstream's static method table and a writable
/// class-entry slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpclients_client_minit(
    shortname: bool,
    client_class: *mut *mut zend_class_entry,
) {
    // SAFETY: MINIT, with a static table.
    unsafe { CLIENT.register(c"Swoole\\Coroutine\\Client", METHODS.as_ptr(), configure) };
    if shortname {
        // SAFETY: MINIT, after `register`.
        unsafe { CLIENT.alias(c"Co\\Client") };
    }
    let ce = CLIENT.entry();
    // SAFETY: MINIT, with the class just registered (each call below too).
    unsafe { declare_long(ce, c"errCode", 0, ZEND_ACC_PUBLIC) };
    // SAFETY: as above.
    unsafe { declare_string(ce, c"errMsg", c"", ZEND_ACC_PUBLIC) };
    // SAFETY: as above.
    unsafe { declare_long(ce, c"fd", -1, ZEND_ACC_PUBLIC) };
    // SAFETY: as above.
    unsafe { CLIENT.property_null(c"socket") };
    // SAFETY: as above.
    unsafe { declare_long(ce, c"type", SOCK_TCP, ZEND_ACC_PUBLIC) };
    // SAFETY: as above.
    unsafe { CLIENT.property_null(c"setting") };
    // SAFETY: as above.
    unsafe { declare_bool(ce, c"connected", false, ZEND_ACC_PUBLIC) };
    for (name, value) in [
        (c"MSG_OOB", libc::MSG_OOB),
        (c"MSG_PEEK", libc::MSG_PEEK),
        (c"MSG_DONTWAIT", libc::MSG_DONTWAIT),
        (c"MSG_WAITALL", libc::MSG_WAITALL),
    ] {
        // SAFETY: as above.
        unsafe { CLIENT.constant_long(name, i64::from(value)) };
    }
    // SAFETY: per the caller, a writable slot.
    unsafe { *client_class = ce };
}

fn this<'a>(call: &Call) -> &'a mut Data {
    // SAFETY: client methods are non-static, so `$this` is a client object, and
    // no other reference to its data is live during the call.
    unsafe { ClientObject::data(call.this()) }
}

/// `php_swoole_client_get_type`: the socket type without the client flags.
fn socket_type(kind: i64) -> i64 {
    kind & !TYPE_FLAGS
}

/// `Address::verify_port(port, true)`, over upstream's `int` port.
fn verify_port(port: i64) -> bool {
    #[expect(clippy::cast_possible_truncation, reason = "upstream passes an int")]
    let port = port as i32;
    (1..=65535).contains(&port)
}

extern "C" fn system_error(code: i32) -> *const c_char {
    strerror(code).as_ptr()
}

/// `php_swoole_socket_set_error_properties(ZEND_THIS, code, message)`.
fn report(call: &Call, code: c_int, message: &CStr) {
    // SAFETY: a plain store into Swoole's thread error.
    unsafe { native::swoole_rs_set_last_error(code) };
    let object = call.this();
    // SAFETY: `$this` is live.
    let ce = unsafe { (*object).ce };
    // SAFETY: `$this` is a live instance of `ce`.
    unsafe { value::update_long(ce, object, c"errCode", i64::from(code)) };
    // SAFETY: as above.
    unsafe { value::update_string(ce, object, c"errMsg", message) };
}

/// `php_swoole_socket_set_error_properties(ZEND_THIS, code)`: `swoole_strerror(code)`.
fn report_code(call: &Call, code: c_int) {
    // SAFETY: known and system messages are static; the unknown-message buffer
    // is only used on the PHP thread, as upstream's is.
    let message = unsafe { swr_v1_error_strerror(code, Some(system_error)) };
    let message = if message.is_null() {
        c""
    } else {
        // SAFETY: a NUL-terminated message.
        unsafe { CStr::from_ptr(message) }
    };
    report(call, code, message);
}

/// `php_swoole_socket_set_error_properties(ZEND_THIS, socket)`.
fn report_socket(call: &Call, socket: Socket) {
    let mut message: *const c_char = ptr::null();
    // SAFETY: a live socket; `message` is a writable local.
    let code = unsafe { native::swoole_socket_coro_error(socket, &raw mut message) };
    let message = if message.is_null() {
        c""
    } else {
        // SAFETY: errMsg is a NUL-terminated string the socket keeps.
        unsafe { CStr::from_ptr(message) }
    };
    report(call, code, message);
}

/// A reference to the client's socket object for the rest of the call, so a
/// close inside it cannot free the socket (`SW_CLIENT_GET_SOCKET_SAFE`'s
/// `tmp_socket`).
struct Hold(*mut zend_object);

impl Drop for Hold {
    fn drop(&mut self) {
        let mut held = MaybeUninit::<zval>::zeroed();
        // SAFETY: the held object, whose reference this zval takes over.
        unsafe { set_object(held.as_mut_ptr(), self.0) };
        // SAFETY: releases the reference `connected` took.
        unsafe { zval_ptr_dtor(held.as_mut_ptr()) };
    }
}

/// `CLIENT_CORO_GET_SOCKET_SAFE`: the connected socket, held for the call;
/// else errCode/errMsg say "Client no connection" and the method returns false.
fn connected(call: &Call) -> Option<(Socket, Hold)> {
    let zsocket = &raw mut this(call).zsocket;
    // SAFETY: the client's own zval.
    if unsafe { type_of(zsocket) } == IS_OBJECT {
        // SAFETY: an object zval.
        let object = unsafe { value::obj_of(zsocket) };
        // SAFETY: a live object; the hold releases this reference.
        unsafe { (*object).gc.refcount += 1 };
        let hold = Hold(object);
        // SAFETY: the client only ever stores `Coroutine\Socket` objects.
        let socket = unsafe { socket_of(object) };
        if !socket.is_null() {
            return Some((socket, hold));
        }
    }
    report_code(call, NO_CONNECTION);
    call.set_bool(false);
    None
}

/// `client_coro_get_socket_for_connect`: create, publish and configure the
/// client's socket; None (with the error reported) when it has one already, the
/// port is invalid or creation fails.
fn socket_for_connect(call: &Call, port: i64) -> Option<Socket> {
    let data = this(call);
    if !data.socket.is_null() {
        report(call, libc::EISCONN, strerror(libc::EISCONN));
        return None;
    }
    let object = call.this();
    // SAFETY: `$this` is live.
    let kind = unsafe { native::swoole_client_coro_type_property(object) };
    let tcp = matches!(socket_type(kind), SOCK_TCP | SOCK_TCP6);
    if tcp && !verify_port(port) {
        warn(true, "The port is invalid");
        return None;
    }
    // SAFETY: `$this` is live and has no socket.
    let socket = unsafe { native::swoole_client_coro_create_socket(object, kind) };
    if socket.is_null() {
        return None;
    }
    data.socket = socket;
    // SAFETY: `$this` is live.
    let settings = unsafe { native::swoole_client_coro_setting(object) };
    // SAFETY: Zend returns a live property zval.
    if !settings.is_null() && unsafe { type_of(settings) } == IS_ARRAY {
        crate::socket_options::apply(socket, settings, crate::socket_options::ALL);
    }
    Some(socket)
}

/// The socket for a datagram call: the client's, or a new one.
fn datagram_socket(call: &Call) -> Option<Socket> {
    let socket = this(call).socket;
    if socket.is_null() {
        socket_for_connect(call, 0)
    } else {
        Some(socket)
    }
}

/// `__construct(int $type)`.
fn construct(call: &Call) {
    let Ok(kind) = call.arity(1, 1).and_then(|()| call.long(1, 0)) else {
        return call.set_bool(false);
    };
    if !(SOCK_TCP..=SOCK_UNIX_DGRAM).contains(&socket_type(kind)) {
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
        return call.set_bool(false);
    }
    // SAFETY: from a handler.
    unsafe { native::swoole_rs_check_reactor() };
    // SAFETY: `$this` is a live instance of the registered class.
    unsafe { value::update_long(CLIENT.entry(), call.this(), c"type", kind) };
    call.set_bool(true);
}

/// `__destruct()`: nothing; the socket closes when the object is freed.
fn destruct(_call: &Call) {}

/// `set(array $settings): bool`.
fn set(call: &Call) {
    let Ok(settings) = call.arity(1, 1).and_then(|()| call.array(1)) else {
        return call.set_bool(false);
    };
    // SAFETY: a live array argument.
    if unsafe { value::count(settings) } == 0 {
        return call.set_bool(false);
    }
    // SAFETY: `$this` is live; `settings` is a live array.
    unsafe { native::swoole_client_coro_merge_setting(call.this(), settings) };
    let socket = this(call).socket;
    if socket.is_null() {
        return call.set_bool(true);
    }
    call.set_bool(crate::socket_options::apply(
        socket,
        settings,
        crate::socket_options::ALL,
    ));
}

/// `connect(string $host, int $port = 0, float $timeout = 0, int $sock_flag = 0): bool`.
fn connect(call: &Call) {
    let parsed = call.arity(1, 4).and_then(|()| {
        Ok((
            call.string(1, b"")?,
            call.long(2, 0)?,
            call.double(3, 0.0)?,
            call.long(4, 0)?,
        ))
    });
    let Ok((host, port, timeout, flags)) = parsed else {
        return call.set_bool(false);
    };
    if host.is_empty() {
        warn(true, "The host is empty");
        return call.set_bool(false);
    }
    let Some(socket) = socket_for_connect(call, port) else {
        return call.set_bool(false);
    };
    // SAFETY: a live socket; a string argument's bytes are NUL-terminated, and
    // upstream reads the host up to its first NUL.
    let ok = unsafe {
        native::swoole_client_coro_connect(socket, host.as_ptr().cast(), port, timeout, flags)
    };
    if !ok {
        report_socket(call, socket);
        // SAFETY: a live socket.
        unsafe { native::swoole_client_coro_close(socket) };
        return call.set_bool(false);
    }
    // SAFETY: a live socket.
    unsafe { native::swoole_client_coro_rdwr_timeout(socket, timeout) };
    // SAFETY: `$this` is a live instance of the registered class.
    unsafe { value::update_bool(CLIENT.entry(), call.this(), c"connected", true) };
    call.set_bool(true);
}

/// `send(string $data, float $timeout = 0): int|false`.
fn send(call: &Call) {
    let parsed = call
        .arity(1, 2)
        .and_then(|()| Ok((call.string(1, b"")?, call.double(2, 0.0)?)));
    let Ok((data, timeout)) = parsed else {
        return call.set_bool(false);
    };
    if data.is_empty() {
        warn(true, "data to send is empty");
        return call.set_bool(false);
    }
    let Some((socket, _hold)) = connected(call) else {
        return;
    };
    // SAFETY: a held socket; the transfer only reads the argument's bytes.
    let sent = unsafe {
        native::swoole_socket_coro_transfer(
            socket,
            Transfer::SendAll as c_int,
            data.as_ptr().cast_mut().cast(),
            data.len(),
            timeout,
        )
    };
    let Ok(count) = usize::try_from(sent) else {
        report_socket(call, socket);
        return call.set_bool(false);
    };
    if count < data.len() {
        let mut message: *const c_char = ptr::null();
        // SAFETY: a held socket and a writable local.
        if unsafe { native::swoole_socket_coro_error(socket, &raw mut message) } != 0 {
            report_socket(call, socket);
        }
    }
    call.set_long(sent as i64);
}

/// `sendto(string $host, int $port, string $data): bool`.
fn sendto(call: &Call) {
    let parsed = call
        .arity(3, 3)
        .and_then(|()| Ok((call.string(1, b"")?, call.long(2, 0)?, call.string(3, b"")?)));
    let Ok((host, port, data)) = parsed else {
        return call.set_bool(false);
    };
    if data.is_empty() {
        return call.set_bool(false);
    }
    let Some(socket) = datagram_socket(call) else {
        return call.set_bool(false);
    };
    // SAFETY: a live socket.
    if unsafe { native::record(socket) }.is_some_and(|record| record.is_tcp()) && !verify_port(port)
    {
        warn(true, "The port is invalid");
        return call.set_bool(false);
    }
    // SAFETY: a live socket and the arguments' bytes.
    let sent = unsafe {
        native::swoole_socket_coro_sendto(
            socket,
            host.as_ptr().cast(),
            host.len(),
            port,
            data.as_ptr().cast(),
            data.len(),
        )
    };
    if sent < 0 {
        report_socket(call, socket);
        return call.set_bool(false);
    }
    call.set_bool(true);
}

/// `recvfrom(int $length, &$address, &$port = 0): string|false`.
fn recvfrom(call: &Call) {
    let Ok(length) = call.arity(2, 3).and_then(|()| call.long(1, 0)) else {
        return call.set_bool(false);
    };
    let address = call.zval_deref(2);
    let port = (call.num_args() >= 3).then(|| call.zval_deref(3));
    let Some(length) = usize::try_from(length).ok().filter(|length| *length > 0) else {
        return call.set_bool(false);
    };
    let Some(socket) = datagram_socket(call) else {
        return call.set_bool(false);
    };
    let buffer = string::alloc(length);
    // SAFETY: a live socket and a buffer of `length` bytes; a zero timeout
    // leaves the socket's own.
    let received = unsafe {
        native::swoole_socket_coro_recvfrom(socket, string::val(buffer).cast(), length, 0.0)
    };
    let Ok(used) = usize::try_from(received) else {
        // SAFETY: the buffer was never shared.
        unsafe { string::free(buffer) };
        report_socket(call, socket);
        return call.set_bool(false);
    };
    let mut peer_port = 0;
    // SAFETY: a live socket and a writable local.
    let peer = unsafe { native::swoole_socket_coro_address(socket, &raw mut peer_port) };
    // SAFETY: the socket's NUL-terminated address text.
    let peer = unsafe { CStr::from_ptr(peer) };
    // SAFETY: the dereferenced by-reference argument: release it, then make it
    // the peer's address.
    unsafe { zval_ptr_dtor(address) };
    // SAFETY: as above; it takes the new string's only reference.
    unsafe { value::set_str(address, string::copy(peer.to_bytes())) };
    if let Some(port) = port {
        // SAFETY: as for the address.
        unsafe { zval_ptr_dtor(port) };
        // SAFETY: as above.
        unsafe { value::set_long(port, i64::from(peer_port)) };
    }
    // SAFETY: an unshared buffer string `used` bytes of which were filled.
    unsafe { string::truncate(buffer, used) };
    // SAFETY: the return value takes the buffer's only reference.
    unsafe { call.set_string(buffer) };
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
        warn(true, "file to send is empty");
        return call.set_bool(false);
    }
    let Some((socket, _hold)) = connected(call) else {
        return;
    };
    // SAFETY: a held socket.
    let kind = i64::from(unsafe { native::swoole_client_coro_type(socket) });
    if !matches!(kind, SOCK_TCP | SOCK_TCP6 | SOCK_UNIX_STREAM) {
        let object = call.this();
        // SAFETY: `$this` is a live instance of the registered class.
        unsafe { value::update_long(CLIENT.entry(), object, c"errCode", libc::EINVAL.into()) };
        // SAFETY: as above.
        unsafe {
            value::update_string(
                CLIENT.entry(),
                object,
                c"errMsg",
                c"dgram socket cannot use sendfile",
            );
        }
        return call.set_bool(false);
    }
    // SAFETY: a held socket; a string argument's bytes are NUL-terminated.
    if unsafe { native::swoole_socket_coro_sendfile(socket, file.as_ptr().cast(), offset, length) }
    {
        call.set_bool(true);
    } else {
        report_socket(call, socket);
        call.set_bool(false);
    }
}

/// `recv(float $timeout = 0): string|false`: a framed packet when a length or
/// EOF protocol is set, else whatever one read returns.
fn recv(call: &Call) {
    let Ok(timeout) = call.arity(0, 1).and_then(|()| call.double(1, 0.0)) else {
        return call.set_bool(false);
    };
    let Some((socket, _hold)) = connected(call) else {
        return;
    };
    let mut result: *mut zend_string = ptr::null_mut();
    // SAFETY: a held socket.
    let received = if unsafe { native::swoole_client_coro_packet_mode(socket) } {
        // SAFETY: as above.
        let mut received =
            unsafe { native::swoole_socket_coro_recv_packet(socket, timeout, false) };
        if received > 0 {
            // SAFETY: a held socket holding a packet.
            let packet = unsafe { native::swoole_socket_coro_pop_packet(socket) };
            if packet.is_null() {
                received = -1;
            } else {
                result = string::from_val(packet.cast());
            }
        }
        received
    } else {
        let length = usize::try_from(BUFFER_SIZE).unwrap_or(0) - size_of::<zend_string>();
        result = string::alloc(length);
        // SAFETY: a held socket and a buffer of `length` bytes.
        let received = unsafe {
            native::swoole_socket_coro_transfer(
                socket,
                Transfer::Recv as c_int,
                string::val(result).cast(),
                length,
                timeout,
            )
        };
        if received <= 0 {
            // SAFETY: the buffer was never shared.
            unsafe { string::free(result) };
        }
        received
    };
    match usize::try_from(received) {
        Err(_) => {
            report_socket(call, socket);
            call.set_bool(false);
        }
        Ok(0) => call.set_empty_string(),
        Ok(used) => {
            // SAFETY: an unshared string at least `used` bytes long.
            unsafe { string::truncate(result, used) };
            // SAFETY: the return value takes its only reference.
            unsafe { call.set_string(result) };
        }
    }
}

/// `peek(int $length = 65535): string|false`.
fn peek(call: &Call) {
    let Ok(length) = call.arity(0, 1).and_then(|()| call.long(1, BUFFER_SIZE)) else {
        return call.set_bool(false);
    };
    let Some((socket, _hold)) = connected(call) else {
        return;
    };
    let length = usize::try_from(length).unwrap_or(0);
    let buffer = string::alloc(length);
    // SAFETY: a held socket and a buffer of `length` bytes.
    let peeked = unsafe {
        native::swoole_socket_coro_transfer(
            socket,
            Transfer::Peek as c_int,
            string::val(buffer).cast(),
            length,
            0.0,
        )
    };
    let Ok(used) = usize::try_from(peeked) else {
        report_socket(call, socket);
        // SAFETY: the buffer was never shared.
        unsafe { string::free(buffer) };
        return call.set_bool(false);
    };
    // SAFETY: an unshared buffer string `used` bytes of which were filled.
    unsafe { string::truncate(buffer, used) };
    // SAFETY: the return value takes the buffer's only reference.
    unsafe { call.set_string(buffer) };
}

/// `isConnected(): bool`.
fn is_connected(call: &Call) {
    let socket = this(call).socket;
    // SAFETY: a socket the client holds is live.
    call.set_bool(!socket.is_null() && unsafe { native::swoole_client_coro_is_connected(socket) });
}

/// `["host" => .., "address" => .., "port" => ..]` into the return value.
fn set_endpoint(call: &Call, address: &CStr, port: c_int) {
    let array = call.return_value();
    // SAFETY: the return slot is writable and holds nothing yet.
    unsafe { value::array_init(array) };
    // SAFETY: a live array ("host" is kept for backward compatibility).
    unsafe { value::add_string(array, c"host", address) };
    // SAFETY: as above.
    unsafe { value::add_string(array, c"address", address) };
    // SAFETY: as above.
    unsafe { value::add_long(array, c"port", i64::from(port)) };
}

/// `getsockname(): array|false`.
fn getsockname(call: &Call) {
    let Some((socket, _hold)) = connected(call) else {
        return;
    };
    // SAFETY: a held socket.
    if !unsafe { native::swoole_socket_coro_getsockname(socket) } {
        report_socket(call, socket);
        return call.set_bool(false);
    }
    let mut port = 0;
    // SAFETY: a held socket and a writable local.
    let address = unsafe { native::swoole_socket_coro_address(socket, &raw mut port) };
    // SAFETY: the socket's NUL-terminated address text.
    set_endpoint(call, unsafe { CStr::from_ptr(address) }, port);
}

/// `getpeername(): array|false`.
fn getpeername(call: &Call) {
    let Some((socket, _hold)) = connected(call) else {
        return;
    };
    let mut address = [0 as c_char; 256];
    let mut port = 0;
    // SAFETY: a held socket and writable locals of the sizes given.
    let found = unsafe {
        native::swoole_socket_coro_getpeername(
            socket,
            address.as_mut_ptr(),
            address.len(),
            &raw mut port,
        )
    };
    if !found {
        report_socket(call, socket);
        return call.set_bool(false);
    }
    // SAFETY: the hook NUL-terminates what it writes.
    set_endpoint(call, unsafe { CStr::from_ptr(address.as_ptr()) }, port);
}

/// `exportSocket(): Socket|null`: the client's `Coroutine\Socket`, shared.
fn export_socket(call: &Call) {
    let zsocket = &raw mut this(call).zsocket;
    // SAFETY: the client's own zval.
    if unsafe { type_of(zsocket) } == IS_OBJECT {
        // SAFETY: an object zval.
        let object = unsafe { value::obj_of(zsocket) };
        // SAFETY: a live object; the return value takes this new reference.
        unsafe { (*object).gc.refcount += 1 };
        // SAFETY: as above.
        unsafe { set_object(call.return_value(), object) };
    } else {
        call.set_null();
    }
}

/// `close(): bool`.
fn close(call: &Call) {
    let Some((socket, _hold)) = connected(call) else {
        return;
    };
    let object = call.this();
    // SAFETY: `$this` is live.
    let ce = unsafe { (*object).ce };
    // SAFETY: `$this` is a live instance of `ce`.
    unsafe { value::update_bool(ce, object, c"connected", false) };
    // SAFETY: a held socket.
    if !unsafe { native::swoole_client_coro_close(socket) } {
        report_socket(call, socket);
        return call.set_bool(false);
    }
    call.set_bool(true);
}

/// `enableSSL(): bool`.
fn enable_ssl(call: &Call) {
    let Some((socket, _hold)) = connected(call) else {
        return;
    };
    // SAFETY: a held socket.
    let kind = i64::from(unsafe { native::swoole_client_coro_type(socket) });
    if !matches!(kind, SOCK_TCP | SOCK_TCP6) {
        report_code(call, libc::ESOCKTNOSUPPORT);
        return call.set_bool(false);
    }
    // SAFETY: a held socket.
    if unsafe { native::swoole_client_coro_has_ssl(socket) }
        || !crate::socket_options::enable_ssl(socket)
    {
        report_code(call, libc::EISCONN);
        return call.set_bool(false);
    }
    // SAFETY: `$this` is live.
    let settings = unsafe { native::swoole_client_coro_setting(call.this()) };
    // SAFETY: Zend returns a live property zval.
    let array = !settings.is_null() && unsafe { type_of(settings) } == IS_ARRAY;
    // SAFETY: as above, an array.
    if array && unsafe { value::count(settings) } > 0 {
        crate::socket_options::apply(socket, settings, crate::socket_options::SSL);
    }
    // SAFETY: a held socket.
    if !unsafe { native::swoole_socket_coro_ssl_handshake(socket) } {
        report_socket(call, socket);
        return call.set_bool(false);
    }
    call.set_bool(true);
}

/// `getPeerCert(): string|false`.
fn peer_cert(call: &Call) {
    let Some((socket, _hold)) = connected(call) else {
        return;
    };
    // SAFETY: a held socket.
    if !unsafe { native::swoole_client_coro_has_ssl(socket) } {
        report_code(call, libc::EISCONN);
        return call.set_bool(false);
    }
    // SAFETY: as above.
    let cert = unsafe { native::swoole_client_coro_peer_cert(socket) };
    if cert.is_null() {
        return call.set_bool(false);
    }
    // SAFETY: the return value takes the new string's only reference.
    unsafe { call.set_string(cert) };
}

/// `verifyPeerCert(bool $allow_self_signed = false): bool`; the socket is
/// checked first.
fn verify_peer_cert(call: &Call) {
    let Some((socket, _hold)) = connected(call) else {
        return;
    };
    // SAFETY: a held socket.
    if !unsafe { native::swoole_client_coro_has_ssl(socket) } {
        report_code(call, libc::ENOTCONN);
        return call.set_bool(false);
    }
    let Ok(allow) = call.arity(0, 1).and_then(|()| call.bool(1, false)) else {
        return call.set_bool(false);
    };
    // SAFETY: a held socket.
    call.set_bool(unsafe { native::swoole_client_coro_verify_peer(socket, allow) });
}

swoole_rs_zend::handlers! {
    zim_swoole_client_coro___construct => construct;
    zim_swoole_client_coro___destruct => destruct;
    zim_swoole_client_coro_set => set;
    zim_swoole_client_coro_connect => connect;
    zim_swoole_client_coro_recv => recv;
    zim_swoole_client_coro_peek => peek;
    zim_swoole_client_coro_send => send;
    zim_swoole_client_coro_sendfile => sendfile;
    zim_swoole_client_coro_sendto => sendto;
    zim_swoole_client_coro_recvfrom => recvfrom;
    zim_swoole_client_coro_enableSSL => enable_ssl;
    zim_swoole_client_coro_getPeerCert => peer_cert;
    zim_swoole_client_coro_verifyPeerCert => verify_peer_cert;
    zim_swoole_client_coro_exportSocket => export_socket;
    zim_swoole_client_coro_isConnected => is_connected;
    zim_swoole_client_coro_getsockname => getsockname;
    zim_swoole_client_coro_getpeername => getpeername;
    zim_swoole_client_coro_close => close;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_types_drop_their_flags() {
        assert_eq!(socket_type(SOCK_TCP | (1 << 9)), SOCK_TCP, "SSL is a flag");
        assert_eq!(
            socket_type(SOCK_TCP6 | (1 << 12)),
            SOCK_TCP6,
            "KEEP is a flag"
        );
        assert_eq!(socket_type(7), 7, "unknown types stay unknown");
    }

    #[test]
    fn ports_verify_as_for_connect() {
        assert!(!verify_port(0), "connect needs a port");
        assert!(verify_port(65535), "the last port is valid");
        assert!(!verify_port(65536), "ports are 16-bit");
        assert!(!verify_port(-1), "ports are positive");
        assert!(
            verify_port((1 << 32) + 80),
            "upstream truncates to int first"
        );
    }

    #[test]
    fn data_matches_client_object() {
        assert_eq!(
            ClientObject::OFFSET,
            size_of::<Socket>() + 2 * size_of::<zval>(),
            "std follows the socket and two zvals"
        );
    }
}
