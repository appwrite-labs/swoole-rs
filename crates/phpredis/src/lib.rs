//! PHP `Swoole\Redis\Server` in Rust: registration, constants,
//! `setHandler`/`getHandler`/`format` and the request dispatch over the
//! protocols crate's RESP walk and reply framing. C++ keeps the Server and
//! answers through `swr_redis_host_*` (`ext-src/swoole_redis_server.cc`); a
//! handler runs there inside `zend_try`, and its trampoline re-raises a
//! bailout once the Rust frames are gone (docs/zend.md).
//!
//! Without the `php` feature the crate is empty.
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
    ffi::{CString, c_char, c_int, c_void},
    ptr,
    sync::atomic::{AtomicPtr, Ordering::Relaxed},
};
use swoole_rs_phpstubs::redis_server as stubs;
use swoole_rs_protocols::{RespPhpState, RespSpan, swr_v1_resp_handler_key, swr_v1_resp_php_next};
use swoole_rs_zend::{
    Call, Class, E_ERROR, E_WARNING, bailout, handlers, method_table,
    string::{self, bytes},
    sys::{
        ZEND_ACC_NOT_SERIALIZABLE, ZEND_ACC_PUBLIC, ZEND_ACC_STATIC, add_next_index_long,
        add_next_index_null, add_next_index_stringl, swr_zval_copy, zend_class_entry, zend_long,
        zend_object, zend_read_property, zval, zval_ptr_dtor,
    },
    value,
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
/// `swr_redis_host_call`'s results.
const CALL_BAILED: c_int = -1;
const CALL_FAILED: c_int = 0;

/// `swoole::Server` and `swoole::RecvData`, opaque.
type Server = c_void;
type RecvData = c_void;

unsafe extern "C" {
    fn swr_redis_host_recv_data(server: *mut Server, request: *mut RecvData, data: *mut zval);
    fn swr_redis_host_fd(request: *const RecvData) -> zend_long;
    fn swr_redis_host_send(
        server: *mut Server,
        fd: zend_long,
        data: *const c_char,
        length: usize,
    ) -> bool;
    fn swr_redis_host_close(server: *mut Server, fd: zend_long);
    fn swr_redis_host_call(
        server: *mut Server,
        callable: *mut c_void,
        args: *mut zval,
        retval: *mut zval,
    ) -> c_int;
    fn swr_redis_host_callable(callback: *mut zval, callable: *mut *mut c_void) -> bool;
    fn swr_redis_host_callable_free(callable: *mut c_void);
    fn swr_redis_host_update_property(
        scope: *mut zend_class_entry,
        object: *mut zend_object,
        name: *const c_char,
        length: usize,
        value: *mut zval,
    ) -> bool;
    fn swr_redis_host_erange();
    fn swr_redis_host_error(level: c_int, message: *const c_char);
    fn swr_redis_host_fatal_error(level: c_int, message: *const c_char);
}

thread_local! {
    /// `redis_handlers`: each command key's `zend::Callable` (C++'s).
    static HANDLERS: RefCell<HashMap<Vec<u8>, usize>> = RefCell::new(HashMap::new());
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

/// MINIT: `Swoole\Redis\Server` extending `Swoole\Server`, and its reply type
/// constants. Its objects are the parent's (it installs no handlers).
/// # Safety
/// Called once, at MINIT, after `Swoole\Server` is registered.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpredis_minit(
    server: *mut zend_class_entry,
    exception: *mut zend_class_entry,
) -> *mut zend_class_entry {
    EXCEPTION.store(exception, Relaxed);
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
    SERVER.entry()
}

/// RSHUTDOWN: free the handlers. True if a destructor bailed out; the C++
/// caller re-raises it.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_phpredis_rshutdown() -> bool {
    let handlers: Vec<usize> =
        HANDLERS.with_borrow_mut(|map| map.drain().map(|(_, h)| h).collect());
    let mut bailed = false;
    for handler in handlers {
        // SAFETY: a callable `swr_redis_host_callable` made, freed once; the
        // body is C++.
        bailed |=
            unsafe { bailout::trap(swr_redis_host_callable_free, handler as *mut c_void) }.is_err();
    }
    bailed
}

/// `redis_handler_key`: `_handler_` and the command, lowercased.
fn handler_key(command: &[u8]) -> Vec<u8> {
    let mut key = [0u8; MAX_COMMAND_SIZE];
    // SAFETY: `command` is readable; `key` has the 64 bytes the export writes.
    let length =
        unsafe { swr_v1_resp_handler_key(command.as_ptr(), command.len(), key.as_mut_ptr()) };
    key[..length].to_vec()
}

/// `%.*s`: up to the first NUL.
fn until_nul(text: &[u8]) -> &[u8] {
    text.iter()
        .position(|&b| b == 0)
        .map_or(text, |nul| &text[..nul])
}

/// `php_swoole_error(level, "%s", message)` (or the fatal variant): a user
/// error handler runs, and may bail out.
fn report(fatal: bool, level: c_int, message: &[u8]) -> Result<(), bailout::Bailout> {
    let message = CString::new(until_nul(message)).unwrap_or_default();
    let reporter = if fatal {
        swr_redis_host_fatal_error
    } else {
        swr_redis_host_error
    };
    // SAFETY: C++'s reporter, printing the message through "%s".
    unsafe { bailout::report_level(reporter, level, &message) }
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
    let mut callable = ptr::null_mut();
    // SAFETY: the passed callback and a local output; C++ traps a bailout.
    if !unsafe { swr_redis_host_callable(callback, &raw mut callable) } {
        bailout::Bailout.defer();
        return;
    }
    if callable.is_null() {
        return;
    }
    let key = handler_key(command);
    // SAFETY: `$this` of the class, the key's bytes and the passed callback,
    // which the property copies; C++ traps a bailout (the dynamic property's
    // deprecation runs the user error handler).
    if !unsafe {
        swr_redis_host_update_property(
            SERVER.entry(),
            call.this(),
            key.as_ptr().cast(),
            key.len(),
            callback,
        )
    } {
        // SAFETY: the callable just made, never stored.
        unsafe { swr_redis_host_callable_free(callable) };
        bailout::Bailout.defer();
        return;
    }
    let old = HANDLERS.with_borrow_mut(|map| map.insert(key, callable as usize));
    if let Some(old) = old {
        // SAFETY: the callable this one replaces, freed once; the body is C++.
        bailout::defer(unsafe { bailout::trap(swr_redis_host_callable_free, old as *mut c_void) });
    }
    call.set_bool(true);
}

fn get_handler(call: &Call) {
    let Ok(command) = call.arity(1, 1).and_then(|()| call.string(1, b"")) else {
        call.set_bool(false);
        return;
    };
    let key = handler_key(command);
    // SAFETY: a zval is plain data.
    let mut rv = unsafe { std::mem::zeroed::<zval>() };
    // SAFETY: `$this` of the class, the key's bytes, silently, into `rv`.
    let handler = unsafe {
        zend_read_property(
            SERVER.entry(),
            call.this(),
            key.as_ptr().cast(),
            key.len(),
            true,
            &raw mut rv,
        )
    };
    // SAFETY: the property's live value; `RETURN_ZVAL(handler, 1, 0)`.
    unsafe { swr_zval_copy(call.return_value(), handler) };
}

/// `php_swoole_redis_server_onReceive` on a Redis port: parse the request,
/// then run its command's handler and send back a string it returns. `SW_OK`
/// or `SW_ERR`, or -2 if PHP bailed out (the C++ caller re-raises it).
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
    // SAFETY: the live request.
    let fd = unsafe { swr_redis_host_fd(request) };
    // SAFETY: a zval is plain data.
    let mut zdata = unsafe { std::mem::zeroed::<zval>() };
    // SAFETY: a live server and request; `zdata` takes the payload string.
    unsafe { swr_redis_host_recv_data(server, request, &raw mut zdata) };
    // SAFETY: `php_swoole_get_recv_data` leaves a string.
    let payload = unsafe { value::str_of(&raw const zdata) };
    // SAFETY: the payload, live until `release`.
    let data = unsafe { bytes(payload) };
    // SAFETY: a zval is plain data.
    let mut params = unsafe { std::mem::zeroed::<zval>() };
    // SAFETY: an empty slot.
    unsafe { value::array_init(&raw mut params) };
    let release = |zdata: &mut zval, params: &mut zval| {
        // SAFETY: the payload and parameters, released once.
        unsafe { zval_ptr_dtor(zdata) };
        // SAFETY: as above.
        unsafe { zval_ptr_dtor(params) };
    };
    let mut state = RespPhpState::default();
    let mut span = RespSpan::default();
    let mut range = 0u8;
    let mut command: &[u8] = &[];
    let result = loop {
        // SAFETY: the payload's bytes and local state, span and range.
        let next = unsafe {
            swr_v1_resp_php_next(
                data.as_ptr(),
                data.len(),
                &raw mut state,
                &raw mut span,
                &raw mut range,
            )
        };
        if range != 0 {
            // SAFETY: sets the thread's errno.
            unsafe { swr_redis_host_erange() };
        }
        if next <= 0 {
            break next;
        }
        let text = &data[span.offset..span.offset + span.length];
        match span.kind {
            // SAFETY: the live parameter array; Zend copies the bytes.
            0 => unsafe {
                add_next_index_stringl(&raw mut params, text.as_ptr().cast(), text.len());
            },
            // SAFETY: as above.
            1 => unsafe {
                add_next_index_long(&raw mut params, zend_long::from(span.integer));
            },
            // SAFETY: as above.
            2 => unsafe {
                add_next_index_null(&raw mut params);
            },
            _ => command = text,
        }
    };
    if result < 0 {
        release(&mut zdata, &mut params);
        // SAFETY: a live server.
        unsafe { swr_redis_host_close(server, fd) };
        return Ok(OK);
    }
    if command.len() >= MAX_COMMAND_SIZE {
        let head = &command[..8];
        let mut message = b"command [".to_vec();
        message.extend_from_slice(until_nul(head));
        message.extend_from_slice(format!("...](length={}) is too long", command.len()).as_bytes());
        release(&mut zdata, &mut params);
        report(false, E_WARNING, &message)?;
        // SAFETY: a live server.
        unsafe { swr_redis_host_close(server, fd) };
        return Ok(OK);
    }
    let key = handler_key(command);
    let Some(callable) = HANDLERS.with_borrow(|map| map.get(&key).copied()) else {
        let mut reply = b"-ERR unknown command '".to_vec();
        reply.extend_from_slice(until_nul(command));
        reply.extend_from_slice(b"'\r\n");
        release(&mut zdata, &mut params);
        // SAFETY: a live server and the reply's bytes.
        let sent = unsafe { swr_redis_host_send(server, fd, reply.as_ptr().cast(), reply.len()) };
        return Ok(if sent { OK } else { ERR });
    };
    let mut args = [swoole_rs_zend::long_zval(fd), params];
    // SAFETY: a zval is plain data (IS_UNDEF).
    let mut retval = unsafe { std::mem::zeroed::<zval>() };
    // SAFETY: a live server, the handler's callable, two arguments and a
    // return slot; C++ traps a bailout.
    let called = unsafe {
        swr_redis_host_call(
            server,
            callable as *mut c_void,
            args.as_mut_ptr(),
            &raw mut retval,
        )
    };
    if called == CALL_BAILED {
        return Err(bailout::Bailout);
    }
    if called == CALL_FAILED {
        let mut message = b"Swoole\\Redis\\Server->onRequest with command '".to_vec();
        message.extend_from_slice(until_nul(command));
        message.extend_from_slice(b"' handler error");
        report(false, E_WARNING, &message)?;
    }
    // SAFETY: the handler's return value.
    if let Some(reply) = unsafe { swoole_rs_zend::string_of(&raw const retval) } {
        // SAFETY: a live server and the reply's bytes.
        unsafe { swr_redis_host_send(server, fd, reply.as_ptr().cast(), reply.len()) };
    }
    // SAFETY: the return value, released once.
    unsafe { zval_ptr_dtor(&raw mut retval) };
    let [_, mut params] = args;
    release(&mut zdata, &mut params);
    Ok(OK)
}

/// `string::copy`, re-exported for `format`.
pub(crate) fn zend_string(bytes: &[u8]) -> *mut swoole_rs_zend::sys::zend_string {
    string::copy(bytes)
}

/// The exception class `format` throws.
pub(crate) fn exception() -> *mut zend_class_entry {
    EXCEPTION.load(Relaxed)
}
