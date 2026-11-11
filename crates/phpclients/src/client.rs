//! `Swoole\Coroutine\Client`: registration, the object and every method. The
//! client owns a `Coroutine\Socket` (`zsocket`), created on connect
//! (`create_socket`, which also hooks the socket's close); each method parses
//! as upstream's ZPP block does and makes one call on the coroutine socket
//! (`crate::native`).
use crate::{
    native::{self, Co, Socket, Transfer},
    net::{SOCK_SSL, SOCK_TCP, SOCK_TCP6, SOCK_UNIX_STREAM, check_type, socket_type},
    socket::{Sock, strerror, warn},
};
use libc::c_int;
use std::{ffi::CStr, ptr};
use swoole_rs_phpstubs::client_coro;
use swoole_rs_reactor::SocketFields;
use swoole_rs_zend::{
    Array, Call, Class, Minit, Object, Owned, OwnedArray, Props, Value, ZString,
    bailout::Bailout,
    string::ZBuf,
    sys::{
        ZEND_ACC_NOT_SERIALIZABLE, ZEND_ACC_PUBLIC, zend_class_entry, zend_object,
        zend_object_handlers, zend_string,
    },
    unset_property_deny,
};
use swoole_rs_zend::{method_table, sys as z};

/// `SW_PHP_CLIENT_BUFFER_SIZE`.
const BUFFER_SIZE: i64 = 65535;
/// `SW_ERROR_CLIENT_NO_CONNECTION`.
const NO_CONNECTION: c_int = 5001;

/// `struct ClientCoroObject { SocketImpl *socket; zval zsocket; zval zobject; zend_object std; }`,
/// with the object itself as a pointer, not a zval.
#[repr(C)]
pub(crate) struct Data {
    socket: Socket,
    /// The `Coroutine\Socket` object, from connect until its close.
    zsocket: Owned,
    /// The client object itself, without a reference.
    object: *mut zend_object,
}

impl Data {
    /// The client's socket, while it has one.
    fn co(&self) -> Option<Co> {
        // SAFETY: a socket the client holds is live.
        (!self.socket.is_null()).then(|| unsafe { Co::new(self.socket) })
    }
}

type ClientObject = Object<Data>;

static CLIENT: Class = Class::new();

unsafe extern "C" fn create(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: Zend creates this class or a subclass; zeroed data is no socket.
    let std = unsafe { ClientObject::create(&CLIENT, ce) };
    // SAFETY: the object just created.
    unsafe { ClientObject::data(std) }.object = std;
    std
}

unsafe extern "C" fn free(std: *mut zend_object) {
    // SAFETY: Zend frees a client object.
    if let Some(socket) = unsafe { ClientObject::data(std) }.co() {
        socket.close();
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

swoole_rs_zend::minit!(client_coro_minit => register);

/// MINIT, as `php_swoole_client_coro_minit` did.
fn register(minit: &Minit) -> Result<(), Bailout> {
    minit.register(&CLIENT, c"Swoole\\Coroutine\\Client", &METHODS.0, configure);
    if swoole_rs_phpswoole::globals().use_shortname.get() {
        minit.alias(&CLIENT, c"Co\\Client")?;
    }
    minit.property_long(&CLIENT, c"errCode", 0, ZEND_ACC_PUBLIC);
    minit.property_string(&CLIENT, c"errMsg", c"", ZEND_ACC_PUBLIC);
    minit.property_long(&CLIENT, c"fd", -1, ZEND_ACC_PUBLIC);
    minit.property_null(&CLIENT, c"socket", ZEND_ACC_PUBLIC);
    minit.property_long(&CLIENT, c"type", SOCK_TCP, ZEND_ACC_PUBLIC);
    minit.property_null(&CLIENT, c"setting", ZEND_ACC_PUBLIC);
    minit.property_bool(&CLIENT, c"connected", false, ZEND_ACC_PUBLIC);
    for (name, value) in [
        (c"MSG_OOB", libc::MSG_OOB),
        (c"MSG_PEEK", libc::MSG_PEEK),
        (c"MSG_DONTWAIT", libc::MSG_DONTWAIT),
        (c"MSG_WAITALL", libc::MSG_WAITALL),
    ] {
        minit.class_constant_long(&CLIENT, name, i64::from(value));
    }
    Ok(())
}

fn this<'a>(call: &Call) -> &'a mut Data {
    // SAFETY: client methods are non-static, so `$this` is a client object, and
    // no other reference to its data is live during the call.
    unsafe { ClientObject::data(call.this()) }
}

/// `Address::verify_port(port, true)`, over upstream's `int` port.
fn verify_port(port: i64) -> bool {
    #[expect(clippy::cast_possible_truncation, reason = "upstream passes an int")]
    let port = port as i32;
    (1..=65535).contains(&port)
}

/// `php_swoole_socket_set_error_properties(ZEND_THIS, code, message)`.
fn report(call: &Call, code: c_int, message: &CStr) {
    report_object(props(call), code, message);
}

/// Property writes on `$this`, a client method's object, under the client
/// class; None in a static call.
fn props(call: &Call) -> Option<Props<'_>> {
    call.props(Some(&CLIENT))
}

impl Data {
    /// Property writes on the client object this data lives in.
    fn props(&self) -> Props<'_> {
        // SAFETY: the object holding this data, live while it is borrowed; the
        // class declares every name written.
        unsafe { Props::new(CLIENT.entry(), self.object) }
    }
}

/// `php_swoole_socket_set_error_properties(zobject, code, message)`.
fn report_object(props: Option<Props>, code: c_int, message: &CStr) {
    swoole_rs_corebase::log::set_last_error(code);
    let Some(props) = props.map(Props::own) else {
        return;
    };
    props.long(c"errCode", i64::from(code));
    props.bytes(c"errMsg", message.to_bytes());
}

/// `swoole_strerror(code)`.
fn error_message(code: c_int) -> &'static CStr {
    swoole_rs_phpruntime::strerror(code)
}

/// `php_swoole_socket_set_error_properties(ZEND_THIS, code)`: `swoole_strerror(code)`.
fn report_code(call: &Call, code: c_int) {
    report(call, code, error_message(code));
}

/// `php_swoole_socket_set_error_properties(ZEND_THIS, socket)`.
fn report_socket(call: &Call, socket: Co) {
    let (code, message) = socket.error();
    report(call, code, message);
}

/// The client's `setting` property, if it is an array.
fn setting(call: &Call) -> Option<&Array> {
    // SAFETY: `$this` is a live client; the class declares `setting`.
    let slot = unsafe { z::swr_zend_property_slot(call.this(), c"setting".as_ptr(), 7) };
    // SAFETY: null or the live property, which the call does not replace.
    unsafe { Value::from_ptr(slot) }.and_then(Value::array)
}

/// `zval_get_long` of the client's `type` property.
fn type_property(call: &Call) -> i64 {
    // SAFETY: `$this` is a live client; the class declares `type`.
    unsafe { swoole_rs_zend::property(call.this(), c"type") }.long()
}

/// `client_coro_socket_dtor`: the socket is going; the client lets go of it.
fn socket_closed(data: &mut Data) {
    if let Some(socket) = data.co()
        && let Err(bailout) = socket.free_length_callable()
    {
        bailout.defer();
    }
    data.socket = ptr::null_mut();
    let props = data.props().own();
    props.null(c"socket");
    props.bool(c"connected", false);
    drop(std::mem::take(&mut data.zsocket));
}

/// `client_coro_create_socket`: a `Coroutine\Socket` for the client's
/// `type`, published in its `socket` and `fd` properties; null on failure,
/// with errCode/errMsg set.
/// # Safety
/// `object` is a live client without a socket.
unsafe fn create_socket(object: *mut zend_object, kind: i64) -> Option<Co> {
    // SAFETY: per the caller.
    let data = unsafe { ClientObject::data(object) };
    #[expect(clippy::cast_possible_truncation, reason = "a socket type")]
    let socket_kind = socket_type(kind) as i32;
    let created = match crate::socket::owner::create_kind(socket_kind) {
        Ok(Some(created)) => created,
        Ok(None) => {
            let code = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            report_object(Some(data.props()), code, error_message(code));
            return None;
        }
        Err(bailout) => {
            bailout.defer();
            return None;
        }
    };
    let socket = Sock::of(&created).and_then(Sock::co)?;
    data.zsocket = created;
    let client = ptr::from_mut(data);
    swoole_rs_hooks::socket_extra::set_close_hook(
        socket.handle(),
        // SAFETY: the client outlives the hook on its socket, and no other
        // reference to it is live while the hook runs.
        Box::new(move |_| socket_closed(unsafe { &mut *client })),
    );
    let props = data.props().own();
    props.long(c"fd", i64::from(socket.handle().sock_fd()));
    props.value(c"socket", &data.zsocket);
    socket.use_zend_strings();
    socket.handle().set_zero_copy(true);
    if kind & SOCK_SSL != 0 && !socket.enable_ssl() {
        report_object(
            Some(data.props()),
            libc::EISCONN,
            error_message(libc::EISCONN),
        );
        socket_closed(data);
        return None;
    }
    Some(socket)
}

/// `CLIENT_CORO_GET_SOCKET_SAFE`: the connected socket, and a reference to
/// its object for the rest of the call, so a close inside it cannot free the
/// socket (`tmp_socket`); else errCode/errMsg say "Client no connection" and
/// the method returns false.
fn connected(call: &Call) -> Option<(Co, Owned)> {
    let zsocket = &this(call).zsocket;
    if let Some(socket) = Sock::of(zsocket).and_then(Sock::co) {
        return Some((socket, zsocket.copy()));
    }
    report_code(call, NO_CONNECTION);
    call.set_bool(false);
    None
}

/// `client_coro_get_socket_for_connect`: create, publish and configure the
/// client's socket; None (with the error reported) when it has one already, the
/// port is invalid or creation fails.
fn socket_for_connect(call: &Call, port: i64) -> Option<Co> {
    let data = this(call);
    if !data.socket.is_null() {
        report(call, libc::EISCONN, strerror(libc::EISCONN));
        return None;
    }
    let object = call.this();
    let kind = type_property(call);
    let tcp = matches!(socket_type(kind), SOCK_TCP | SOCK_TCP6);
    if tcp && !verify_port(port) {
        warn(true, "The port is invalid");
        return None;
    }
    // SAFETY: `$this` is live and has no socket.
    let socket = unsafe { create_socket(object, kind) }?;
    data.socket = socket.as_ptr();
    if let Some(settings) = setting(call) {
        crate::socket_options::apply(socket, settings, crate::socket_options::ALL);
    }
    Some(socket)
}

/// The socket for a datagram call: the client's, or a new one.
fn datagram_socket(call: &Call) -> Option<Co> {
    client_socket(call).or_else(|| socket_for_connect(call, 0))
}

/// The client's socket, if it has one.
fn client_socket(call: &Call) -> Option<Co> {
    this(call).co()
}

/// `__construct(int $type)`.
fn construct(call: &Call) {
    let Ok(kind) = call.arity(1, 1).and_then(|()| call.long(1, 0)) else {
        return call.set_bool(false);
    };
    if !check_type(kind) {
        return call.set_bool(false);
    }
    swoole_rs_phpruntime::check_reactor();
    if let Some(props) = props(call) {
        props.long(c"type", kind);
    }
    call.set_bool(true);
}

/// `__destruct()`: nothing; the socket closes when the object is freed.
fn destruct(_call: &Call) {}

/// `set(array $settings): bool`.
fn set(call: &Call) {
    let Ok(array) = call.arity(1, 1).and_then(|()| call.array_ref(1)) else {
        return call.set_bool(false);
    };
    if array.count() == 0 {
        return call.set_bool(false);
    }
    let settings = call.zval(1);
    // SAFETY: `$this` is a live client; `settings` is a live array.
    unsafe {
        z::swr_zend_property_merge(
            CLIENT.entry(),
            call.this(),
            c"setting".as_ptr(),
            7,
            settings,
        );
    }
    let Some(socket) = client_socket(call) else {
        return call.set_bool(true);
    };
    call.set_bool(crate::socket_options::apply(
        socket,
        array,
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
    // The coroutine socket's connect ignores `$sock_flag`, as upstream's does.
    let Ok((host, port, timeout, _flags)) = parsed else {
        return call.set_bool(false);
    };
    if host.is_empty() {
        warn(true, "The host is empty");
        return call.set_bool(false);
    }
    let Some(socket) = socket_for_connect(call, port) else {
        return call.set_bool(false);
    };
    if !socket.connect_host(host, port, timeout) {
        report_socket(call, socket);
        socket.close();
        return call.set_bool(false);
    }
    socket.set_timeout(timeout, native::TIMEOUT_RDWR);
    if let Some(props) = props(call) {
        props.bool(c"connected", true);
    }
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
    let sent = socket.send(Transfer::SendAll, data, timeout);
    let Ok(count) = usize::try_from(sent) else {
        report_socket(call, socket);
        return call.set_bool(false);
    };
    if count < data.len() && socket.error().0 != 0 {
        report_socket(call, socket);
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
    if socket
        .net()
        .is_some_and(|net| net.read(SocketFields::is_tcp))
        && !verify_port(port)
    {
        warn(true, "The port is invalid");
        return call.set_bool(false);
    }
    let sent = socket.sendto(host, port, data);
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
    let Some(length) = usize::try_from(length).ok().filter(|length| *length > 0) else {
        return call.set_bool(false);
    };
    let Some(socket) = datagram_socket(call) else {
        return call.set_bool(false);
    };
    let mut buffer = ZBuf::with_capacity(length);
    // A zero timeout leaves the socket's own.
    if socket.recvfrom(&mut buffer, 0.0) < 0 {
        report_socket(call, socket);
        return call.set_bool(false);
    }
    let (peer, peer_port) = socket.address();
    // SAFETY: nothing borrows arguments 2 and 3.
    unsafe { call.assign(2, Owned::string(ZString::new(peer.to_bytes()))) };
    if call.num_args() >= 3 {
        // SAFETY: as above.
        unsafe { call.assign(3, Owned::long(i64::from(peer_port))) };
    }
    call.set_zstring(buffer.into_zstring());
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
    let kind = i64::from(socket.kind());
    if !matches!(kind, SOCK_TCP | SOCK_TCP6 | SOCK_UNIX_STREAM) {
        if let Some(props) = props(call) {
            props.long(c"errCode", libc::EINVAL.into());
            props.bytes(c"errMsg", b"dgram socket cannot use sendfile");
        }
        return call.set_bool(false);
    }
    if socket.sendfile(file, offset, length) {
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
    let (received, result) = if socket.packet_mode() {
        let received = socket.recv_packet(timeout, false);
        match usize::try_from(received) {
            Ok(length @ 1..) => socket
                .pop_packet(length)
                .map_or((-1, None), |packet| (received, Some(packet))),
            _ => (received, None),
        }
    } else {
        let length = usize::try_from(BUFFER_SIZE).unwrap_or(0) - size_of::<zend_string>();
        let mut buffer = ZBuf::with_capacity(length);
        let received = socket.receive(Transfer::Recv, &mut buffer, timeout);
        (received, (received > 0).then(|| buffer.into_zstring()))
    };
    match result {
        _ if received < 0 => {
            report_socket(call, socket);
            call.set_bool(false);
        }
        Some(result) => call.set_zstring(result),
        None => call.set_empty_string(),
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
    let mut buffer = ZBuf::with_capacity(length);
    if socket.receive(Transfer::Peek, &mut buffer, 0.0) < 0 {
        report_socket(call, socket);
        return call.set_bool(false);
    }
    call.set_zstring(buffer.into_zstring());
}

/// `isConnected(): bool`.
fn is_connected(call: &Call) {
    call.set_bool(client_socket(call).is_some_and(Co::is_connected));
}

/// `["host" => .., "address" => .., "port" => ..]` into the return value
/// ("host" is kept for backward compatibility).
fn set_endpoint(call: &Call, address: &CStr, port: c_int) {
    let mut endpoint = OwnedArray::default();
    endpoint.insert_bytes(b"host", Some(address.to_bytes()));
    endpoint.insert_bytes(b"address", Some(address.to_bytes()));
    endpoint.insert_long(b"port", i64::from(port));
    call.set_owned(endpoint.into_owned());
}

/// `getsockname(): array|false`.
fn getsockname(call: &Call) {
    let Some((socket, _hold)) = connected(call) else {
        return;
    };
    if !socket.getsockname() {
        report_socket(call, socket);
        return call.set_bool(false);
    }
    let (address, port) = socket.address();
    set_endpoint(call, &address, port);
}

/// `getpeername(): array|false`.
fn getpeername(call: &Call) {
    let Some((socket, _hold)) = connected(call) else {
        return;
    };
    let Some((address, port)) = socket.getpeername() else {
        report_socket(call, socket);
        return call.set_bool(false);
    };
    set_endpoint(call, &address, port);
}

/// `exportSocket(): Socket|null`: the client's `Coroutine\Socket`, shared.
fn export_socket(call: &Call) {
    let zsocket = &this(call).zsocket;
    if zsocket.object().is_some() {
        call.set_owned(zsocket.copy());
    } else {
        call.set_null();
    }
}

/// `close(): bool`.
fn close(call: &Call) {
    let Some((socket, _hold)) = connected(call) else {
        return;
    };
    if let Some(props) = props(call) {
        props.own().bool(c"connected", false);
    }
    if !socket.close() {
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
    let kind = i64::from(socket.kind());
    if !matches!(kind, SOCK_TCP | SOCK_TCP6) {
        report_code(call, libc::ESOCKTNOSUPPORT);
        return call.set_bool(false);
    }
    if socket.has_ssl() || !crate::socket_options::enable_ssl(socket) {
        report_code(call, libc::EISCONN);
        return call.set_bool(false);
    }
    let settings = setting(call).filter(|settings| settings.count() > 0);
    if let Some(settings) = settings {
        crate::socket_options::apply(socket, settings, crate::socket_options::SSL);
    }
    if !socket.ssl_handshake() {
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
    if !socket.has_ssl() {
        report_code(call, libc::EISCONN);
        return call.set_bool(false);
    }
    match socket.peer_cert() {
        Some(cert) => call.set_zstring(cert),
        None => call.set_bool(false),
    }
}

/// `verifyPeerCert(bool $allow_self_signed = false): bool`; the socket is
/// checked first.
fn verify_peer_cert(call: &Call) {
    let Some((socket, _hold)) = connected(call) else {
        return;
    };
    if !socket.has_ssl() {
        report_code(call, libc::ENOTCONN);
        return call.set_bool(false);
    }
    let Ok(allow) = call.arity(0, 1).and_then(|()| call.bool(1, false)) else {
        return call.set_bool(false);
    };
    call.set_bool(socket.verify_peer(allow));
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
}
