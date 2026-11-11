//! `Swoole\Async\Client`: a `Swoole\Client` whose `network::Client` runs on
//! the reactor and raises PHP callbacks. It shares the parent's object
//! ([`Data`]); the callbacks are callables by slot, and the events the
//! client raises arrive through `async_event`.
use crate::{
    native::{self, ErrorKind, Run},
    net::{
        self, CONNECT_TIMEOUT, Conn, INVALID_PARAMS, Obj, SOCK_TCP, SOCK_TCP6, bailed, check_type,
        destructing, errno, last_error, report, set_err_code, set_last_error, socket_type,
        strerror, type_property, warn, warn_sys,
    },
};
use libc::{c_char, c_int, c_void};
use std::{ffi::CStr, ptr, rc::Rc};
use swoole_rs_clients::client::{
    self as state, ON_BUFFER_EMPTY, ON_BUFFER_FULL, ON_CLOSE, ON_CONNECT, ON_ERROR, ON_RECEIVE, Op,
    SSL_STATE_WAIT_STREAM,
};
use swoole_rs_clients::swr_v1_clients_port;
use swoole_rs_phpstubs::client_async;
use swoole_rs_zend::{
    Call, Class, E_ERROR, Minit, Owned, ZString,
    bailout::{self, Bailout},
    host::callable::Callable,
    set_object,
    sys::{
        ZEND_ACC_NOT_SERIALIZABLE, ZEND_ACC_PRIVATE, zend_class_entry, zend_object_handlers, zval,
        zval_ptr_dtor,
    },
    unset_property_deny,
};
use swoole_rs_zend::{method_table, sys as z};

static ASYNC: Class = Class::new();

/// Callback slots (`CLIENT_CB_*` less one) and their property names.
const CONNECT: usize = 0;
const RECEIVE: usize = 1;
const CLOSE: usize = 2;
const ERROR: usize = 3;
const BUFFER_FULL: usize = 4;
const BUFFER_EMPTY: usize = 5;
const SSL_READY: usize = 6;
const NAMES: [&CStr; 7] = [
    c"onConnect",
    c"onReceive",
    c"onClose",
    c"onError",
    c"onBufferFull",
    c"onBufferEmpty",
    c"onSSLReady",
];
/// The names `on()` accepts, by slot, matched as upstream's `strncasecmp`.
const EVENTS: [&[u8]; 6] = [
    b"connect",
    b"receive",
    b"close",
    b"error",
    b"bufferfull",
    b"bufferempty",
];

/// `SW_ERROR_DNSLOOKUP_RESOLVE_FAILED`.
const DNS_RESOLVE_FAILED: c_int = 711;

fn configure(entry: &mut zend_class_entry, handlers: &mut zend_object_handlers) {
    entry.ce_flags |= ZEND_ACC_NOT_SERIALIZABLE;
    handlers.clone_obj = None;
    handlers.unset_property = Some(unset_property_deny);
}

method_table! {
    static METHODS = [
        c"__construct" => zim_swoole_client_async___construct, &client_async::arginfo_class_Swoole_Async_Client___construct, z::ZEND_ACC_PUBLIC;
        c"__destruct" => zim_swoole_client_async___destruct, &client_async::arginfo_class_Swoole_Async_Client___destruct, z::ZEND_ACC_PUBLIC;
        c"connect" => zim_swoole_client_async_connect, &client_async::arginfo_class_Swoole_Async_Client_connect, z::ZEND_ACC_PUBLIC;
        c"sleep" => zim_swoole_client_async_sleep, &client_async::arginfo_class_Swoole_Async_Client_sleep, z::ZEND_ACC_PUBLIC;
        c"wakeup" => zim_swoole_client_async_wakeup, &client_async::arginfo_class_Swoole_Async_Client_wakeup, z::ZEND_ACC_PUBLIC;
        c"pause" => zim_swoole_client_async_sleep, &client_async::arginfo_class_Swoole_Async_Client_sleep, z::ZEND_ACC_PUBLIC;
        c"resume" => zim_swoole_client_async_wakeup, &client_async::arginfo_class_Swoole_Async_Client_wakeup, z::ZEND_ACC_PUBLIC;
        c"enableSSL" => zim_swoole_client_async_enableSSL, &client_async::arginfo_class_Swoole_Async_Client_enableSSL, z::ZEND_ACC_PUBLIC;
        c"isConnected" => zim_swoole_client_async_isConnected, &client_async::arginfo_class_Swoole_Async_Client_isConnected, z::ZEND_ACC_PUBLIC;
        c"close" => zim_swoole_client_async_close, &client_async::arginfo_class_Swoole_Async_Client_close, z::ZEND_ACC_PUBLIC;
        c"on" => zim_swoole_client_async_on, &client_async::arginfo_class_Swoole_Async_Client_on, z::ZEND_ACC_PUBLIC;
    ];
}

swoole_rs_zend::minit!(php_swoole_client_async_minit => register);

/// `php_swoole_client_async_minit`, after `Swoole\Client`'s: objects come from
/// the parent's `create_object`, which Zend inherits.
#[expect(clippy::unnecessary_wraps, reason = "the shape `minit!` takes")]
fn register(minit: &Minit) -> Result<(), Bailout> {
    minit.register_child(
        &ASYNC,
        c"Swoole\\Async\\Client",
        &METHODS.0,
        minit.parent(&net::CLIENT),
        configure,
    );
    for name in [
        c"onConnect",
        c"onError",
        c"onReceive",
        c"onClose",
        c"onBufferFull",
        c"onBufferEmpty",
        c"onSSLReady",
    ] {
        minit.property_null(&ASYNC, name, ZEND_ACC_PRIVATE);
    }
    Ok(())
}

/// A reference to the callback in `slot`, which keeps it through the call even
/// if the call replaces it.
fn callback(data: &net::Data, slot: usize) -> Option<Rc<Callable>> {
    let cell = data.callbacks.get(slot)?;
    let callable = cell.take();
    cell.set(callable.clone());
    callable
}

/// `php_swoole_client_async_free_object`: free every callback, all of them
/// even when a destructor bails out (each drop defers its bailout).
pub(crate) fn free_callbacks(object: Obj) {
    for slot in &object.data().callbacks {
        drop(slot.take());
    }
}

/// Call `callable` with `argv`: whether the call succeeded.
fn invoke(callable: &Callable, argv: &mut [zval]) -> Result<bool, Bailout> {
    native::call(callable, argv, ptr::null_mut(), true)
}

fn object_zval(object: Obj) -> zval {
    let mut zobject = swoole_rs_zend::long_zval(0);
    // SAFETY: a borrowed copy of the object, as upstream's `args[0] = *zobject`.
    unsafe { set_object(&raw mut zobject, object.as_ptr()) };
    zobject
}

/// `client_execute_callback`: run the slot's callback with the object.
fn execute(object: Obj, slot: usize) -> Result<(), Bailout> {
    let class = object.class_name().to_string_lossy();
    let name = NAMES.get(slot).copied().unwrap_or(c"").to_string_lossy();
    let Some(callable) = callback(object.data(), slot) else {
        warn(ErrorKind::Fatal, &format!("{class} has no {name} callback"));
        return Ok(());
    };
    if !invoke(&callable, &mut [object_zval(object)])? {
        warn(ErrorKind::Fatal, &format!("{class}->{name} handler error"));
    }
    Ok(())
}

/// `client_execute_callback_and_dtor`: the callback, then free the connection
/// and drop the reference `connect()` took.
fn execute_and_release(object: Obj, slot: usize) -> Result<(), Bailout> {
    execute(object, slot)?;
    if let Some(conn) = object.data().conn() {
        net::release(object, conn)?;
    }
    // SAFETY: `zval_delref_p`: the reference `connect()` took.
    unsafe { (*object.as_ptr()).gc.refcount -= 1 };
    Ok(())
}

fn event(object: Obj, kind: c_int, data: *const c_char, length: usize) -> Result<(), Bailout> {
    match kind {
        ON_CONNECT => {
            if let Some(conn) = object.data().conn()
                && conn.ssl_wait.replace(false)
            {
                return execute(object, SSL_READY);
            }
            execute(object, CONNECT)
        }
        ON_RECEIVE => {
            let callable = callback(object.data(), RECEIVE);
            // SAFETY: the client lends `length` bytes for the call.
            let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), length) };
            let payload = Owned::string(ZString::new(bytes)).into_zval();
            let mut argv = [object_zval(object), payload];
            let called = callable.map_or(Ok(false), |callable| invoke(&callable, &mut argv));
            // SAFETY: our reference to the payload.
            unsafe { zval_ptr_dtor(&raw mut argv[1]) };
            if !called? {
                let class = object.class_name().to_string_lossy();
                warn(
                    ErrorKind::Fatal,
                    &format!("{class}->onReceive handler error"),
                );
            }
            Ok(())
        }
        ON_CLOSE => execute_and_release(object, CLOSE),
        ON_ERROR => {
            // The error code arrives as the length (`swoole_get_last_error()`).
            let error = i64::try_from(length).unwrap_or(0);
            object.props().long(c"errCode", error);
            execute_and_release(object, ERROR)
        }
        ON_BUFFER_FULL => execute(object, BUFFER_FULL),
        ON_BUFFER_EMPTY => execute(object, BUFFER_EMPTY),
        _ => std::process::abort(),
    }
}

/// An event `network::Client` raised on an async client's object; a PHP
/// bailout is re-raised as the last step.
/// # Safety
/// `object` is the live client object `connect()` bound; `data` holds
/// `length` bytes for [`ON_RECEIVE`].
unsafe extern "C" fn async_event(
    object: *mut c_void,
    kind: c_int,
    data: *const c_char,
    length: usize,
) {
    // SAFETY: per the caller.
    let object = unsafe { Obj::new(object.cast()) };
    bailout::raise(event(object, kind, data, length));
}

/// `__construct(int $type)`.
fn construct(call: &Call) {
    let Ok(kind) = call.arity(1, 1).and_then(|()| call.long(1, 0)) else {
        call.throw_error(c"socket type param is required");
        return call.set_bool(false);
    };
    if !check_type(kind) {
        return call.set_bool(false);
    }
    swoole_rs_phpruntime::check_reactor();
    Obj::this(call).props().long(c"type", kind);
    call.set_bool(true);
}

/// `__destruct()`: close and free a held connection.
fn destruct(call: &Call) {
    if !destructing(call) {
        return;
    }
    let Some(conn) = net::this(call).conn() else {
        return;
    };
    if !conn.flag(state::CLOSED) && bailed(conn.call(Op::Close).run()).is_none() {
        return;
    }
    // A close callback may have freed it already.
    if let Some(conn) = net::this(call).conn() {
        bailed(net::release(Obj::this(call), conn));
    }
}

/// `php_swoole_client_async_new`: an async client for the object's type.
fn connection(object: Obj, port: i64) -> Option<Rc<Conn>> {
    let kind = type_property(object, true)?;
    #[expect(clippy::cast_possible_truncation, reason = "upstream passes an int")]
    let int_port = port as i32;
    if matches!(socket_type(kind), SOCK_TCP | SOCK_TCP6) && swr_v1_clients_port(int_port, 1) == 0 {
        warn(ErrorKind::Fatal, "The port is invalid");
        set_last_error(INVALID_PARAMS);
        return None;
    }
    #[expect(clippy::cast_possible_truncation, reason = "a socket type")]
    let cli = match native::create(socket_type(kind) as c_int, true) {
        Ok(cli) => cli,
        Err(error) => {
            warn_sys("failed to create client", error);
            set_err_code(object, error);
            return None;
        }
    };
    let conn = Conn::new(cli);
    object.props().long(c"sock", i64::from(conn.fd()));
    // `SW_SOCK_SSL`.
    if kind & (1 << 9) != 0 {
        bailed(conn.call(Op::EnableSsl).run())?;
    }
    Some(Rc::new(conn))
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
    let data = net::this(call);
    if data.has_conn() {
        warn(
            ErrorKind::Fatal,
            "connection to the server has already been established",
        );
        return call.set_bool(false);
    }
    if !data.listening.get() {
        warn(ErrorKind::Fatal, "async client is not initialized");
        return call.set_bool(false);
    }
    let Some(fresh) = connection(object, port) else {
        return call.set_bool(false);
    };
    if let Some(zset) = net::settings(object)
        && crate::settings::apply(fresh.cli, zset).is_none()
    {
        return call.set_bool(false);
    }
    let data = net::this(call);
    let set = |slot: usize| callback(data, slot).is_some();
    if !set(RECEIVE) {
        report(
            ErrorKind::Fatal,
            E_ERROR,
            "no 'onReceive' callback function",
        );
        return call.set_bool(false);
    }
    let bit = |event: c_int| 1u32 << event;
    let mut events = bit(ON_RECEIVE);
    if fresh.is_stream() {
        for (slot, name) in [
            (CONNECT, "onConnect"),
            (ERROR, "onError"),
            (CLOSE, "onClose"),
        ] {
            if !set(slot) {
                report(
                    ErrorKind::Fatal,
                    E_ERROR,
                    &format!("no '{name}' callback function"),
                );
                return call.set_bool(false);
            }
        }
        events |= bit(ON_CONNECT) | bit(ON_CLOSE) | bit(ON_ERROR);
        if set(BUFFER_FULL) {
            events |= bit(ON_BUFFER_FULL);
        }
        if set(BUFFER_EMPTY) {
            events |= bit(ON_BUFFER_EMPTY);
        }
    } else {
        for (slot, event) in [(CONNECT, ON_CONNECT), (CLOSE, ON_CLOSE), (ERROR, ON_ERROR)] {
            if set(slot) {
                events |= bit(event);
            }
        }
    }
    state::swr_v1_clients_listen(fresh.cli.get(), object.as_ptr().cast(), events, async_event);
    net::this(call).set_conn(Some(fresh.clone()));
    // SAFETY: `zval_addref_p(ZEND_THIS)`: the client holds the object.
    unsafe { (*object.as_ptr()).gc.refcount += 1 };

    let mut connect = fresh.call(Op::Connect);
    connect.data = host.as_ptr().cast();
    connect.a = port;
    connect.timeout = timeout;
    connect.b = flags;
    let Some(result) = bailed(connect.run()) else {
        return call.set_bool(false);
    };
    // Only the object holds the connection now, so a release below destroys it.
    drop(fresh);
    if result >= 0 {
        return call.set_bool(true);
    }
    let failure = errno();
    let host = String::from_utf8_lossy(host.split(|&b| b == 0).next().unwrap_or_default());
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream prints (int) port"
    )]
    let port = port as i32;
    if failure == 0 {
        let error = last_error();
        if error == DNS_RESOLVE_FAILED {
            let message = format!(
                "connect to server[{host}:{port}] failed. Error: {}[{error}]",
                strerror(error)
            );
            warn(ErrorKind::Error, &message);
        }
        object.props().long(c"errCode", i64::from(error));
    } else {
        warn_sys(&format!("connect to server[{host}:{port}] failed"), failure);
        object.props().long(c"errCode", i64::from(failure));
    }
    // The object's connection, if an error callback did not free it.
    if let Some(remaining) = net::this(call).conn()
        && events & bit(ON_ERROR) == 0
        && bailed(net::release(object, remaining)).is_some()
    {
        // SAFETY: the reference taken above.
        unsafe { (*object.as_ptr()).gc.refcount -= 1 };
    }
    call.set_bool(false);
}

/// `close(bool $force = false): bool`: the close callback frees the client.
fn close(call: &Call) {
    let Some(conn) = net::conn(call) else {
        warn(ErrorKind::Fatal, "client is not connected to the server");
        return call.set_bool(false);
    };
    call.set_bool(bailed(conn.call(Op::Close).run()).is_some_and(|result| result >= 0));
}

/// `on(string $host, callable $callback): bool`.
fn on(call: &Call) {
    let Ok(name) = call.arity(2, 2).and_then(|()| call.string(1, b"")) else {
        return call.set_bool(false);
    };
    let object = Obj::this(call);
    let Some(callable) = call
        .value(2)
        .and_then(|value| bailed(native::owned_callable(value)))
    else {
        return call.set_bool(false);
    };
    let Some(callable) = callable else {
        return call.set_null();
    };
    net::this(call).listening.set(true);
    let name = name.split(|&b| b == 0).next().unwrap_or_default();
    let found = event_slot(name).and_then(|slot| Some((slot, *NAMES.get(slot)?)));
    let Some((slot, property)) = found else {
        let message = format!(
            "Unknown event callback type name '{}'",
            String::from_utf8_lossy(name)
        );
        warn(ErrorKind::Fatal, &message);
        drop(callable);
        return call.set_bool(false);
    };
    set_property(call, 2, property);
    replace(object, slot, callable);
    call.set_bool(true);
}

/// `zend_update_property(swoole_client_async_ce, ZEND_THIS, name, zcallback)`:
/// the property takes a copy of argument `n`, the callback.
fn set_property(call: &Call, n: u32, name: &CStr) {
    let callback = call.value(n);
    if let (Some(props), Some(callback)) = (call.props(Some(&ASYNC)), callback) {
        props.value(name, callback);
    }
}

/// The first event `name` names: upstream's `strncasecmp(event, name, len)`
/// chain, so a case-insensitive prefix (even an empty one) matches.
fn event_slot(name: &[u8]) -> Option<usize> {
    EVENTS.iter().position(|event| {
        name.len() <= event.len()
            && name
                .iter()
                .zip(event.iter())
                .all(|(a, b)| a.to_ascii_lowercase() == *b)
    })
}

/// Put `callable` in `slot`, freeing what was there.
fn replace(object: Obj, slot: usize, callable: Rc<Callable>) {
    if let Some(entry) = object.data().callbacks.get(slot) {
        drop(entry.replace(Some(callable)));
    }
}

/// `sleep()`/`pause()` and `wakeup()`/`resume()`.
fn pause(call: &Call, op: Op) {
    let Some(conn) = net::connected(call) else {
        return call.set_bool(false);
    };
    call.set_bool(bailed(conn.call(op).run()).is_some_and(|result| result >= 0));
}

fn sleep(call: &Call) {
    pause(call, Op::Sleep);
}

fn wakeup(call: &Call) {
    pause(call, Op::Wakeup);
}

/// `enableSSL(?callable $onSslReady = null): bool`.
fn enable_ssl(call: &Call) {
    if call.arity(0, 1).is_err() {
        return call.set_bool(false);
    }
    if call.zval(1).is_null() {
        call.throw(
            swoole_rs_phpswoole::exception(),
            c"require `onSslReady` callback",
            i64::from(INVALID_PARAMS),
        );
        return call.set_bool(false);
    }
    let Some(conn) = net::connected(call) else {
        return call.set_bool(false);
    };
    if !net::enable_ssl(call, &conn) {
        return call.set_bool(false);
    }
    if bailed(conn.call(Op::SslWait).run()).is_none_or(|result| result < 0) {
        return call.set_bool(false);
    }
    let object = Obj::this(call);
    drop(net::this(call).callbacks[SSL_READY].take());
    if bailout::pending() {
        return call.set_bool(false);
    }
    let created = call
        .value(1)
        .and_then(|value| bailed(native::owned_callable(value)));
    let Some(Some(callable)) = created else {
        return call.set_bool(false);
    };
    set_property(call, 1, c"onSSLReady");
    replace(object, SSL_READY, callable);
    conn.ssl_wait.set(true);
    if let Some(socket) = conn.cli.net() {
        socket.edit(|record| record.tls.state = SSL_STATE_WAIT_STREAM);
    }
    call.set_bool(true);
}

swoole_rs_zend::handlers! {
    zim_swoole_client_async___construct => construct;
    zim_swoole_client_async___destruct => destruct;
    zim_swoole_client_async_connect => connect;
    zim_swoole_client_async_sleep => sleep;
    zim_swoole_client_async_wakeup => wakeup;
    zim_swoole_client_async_enableSSL => enable_ssl;
    zim_swoole_client_async_isConnected => net::is_connected;
    zim_swoole_client_async_close => close;
    zim_swoole_client_async_on => on;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(name: &[u8]) -> Option<usize> {
        event_slot(name)
    }

    #[test]
    fn event_names_match_as_strncasecmp_prefixes() {
        assert_eq!(slot(b"Connect"), Some(CONNECT), "case-insensitive");
        assert_eq!(slot(b"bufferFull"), Some(BUFFER_FULL), "camel case");
        assert_eq!(
            slot(b"buffer"),
            Some(BUFFER_FULL),
            "a prefix matches the first event"
        );
        assert_eq!(slot(b""), Some(CONNECT), "empty matches the first");
        assert_eq!(slot(b"connected"), None, "longer than the name");
        assert_eq!(slot(b"open"), None, "unknown");
    }
}
