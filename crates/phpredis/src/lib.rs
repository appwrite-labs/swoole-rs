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
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

mod format;

use std::{
    cell::RefCell,
    collections::HashMap,
    ffi::{CString, c_int, c_void},
    ptr,
    sync::atomic::{AtomicPtr, Ordering::Relaxed},
};
use swoole_rs_corebase::log::set_last_error;
use swoole_rs_phpcoroutines::create_coroutine;
use swoole_rs_phphelpers::{callable, function_call};
use swoole_rs_phpstubs::redis_server as stubs;
use swoole_rs_protocols::{RespPhpState, resp_handler_key, resp_php_next};
use swoole_rs_server::{server_methods, view};
use swoole_rs_zend::{
    Call, Class, E_ERROR, E_WARNING, Owned, OwnedArray, bailout, handlers, method_table, php_error,
    sys::{
        ZEND_ACC_NOT_SERIALIZABLE, ZEND_ACC_PUBLIC, ZEND_ACC_STATIC, swr_zend_callable_warning,
        swr_zend_try_facade_update, swr_zval_copy, zend_class_entry, zend_long, zend_read_property,
    },
};

static SERVER: Class = Class::new();
/// `swoole_exception_ce`, for `format`'s exceptions.
static EXCEPTION: AtomicPtr<zend_class_entry> = AtomicPtr::new(ptr::null_mut());

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

/// `SW_OK`/`SW_ERR`, and `swr_v1_phpredis_receive`'s bailout.
const OK: c_int = 0;
const ERR: c_int = -1;
const BAILED: c_int = -2;
/// `SW_ERROR_PHP_FATAL_ERROR` and `SW_ERROR_PHP_RUNTIME_NOTICE`.
const PHP_FATAL_ERROR: c_int = 503;
const PHP_RUNTIME_NOTICE: c_int = 510;

/// `swoole::Server` and `swoole::RecvData`, opaque.
type Server = c_void;
type RecvData = c_void;

thread_local! {
    /// `redis_handlers`: each command key's callable.
    static HANDLERS: RefCell<HashMap<Vec<u8>, *mut callable::Callable>> =
        RefCell::new(HashMap::new());
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

/// `Swoole\Redis\Server`, as C++ reads it; MINIT writes it.
#[unsafe(no_mangle)]
pub static mut swoole_redis_server_ce: *mut zend_class_entry = ptr::null_mut();

unsafe extern "C" {
    /// `Swoole\Exception` (crates/phpswoole).
    static swoole_exception_ce: *mut zend_class_entry;
}

/// MINIT: `Swoole\Redis\Server` extending `Swoole\Server`, and its reply type
/// constants. Its objects are the parent's (it installs no handlers).
/// # Safety
/// Called once, at MINIT, after `Swoole\Server` is registered.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn php_swoole_redis_server_minit(_module_number: c_int) {
    let server = swoole_rs_phpserver::server_class_entry();
    // SAFETY: as above.
    EXCEPTION.store(unsafe { swoole_exception_ce }, Relaxed);
    // SAFETY: MINIT, with a static table and the registered parent.
    unsafe {
        SERVER.register_child(
            c"Swoole\\Redis\\Server",
            METHODS.as_ptr(),
            server,
            |entry, _| {
                entry.ce_flags |= ZEND_ACC_NOT_SERIALIZABLE;
            },
        );
    }
    for (name, value) in REPLY_TYPES {
        // SAFETY: MINIT, after `register`.
        unsafe { SERVER.constant_long(name, value) };
    }
    // SAFETY: MINIT, before anything reads it.
    unsafe { swoole_redis_server_ce = SERVER.entry() };
}

/// RSHUTDOWN: free the handlers, then re-raise a destructor's bailout.
#[unsafe(no_mangle)]
pub extern "C" fn php_swoole_redis_server_rshutdown() {
    let handlers: Vec<_> = HANDLERS.with_borrow_mut(|map| map.drain().map(|(_, h)| h).collect());
    for handler in handlers {
        // SAFETY: a callable `set_handler` made, freed once; a destructor's
        // bailout stays pending.
        let _freed = unsafe { callable::release(handler) };
    }
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
    // SAFETY: from a handler or a callback Zend runs.
    unsafe { php_error(level, &message) }
}

fn set_handler(call: &Call) {
    let Ok((command, callback)) = call
        .arity(2, 2)
        .and_then(|()| Ok((call.string(1, b"")?, call.zval(2))))
    else {
        call.set_bool(false);
        return;
    };
    if command.is_empty() || command.len() >= MAX_COMMAND_SIZE {
        bailout::defer(report(true, E_ERROR, b"invalid command"));
        call.set_bool(false);
        return;
    }
    // SAFETY: the passed callback; `sw_callable_create`, whose bailout stays
    // pending for the handler's last step.
    let created = unsafe {
        callable::factory(
            callback,
            ptr::null(),
            true,
            ptr::null_mut(),
            swr_zend_callable_warning,
        )
    };
    let Ok(callable) = created else { return };
    if callable.is_null() {
        return;
    }
    let key = handler_key(command);
    // SAFETY: `$this` of the class, the key's bytes and the passed callback,
    // which the property copies; the shim traps a bailout (the dynamic
    // property's deprecation runs the user error handler).
    let bailed = unsafe {
        swr_zend_try_facade_update(
            SERVER.entry(),
            call.this(),
            key.as_ptr().cast(),
            key.len(),
            callback,
        )
    };
    if bailed != 0 {
        // SAFETY: the callable just made, never stored.
        let _freed = unsafe { callable::release(callable) };
        bailout::Bailout.defer();
        return;
    }
    let old = HANDLERS.with_borrow_mut(|map| map.insert(key, callable));
    if let Some(old) = old {
        // SAFETY: the callable this one replaces, freed once; a destructor's
        // bailout stays pending.
        let _freed = unsafe { callable::release(old) };
    }
    call.set_bool(true);
}

fn get_handler(call: &Call) {
    let Ok(command) = call.arity(1, 1).and_then(|()| call.string(1, b"")) else {
        call.set_bool(false);
        return;
    };
    let key = handler_key(command);
    let rv = Owned::default();
    // SAFETY: `$this` of the class, the key's bytes, silently, into `rv`.
    let handler = unsafe {
        zend_read_property(
            SERVER.entry(),
            call.this(),
            key.as_ptr().cast(),
            key.len(),
            true,
            rv.as_ptr(),
        )
    };
    // SAFETY: the property's live value; `RETURN_ZVAL(handler, 1, 0)`.
    unsafe { swr_zval_copy(call.return_value(), handler) };
}

/// `php_swoole_redis_server_onReceive` on a Redis port: parse the request,
/// then run its command's handler and send back a string it returns. `SW_OK`
/// or `SW_ERR`, or -2 if PHP bailed out (the caller re-raises it).
/// # Safety
/// A live server and its received request.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpredis_receive(
    server: *mut Server,
    request: *mut RecvData,
) -> c_int {
    receive(server, request).unwrap_or(BAILED)
}

fn receive(server: *mut Server, request: *mut RecvData) -> Result<c_int, bailout::Bailout> {
    // SAFETY: the live server the receive callback passes.
    let serv = unsafe { view::Server::from_raw(server) };
    // SAFETY: the live request, which starts with its `DataHead` and its `fd`.
    let fd = unsafe { *request.cast::<zend_long>() };
    let zdata = Owned::default();
    // SAFETY: a live server and request; `zdata` takes the payload string.
    unsafe { swoole_rs_phpserver::php_swoole_get_recv_data(server, zdata.as_ptr(), request) };
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
    let Some(callable) = HANDLERS.with_borrow(|map| map.get(&key).copied()) else {
        let mut reply = b"-ERR unknown command '".to_vec();
        reply.extend_from_slice(until_nul(command));
        reply.extend_from_slice(b"'\r\n");
        let sent = server_methods::send(serv, fd, &reply);
        return Ok(if sent { OK } else { ERR });
    };
    let args = [Owned::long(fd), params.into_owned()];
    let retval = Owned::default();
    // SAFETY: a live server.
    let coroutine = unsafe { swoole_rs_phpserver::is_enable_coroutine(server) };
    // SAFETY: a handler `set_handler` stored.
    let cache = unsafe { callable::cache(callable) };
    // SAFETY: its resolved cache, two arguments and a return slot; the
    // coroutine leaf and the reporter run inside C traps.
    let called = unsafe {
        function_call(
            cache,
            2,
            args.as_ptr().cast_mut().cast(),
            retval.as_ptr(),
            coroutine,
            create_coroutine,
            swr_zend_callable_warning,
        )
    }?;
    if !called {
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

/// `errno = value`.
fn set_errno(value: c_int) {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    // SAFETY: this thread's errno slot, live for the thread.
    let slot = unsafe { libc::__errno_location() };
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    // SAFETY: as above.
    let slot = unsafe { libc::__error() };
    // SAFETY: `slot` is this thread's errno.
    unsafe { slot.write(value) };
}

/// The exception class `format` throws.
pub(crate) fn exception() -> *mut zend_class_entry {
    EXCEPTION.load(Relaxed)
}
