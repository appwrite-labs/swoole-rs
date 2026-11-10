//! PHP `Swoole\Redis\Server` in Rust: registration, constants,
//! `setHandler`/`getHandler`/`format` and the request dispatch over the
//! protocols crate's RESP walk and reply framing. A handler runs inside a C
//! trap, and crates/phpserver's onReceive slot re-raises its bailout once the
//! Rust frames are gone (docs/zend.md).
//!
//! Without the `php` feature the crate is empty.
#![cfg_attr(
    feature = "php",
    expect(
        clippy::disallowed_types,
        reason = "existing uses; tools/budget.py counts them down (docs/migration.md#data-layout)"
    )
)]
#![cfg(feature = "php")]
#![forbid(unsafe_code)]

mod format;

use std::{
    cell::RefCell,
    collections::HashMap,
    ffi::{CString, c_int, c_long},
    rc::Rc,
};
use swoole_rs_corebase::{log::set_last_error, sys::set_errno};
use swoole_rs_phphelpers::callable::Callable;
use swoole_rs_phpserver::REDIS_SERVER as SERVER;
use swoole_rs_phpstubs::redis_server as stubs;
use swoole_rs_protocols::{RespPhpState, resp_handler_key, resp_php_next};
use swoole_rs_server::{server_methods, view};
use swoole_rs_zend::{
    Call, E_ERROR, E_WARNING, Minit, Owned, OwnedArray, bailout, handlers, method_table,
    sys::{ZEND_ACC_NOT_SERIALIZABLE, ZEND_ACC_PUBLIC, ZEND_ACC_STATIC, zend_long},
};

/// `SW_REDIS_MAX_COMMAND_SIZE`.
const MAX_COMMAND_SIZE: usize = 64;
/// `swoole::redis::ReplyType`, as the class constants name them.
const REPLY_TYPES: [(&std::ffi::CStr, zend_long); 7] = [
    (c"NIL", format::NIL),
    (c"ERROR", format::ERROR),
    (c"STATUS", format::STATUS),
    (c"INT", format::INT),
    (c"STRING", format::STRING),
    (c"SET", format::SET),
    (c"MAP", format::MAP),
];

/// `SW_OK`/`SW_ERR`.
const OK: c_int = 0;
const ERR: c_int = -1;
/// `SW_ERROR_PHP_FATAL_ERROR` and `SW_ERROR_PHP_RUNTIME_NOTICE`.
const PHP_FATAL_ERROR: c_int = 503;
const PHP_RUNTIME_NOTICE: c_int = 510;

thread_local! {
    /// `redis_handlers`: each command key's callable. A handler that replaces
    /// itself while it runs lives until its call returns.
    static HANDLERS: RefCell<HashMap<Vec<u8>, Rc<Callable>>> = RefCell::new(HashMap::new());
}

method_table! {
    static METHODS = [
        c"setHandler" => zim_swoole_redis_server_setHandler, &stubs::arginfo_class_Swoole_Redis_Server_setHandler, ZEND_ACC_PUBLIC;
        c"getHandler" => zim_swoole_redis_server_getHandler, &stubs::arginfo_class_Swoole_Redis_Server_getHandler, ZEND_ACC_PUBLIC;
        c"format" => zim_swoole_redis_server_format, &stubs::arginfo_class_Swoole_Redis_Server_format, ZEND_ACC_PUBLIC | ZEND_ACC_STATIC;
    ];
}

handlers! {
    zim_swoole_redis_server_setHandler => set_handler;
    zim_swoole_redis_server_getHandler => get_handler;
    zim_swoole_redis_server_format => format::method;
}

swoole_rs_zend::minit!(redis_minit => register);

/// MINIT, after `Swoole\Server`: `Swoole\Redis\Server` extending it, with its
/// reply type constants, and the Redis ports' `onReceive`. Its objects are the
/// parent's (it installs no handlers).
#[expect(
    clippy::unnecessary_wraps,
    reason = "minit! raises the bailout a MINIT body returns"
)]
fn register(minit: &Minit) -> Result<(), bailout::Bailout> {
    let parent = minit.parent(swoole_rs_phpserver::server_class_ref());
    minit.register_child(
        &SERVER,
        c"Swoole\\Redis\\Server",
        &METHODS.0,
        parent,
        |entry, _| {
            entry.ce_flags |= ZEND_ACC_NOT_SERIALIZABLE;
        },
    );
    for (name, value) in REPLY_TYPES {
        minit.class_constant_long(&SERVER, name, value);
    }
    swoole_rs_phpserver::set_redis_receive(receive);
    Ok(())
}

/// RSHUTDOWN: free the handlers, then re-raise a destructor's bailout.
pub extern "C" fn redis_rshutdown() {
    let handlers: Vec<_> = HANDLERS.with_borrow_mut(|map| map.drain().map(|(_, h)| h).collect());
    // A destructor's bailout stays pending.
    drop(handlers);
    bailout::finish();
}

/// `redis_handler_key`: `_handler_` and the command, lowercased.
fn handler_key(command: &[u8]) -> Vec<u8> {
    let (key, length) = resp_handler_key(command);
    key.get(..length).unwrap_or(&key).to_vec()
}

/// `%.*s`: up to the first NUL.
fn until_nul(text: &[u8]) -> &[u8] {
    text.split(|&b| b == 0).next().unwrap_or(text)
}

/// `php_swoole_error(level, "%s", message)` (or the fatal variant): a user
/// error handler runs, and may bail out.
fn report(fatal: bool, level: c_int, message: &[u8]) -> Result<(), bailout::Bailout> {
    set_last_error(if fatal {
        PHP_FATAL_ERROR
    } else {
        PHP_RUNTIME_NOTICE
    });
    if !fatal && level != E_ERROR && !swoole_rs_phpswoole::display_errors() {
        return Ok(());
    }
    let message = CString::new(until_nul(message)).unwrap_or_default();
    swoole_rs_zend::report(level, &message)
}

fn set_handler(call: &Call) {
    let Ok((command, Some(callback))) = call
        .arity(2, 2)
        .and_then(|()| Ok((call.string(1, b"")?, call.value(2))))
    else {
        call.set_bool(false);
        return;
    };
    if command.is_empty() || command.len() >= MAX_COMMAND_SIZE {
        bailout::defer(report(true, E_ERROR, b"invalid command"));
        call.set_bool(false);
        return;
    }
    // `sw_callable_create`, whose bailout stays pending for the handler's last step.
    let Ok(Some(callable)) = Callable::new(callback) else {
        return;
    };
    let key = handler_key(command);
    // The dynamic property's deprecation runs the user error handler.
    if let Err(bailed) = call.update_property(Some(&SERVER), &key, callback) {
        drop(callable);
        bailed.defer();
        return;
    }
    // The handler this one replaces goes; a destructor's bailout stays pending.
    let old = HANDLERS.with_borrow_mut(|map| map.insert(key, callable));
    drop(old);
    call.set_bool(true);
}

fn get_handler(call: &Call) {
    let Ok(command) = call.arity(1, 1).and_then(|()| call.string(1, b"")) else {
        call.set_bool(false);
        return;
    };
    call.set_property(Some(&SERVER), &handler_key(command));
}

/// `php_swoole_redis_server_onReceive` on a Redis port: parse the request,
/// then run its command's handler and send back a string it returns. `SW_OK`
/// or `SW_ERR`, or the bailout for crates/phpserver to re-raise.
fn receive(
    serv: view::Server<'_>,
    fd: c_long,
    zdata: &Owned,
    coroutine: bool,
) -> Result<c_int, bailout::Bailout> {
    // `php_swoole_get_recv_data` leaves a string.
    let data = zdata.bytes().unwrap_or_default();
    let mut params = OwnedArray::default();
    let mut state = RespPhpState::default();
    let mut command: &[u8] = &[];
    let result = loop {
        let mut range = false;
        let next = resp_php_next(data, &mut state, &mut range);
        if range {
            set_errno(libc::ERANGE);
        }
        let Ok(Some(span)) = next else {
            break next.map(|_| ());
        };
        let text = data.get(span.offset..span.offset + span.length);
        let text = text.unwrap_or_default();
        match span.kind {
            0 => params.push_bytes(text),
            1 => params.push_long(zend_long::from(span.integer)),
            2 => params.push_null(),
            _ => command = text,
        }
    };
    if result.is_err() {
        server_methods::close(serv, fd, false);
        return Ok(OK);
    }
    if command.len() >= MAX_COMMAND_SIZE {
        let head = command.get(..8).unwrap_or(command);
        let mut message = b"command [".to_vec();
        message.extend_from_slice(until_nul(head));
        message.extend_from_slice(format!("...](length={}) is too long", command.len()).as_bytes());
        report(false, E_WARNING, &message)?;
        server_methods::close(serv, fd, false);
        return Ok(OK);
    }
    let key = handler_key(command);
    let Some(callable) = HANDLERS.with_borrow(|map| map.get(&key).cloned()) else {
        let mut reply = b"-ERR unknown command '".to_vec();
        reply.extend_from_slice(until_nul(command));
        reply.extend_from_slice(b"'\r\n");
        let sent = server_methods::send(serv, fd, &reply);
        return Ok(if sent { OK } else { ERR });
    };
    let args = [Owned::long(fd), params.into_owned()];
    let retval = Owned::default();
    let called = swoole_rs_phpcoroutines::call_handler(&callable, &args, &retval, coroutine);
    // A handler that replaced itself goes now; its destructor's bailout is this call's.
    drop(callable);
    if bailout::pending() {
        return Err(bailout::Bailout);
    }
    if !called? {
        let mut message = b"Swoole\\Redis\\Server->onRequest with command '".to_vec();
        message.extend_from_slice(until_nul(command));
        message.extend_from_slice(b"' handler error");
        report(false, E_WARNING, &message)?;
    }
    if let Some(reply) = retval.bytes() {
        server_methods::send(serv, fd, reply);
    }
    Ok(OK)
}
