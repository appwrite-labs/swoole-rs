//! WebSocket server registration, handshakes, sends and event dispatch.
use crate::{
    context::{self, Ctx},
    host::{self, CO_SOCKET},
    send,
};
use std::{
    ffi::{CStr, c_int},
    sync::{Arc, atomic::Ordering::Relaxed},
};
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::log::LogLevel::SW_LOG_TRACE as LOG_TRACE;
use swoole_rs_http::status::{BAD_REQUEST, SWITCHING_PROTOCOLS};
use swoole_rs_phpserver::PortCallback;
use swoole_rs_phpserver::{RecvData, php_swoole_get_recv_data, php_swoole_server_of, zserv};
use swoole_rs_phpstubs::websocket as stubs;
use swoole_rs_phpswoole::MinitArgs;
use swoole_rs_server::{Open, WebSocket, server_methods, view};
use swoole_rs_websocket::{Delivery, ENCODED_KEY_LEN, Opcode, message_policy};
use swoole_rs_zend::{
    Call, Class, Owned, Value, ZStr, ZString, bailout, handlers, method_table,
    sys::{ZEND_ACC_NOT_SERIALIZABLE, ZEND_ACC_PUBLIC, ZEND_ACC_STATIC, zend_long},
};

pub use swoole_rs_phpclients::websocket::FrameObject;
pub(crate) use swoole_rs_phpclients::websocket::{
    Assembly, FLAG_FIN, IN_SERVER, OPEN_CLOSE_FRAME, OPEN_PING_FRAME, OPEN_PONG_FRAME,
    SETTING_COMPRESSION, encode, has_zlib, recv,
};
use swoole_rs_phpclients::websocket::{
    CLOSE_NORMAL, FLAG_RSV1, close_frame, deliver, pack, register_frames, uncompress,
    zim_swoole_websocket_server_pack, zim_swoole_websocket_server_unpack,
};
static SERVER: Class = Class::new();
/// Swoole error codes and log levels (`include/swoole_error.h`, `swoole_log.h`).
const SESSION_NOT_EXIST: c_int = swoole_rs_error::ErrorCode::SW_ERROR_SESSION_NOT_EXIST as c_int;
const WEBSOCKET_UNCONNECTED: c_int =
    swoole_rs_error::ErrorCode::SW_ERROR_WEBSOCKET_UNCONNECTED as c_int;
const WEBSOCKET_PACK_FAILED: c_int =
    swoole_rs_error::ErrorCode::SW_ERROR_WEBSOCKET_PACK_FAILED as c_int;
/// `Swoole\WebSocket\Server`, once MINIT registered it; crates/phpserver
/// tells a WebSocket server by it.
#[must_use]
pub fn websocket_class() -> &'static Class {
    &SERVER
}

/// What the methods read of a `swoole::Connection`.
#[derive(Clone, Copy)]
struct Connection {
    server_fd: c_int,
    compression: bool,
}

/// `php_swoole_server_get_and_check_server(object)`: `None` after the
/// `E_ERROR` of a server object never constructed. The view lives while
/// `_held`, which holds the object (the method's frame, or the value), is
/// borrowed.
pub(crate) fn server_of(object: &Value) -> Option<view::Server<'_>> {
    let server = php_swoole_server_of(object);
    if server.is_none() {
        let name = object.class_name().map(CStr::to_string_lossy);
        let name = name.unwrap_or_default();
        let message = format!("Invalid instance of {name}");
        host::set_last_error(host::PHP_FATAL_ERROR);
        let message = std::ffi::CString::new(message).unwrap_or_default();
        bailout::defer(swoole_rs_zend::report(swoole_rs_zend::E_ERROR, &message));
    }
    server
}

/// `Server::close(fd, reset)`.
fn close(server: view::Server<'_>, fd: zend_long, reset: bool) -> bool {
    server_methods::close(server, fd, reset)
}

/// The `websocket_subprotocol` of the port listening on `server_fd`.
fn port_protocol(server: view::Server<'_>, server_fd: c_int) -> Option<Arc<Vec<u8>>> {
    server
        .port_of_server_fd(server_fd)
        .and_then(|port| port.websocket_protocol.get())
}

method_table! {
    static SERVER_METHODS = [
        c"push" => zim_swoole_websocket_server_push, &stubs::arginfo_class_Swoole_WebSocket_Server_push, ZEND_ACC_PUBLIC;
        c"disconnect" => zim_swoole_websocket_server_disconnect, &stubs::arginfo_class_Swoole_WebSocket_Server_disconnect, ZEND_ACC_PUBLIC;
        c"ping" => zim_swoole_websocket_server_ping, &stubs::arginfo_class_Swoole_WebSocket_Server_ping, ZEND_ACC_PUBLIC;
        c"isEstablished" => zim_swoole_websocket_server_isEstablished, &stubs::arginfo_class_Swoole_WebSocket_Server_isEstablished, ZEND_ACC_PUBLIC;
        c"pack" => zim_swoole_websocket_server_pack, &stubs::arginfo_class_Swoole_WebSocket_Server_pack, ZEND_ACC_PUBLIC | ZEND_ACC_STATIC;
        c"unpack" => zim_swoole_websocket_server_unpack, &stubs::arginfo_class_Swoole_WebSocket_Server_unpack, ZEND_ACC_PUBLIC | ZEND_ACC_STATIC;
    ];
}

/// MINIT: the three classes and the constants, as
/// `php_swoole_websocket_server_minit` registered them.
/// After `Swoole\Http\Server` is registered.
pub fn websocket_minit(args: &MinitArgs) {
    let serializable = args.thread;
    let minit = args.minit;
    // Its objects are the parent's (no handlers of its own are installed).
    let http_server = minit.parent(&crate::server::SERVER);
    minit.register_child(
        &SERVER,
        c"Swoole\\WebSocket\\Server",
        SERVER_METHODS.table(),
        http_server,
        |entry, _| {
            if !serializable {
                entry.ce_flags |= ZEND_ACC_NOT_SERIALIZABLE;
            }
        },
    );
    register_frames(minit);
}

/// A value holding `string`, or the empty string for none.
fn string_or_empty(string: Option<&ZStr>) -> Owned {
    Owned::string(string.map_or_else(ZString::empty, ZStr::to_owned))
}

/// `php_swoole_server_get_and_check_server(ZEND_THIS)` and the running check.
fn server<'c>(call: &'c Call<'_>) -> Option<view::Server<'c>> {
    let server = server_of(call.this_value()?)?;
    if !server.is_started() {
        host::fatal("server is not running");
        return None;
    }
    Some(server)
}

/// `Server::get_connection_verify(fd) != nullptr`.
pub(crate) fn server_connection(server: view::Server<'_>, fd: zend_long) -> bool {
    connection(server, fd, true).is_some()
}

fn connection(server: view::Server<'_>, fd: zend_long, verify: bool) -> Option<Connection> {
    let conn = if verify {
        server.verified(fd)
    } else {
        server.by_session(fd)
    }?;
    Some(Connection {
        server_fd: conn.server_fd(),
        compression: conn.websocket_compression(),
    })
}

/// `swoole_websocket_server_push`: a packed frame to a WebSocket connection, as its
/// `head` and the `payload` that follows it (all of it in `head`, for a small one).
fn push_frame(server: view::Server<'_>, fd: zend_long, head: &[u8], payload: &[u8]) -> bool {
    if fd <= 0 {
        host::fatal(&format!("fd[{fd}] is invalid"));
        return false;
    }
    // Past the start of its handshake.
    let unconnected = |conn: swoole_rs_server::Connection<'_>| {
        matches!(conn.websocket(), WebSocket::Http | WebSocket::Requested(_))
    };
    if server.by_session(fd).is_none_or(unconnected) {
        host::set_last_error(WEBSOCKET_UNCONNECTED);
        host::fatal(&format!(
            "the connected client of connection[{fd}] is not a websocket client or closed"
        ));
        return false;
    }
    context::server_send_parts(server, fd, head, payload)
}

fn push(call: &Call) {
    let run = || -> Option<bool> {
        let server = server(call)?;
        call.arity(2, 4).ok()?;
        let fd = call.long(1, 0).ok()?;
        let data = call.value(2);
        let opcode = call.long(3, i64::from(Opcode::Text.code())).ok()?;
        let flags = call
            .zval_or_null(4)
            .map_or(i64::from(FLAG_FIN), Value::long);
        let Some(conn) = connection(server, fd, true) else {
            host::set_last_error(SESSION_NOT_EXIST);
            host::fatal(&format!("session#{fd} does not exists"));
            return None;
        };
        let frame = FrameObject::server(data, opcode, flags, conn.compression);
        let sent = frame.send_parts(|head, payload| push_frame(server, fd, head, payload));
        if sent.is_none() {
            host::set_last_error(WEBSOCKET_PACK_FAILED);
        }
        sent
    };
    call.set_bool(run().unwrap_or(false));
}

fn disconnect(call: &Call) {
    let run = || -> Option<bool> {
        let server = server(call)?;
        call.arity(1, 3).ok()?;
        let fd = call.long(1, 0).ok()?;
        let code = call.long(2, CLOSE_NORMAL).ok()?;
        let reason = call.string(3, b"").ok()?;
        let frame = close_frame(code.c_cast::<i32>(), reason, 0)?;
        if !push_frame(server, fd, frame.bytes(), &[]) {
            return Some(false);
        }
        // Mark it closing at once, so it is not closed twice.
        server.by_session(fd)?.websocket_closing();
        Some(close(server, fd, false))
    };
    call.set_bool(run().unwrap_or(false));
}

fn ping(call: &Call) {
    let run = || -> Option<bool> {
        let server = server(call)?;
        call.arity(1, 2).ok()?;
        let fd = call.long(1, 0).ok()?;
        let data = string_or_empty(call.zstr(2).ok()?);
        let frame = FrameObject::new(
            Some(&data),
            i64::from(Opcode::Ping.code()),
            i64::from(FLAG_FIN),
            0,
        );
        let Some(packed) = pack(&frame) else {
            host::set_last_error(WEBSOCKET_PACK_FAILED);
            return None;
        };
        Some(push_frame(server, fd, packed.bytes(), &[]))
    };
    call.set_bool(run().unwrap_or(false));
}

fn is_established(call: &Call) {
    let run = || -> Option<bool> {
        let server = server(call)?;
        let fd = call.arity(1, 1).and_then(|()| call.long(1, 0)).ok()?;
        let established = |conn: swoole_rs_server::Connection<'_>| {
            let state = conn.websocket();
            conn.closed().load(Relaxed) == 0
                && matches!(
                    state,
                    WebSocket::Open(_) | WebSocket::Closing(_) | WebSocket::Failed
                )
        };
        Some(server.verified(fd).is_some_and(established))
    };
    call.set_bool(run().unwrap_or(false));
}

handlers! {
    zim_swoole_websocket_server_push => push;
    zim_swoole_websocket_server_disconnect => disconnect;
    zim_swoole_websocket_server_ping => ping;
    zim_swoole_websocket_server_isEstablished => is_established;
}

/// Run a server callback; `Err` if it bailed out. A failed call warns
/// `"Swoole\WebSocket\Server->{event} handler error"` and closes `fd`.
fn dispatch(
    server: view::Server<'_>,
    server_fd: c_int,
    event: PortCallback,
    args: swoole_rs_zend::Zvals<'_>,
    fd: zend_long,
) -> Result<(), bailout::Bailout> {
    if crate::server::call_port(server, server_fd, event, args)? == Some(false) {
        let name = match event {
            PortCallback::BeforeHandshakeResponse => "onBeforeHandshakeResponse",
            PortCallback::Open => "onOpen",
            _ => "onMessage",
        };
        host::notice(&format!("Swoole\\WebSocket\\Server->{name} handler error"));
        close(server, fd, false);
    }
    Ok(())
}

/// `swoole_websocket_onMessage`: a frame `Server::dispatch_frame` delivered,
/// answered by policy or handed to `onMessage` as a Frame. A bailout in the
/// handler is raised last, over this frame only.
/// Only an open connection has frames.
pub(crate) fn on_message(
    server: view::Server<'_>,
    open: Open<'_>,
    request: &mut RecvData<'_>,
) -> bool {
    let result = message(server, open, request);
    let ok = *result.as_ref().unwrap_or(&true);
    bailout::defer(result.map(|_| ()));
    host::stop_if_bailed();
    ok
}

/// `_open`: the frame's connection is open, as only an open one has frames.
fn message(
    view: view::Server<'_>,
    _open: Open<'_>,
    request: &mut RecvData<'_>,
) -> Result<bool, bailout::Bailout> {
    let info = request.info();
    let (fd, server_fd, ext_flags) = (info.fd, c_int::from(info.server_fd), info.ext_flags);
    let server = view;
    let Some(port) = server
        .by_session(fd)
        .and_then(|conn| server.port_of_fd(conn.fd()))
    else {
        return Ok(false);
    };
    let mut data = php_swoole_get_recv_data(view, request);
    let [opcode, flags] = ext_flags.to_be_bytes();
    let delivery = message_policy(Opcode::from_code(opcode), port.websocket_frames.get());
    if delivery != Delivery::Deliver {
        if delivery == Delivery::Pong
            && let Some(pong) = encode(
                data.bytes().unwrap_or_default(),
                Opcode::Pong.code(),
                FLAG_FIN,
            )
        {
            server_methods::send(server, fd, pong.bytes());
        }
        return Ok(true);
    }
    // RFC 7692: the payload is deflated.
    if flags & FLAG_RSV1 != 0 {
        let max = port.protocol.get().package_max_length as usize;
        let Some(plain) = uncompress(data.bytes().unwrap_or_default(), max) else {
            return Ok(true);
        };
        data = plain;
    }
    let frame = deliver(fd, opcode, data, flags);
    // The handler borrows the Frame, whose drop releases it.
    let server_object = zserv();
    let called = swoole_rs_zend::Zvals::with([&server_object, &frame], |args| {
        dispatch(server, server_fd, PortCallback::Message, args, fd)
    });
    // Server::start's frame keeps the server live through its callbacks.
    // Give back only this temporary hold, without running a destructor trap.
    server_object.delref();
    if let Err(bailout) = called {
        // As upstream, whose bailout jumps past the releases.
        let _kept = frame.into_zval();
        return Err(bailout);
    }
    Ok(true)
}

/// `swoole_websocket_onHandshake`: the handshake, then `onOpen`, or a reset.
pub(crate) fn handshake_open(mut context: Ctx) -> Result<(), bailout::Bailout> {
    let server = context::server(context.state());
    let fd = context.fd();
    if handshake(context)? {
        on_open(server, context)
    } else {
        close(server, fd, true);
        Ok(())
    }
}

/// Set a response header as `Context::set_header(..., false)`.
fn set_header(context: Ctx, key: &[u8], value: &[u8]) -> Result<(), bailout::Bailout> {
    crate::response::set_header_bytes(context, key, value);
    if bailout::pending() {
        Err(bailout::Bailout)
    } else {
        Ok(())
    }
}

fn handshake(mut context: Ctx) -> Result<bool, bailout::Bailout> {
    let header = crate::parse::made(&context.state().request, crate::parse::Property::Header);
    let key = header
        .as_deref()
        .and_then(Value::array)
        .and_then(|header| header.find(b"sec-websocket-key"));
    let key = key.map(Value::string);
    if key.is_some() && bailout::pending() {
        return Err(bailout::Bailout);
    }
    let key = key.filter(|key| key.bytes().len() == ENCODED_KEY_LEN);
    let Some(key) = key else {
        context.state().response.status = BAD_REQUEST;
        send::end(context, None)?;
        return Ok(false);
    };
    let accept = swoole_rs_websocket::accept(key.bytes());
    drop(key);
    set_header(context, b"Upgrade", b"websocket")?;
    set_header(context, b"Connection", b"Upgrade")?;
    set_header(context, b"Sec-WebSocket-Accept", &accept)?;
    set_header(context, b"Sec-WebSocket-Version", b"13")?;
    let fd = context.fd();
    if context.flags() & CO_SOCKET == 0 {
        let server = context::server(context.state());
        let Some(conn) = server.by_session(fd) else {
            closed(c"swoole_websocket_handshake", fd);
            return Ok(false);
        };
        // It opens as the 101 goes ([`switching`]); one that lost to a `disconnect`
        // finds no handshake there.
        if let WebSocket::Requested(requested) = conn.websocket() {
            _ = requested.handshake();
        }
        let server_fd = conn.server_fd();
        if let Some(protocol) = port_protocol(server, server_fd)
            && !protocol.is_empty()
        {
            set_header(context, b"Sec-WebSocket-Protocol", &protocol)?;
        }
        let state = context.state();
        let server_object = zserv();
        let called = swoole_rs_zend::Zvals::with(
            [
                &server_object,
                state.request.zobject.value(),
                state.response.zobject.value(),
            ],
            |args| {
                dispatch(
                    server,
                    server_fd,
                    PortCallback::BeforeHandshakeResponse,
                    args,
                    fd,
                )
            },
        );
        // The active Server::start frame still holds this object.
        server_object.delref();
        called?;
    } else {
        swoole_rs_phpclients::websocket_protocol(&context.state().zsocket);
    }
    let state = context.state();
    state.response.status = SWITCHING_PROTOCOLS;
    state.upgrade = true;
    send::end(context, None)
}

/// `swoole_error_log(SW_LOG_TRACE, SW_ERROR_SESSION_NOT_EXIST, ...)`.
fn closed(func: &CStr, fd: zend_long) {
    host::log(
        LOG_TRACE,
        SESSION_NOT_EXIST,
        func,
        format!("session[{fd}] is closed"),
    );
}

/// `swoole_websocket_onOpen`.
fn on_open(server: view::Server<'_>, mut context: Ctx) -> Result<(), bailout::Bailout> {
    let fd = context.fd();
    let Some(conn) = connection(server, fd, false) else {
        closed(c"swoole_websocket_onOpen", fd);
        return Ok(());
    };
    let server_object = zserv();
    swoole_rs_zend::Zvals::with(
        [&server_object, context.state().request.zobject.value()],
        |args| dispatch(server, conn.server_fd, PortCallback::Open, args, fd),
    )
}

/// `swoole_websocket_onRequest`, the default `onRequest`: 400 and close.
pub(crate) fn request(mut context: Ctx) {
    static RESPONSE: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        format!(
            "HTTP/1.1 400 Bad Request\r\n\
        Connection: close\r\n\
        Content-Type: text/html; charset=UTF-8\r\n\
        Cache-Control: must-revalidate,no-cache,no-store\r\n\
        Content-Length: 83\r\n\
        Server: {}\r\n\r\n\
        <html><body><h2>HTTP 400 Bad Request</h2><hr><i>Powered by Swoole</i></body></html>",
            swoole_rs_corebase::constants::SW_HTTP_SERVER_SOFTWARE
        )
    });
    context::send(context, RESPONSE.as_bytes());
    context.state().end = true;
    context::close(context);
}

/// The coroutine `Response::push`: a frame over the context's socket.
pub(crate) fn response_push(
    mut context: Ctx,
    data: &Value,
    opcode: i64,
    flags: Option<&Value>,
) -> bool {
    let flags = flags.map_or(i64::from(FLAG_FIN), Value::long);
    let frame = FrameObject::server(
        Some(data),
        opcode,
        flags,
        context.state().websocket_compression,
    );
    send_frame(context, &frame).unwrap_or(false)
}

/// `http_response_send_frame`: `None` when the frame cannot be packed.
fn send_frame(context: Ctx, frame: &FrameObject) -> Option<bool> {
    let Some(packed) = pack(frame) else {
        host::set_last_error(WEBSOCKET_PACK_FAILED);
        return None;
    };
    let packed = packed.bytes();
    Some(context::send(context, packed))
}

/// The coroutine `Response::disconnect`: a close frame, then close.
pub(crate) fn response_disconnect(context: Ctx, code: i64, reason: Option<&ZStr>) -> bool {
    let data = string_or_empty(reason);
    let frame = FrameObject::new(
        Some(&data),
        i64::from(Opcode::Close.code()),
        i64::from(FLAG_FIN),
        code,
    );
    let Some(sent) = send_frame(context, &frame) else {
        return false;
    };
    context::close(context);
    sent
}

/// Match an extension name before its parameters. Commas inside a quoted
/// parameter are data, including after an escaped quote.
fn offers_compression(mut header: &[u8]) -> bool {
    while !header.is_empty() {
        let end = header
            .iter()
            .position(|b| matches!(b, b';' | b','))
            .unwrap_or(header.len());
        let (name, parameters) = header.split_at(end);
        let mut token = name
            .split(|b| matches!(b, b' ' | b'\t'))
            .filter(|s| !s.is_empty());
        if token.next() == Some(b"permessage-deflate") && token.next().is_none() {
            return true;
        }
        let mut params = parameters.iter();
        let mut quoted = false;
        while let Some(byte) = params.next() {
            match byte {
                b'\\' if quoted => {
                    params.next();
                }
                b'"' => quoted = !quoted,
                b',' if !quoted => break,
                _ => {}
            }
        }
        header = params.as_slice();
    }
    false
}

/// The coroutine `Response::upgrade`'s handshake.
/// The handshake response's `permessage-deflate` answer, when built with zlib
/// and offered: the request asked for it, so the response says so and the
/// connection compresses.
pub(crate) fn accept(mut context: Ctx) {
    if !has_zlib() {
        return;
    }
    let state = context.state();
    let offered = if context::co_socket(state) {
        state.websocket_settings & SETTING_COMPRESSION != 0
    } else {
        crate::context::server(state)
            .config()
            .websocket_compression
            .get()
    };
    let mut accepted = false;
    if offered && state.request.zobject.get().is_some() {
        let header = crate::parse::made(&state.request, crate::parse::Property::Header);
        let text = header
            .as_deref()
            .and_then(Value::array)
            .and_then(|header| header.find(b"sec-websocket-extensions"))
            .and_then(Value::bytes);
        if text.is_some_and(offers_compression) {
            accepted = true;
            crate::response::set_header_bytes(
                context,
                b"Sec-Websocket-Extensions",
                b"permessage-deflate; client_no_context_takeover; server_no_context_takeover",
            );
        }
    }
    let state = context.state();
    state.websocket_compression = accepted;
    if !crate::context::co_socket(state)
        && let Some(conn) = crate::context::server(state).verified(state.fd)
    {
        conn.set_websocket_compression(accepted);
    }
}

/// The server session's connection, while its handshake runs.
fn handshaking(context: &mut Ctx) -> Option<swoole_rs_server::Handshaking<'static>> {
    let state = context.state();
    let conn = crate::context::server(state).verified(state.fd)?;
    match conn.websocket() {
        WebSocket::Handshaking(handshaking) => Some(handshaking),
        _ => None,
    }
}

/// Before a server handshake's 101 is built: the client may send frames as soon as
/// it is on the wire, so the connection opens now, and nothing it sent before is
/// read as a frame.
pub(crate) fn switching(mut context: Ctx) {
    if context.state().response.status == SWITCHING_PROTOCOLS
        && let Some(handshaking) = handshaking(&mut context)
    {
        // Lost to a `disconnect`: it stays closing, and nothing is framed.
        _ = handshaking.open();
    }
}

/// After a server handshake response: the connection is a WebSocket on 101,
/// otherwise the handshake failed and the connection closes.
pub(crate) fn handshaked(mut context: Ctx) {
    let Some(handshaking) = handshaking(&mut context) else {
        return;
    };
    let state = context.state();
    if state.response.status == SWITCHING_PROTOCOLS {
        // As in `switching`, one that lost stays closing.
        _ = handshaking.open();
    } else {
        state.keepalive = false;
        handshaking.fail();
    }
}

pub(crate) fn upgrade(context: Ctx) -> Result<bool, bailout::Bailout> {
    handshake(context)
}
