//! `Swoole\Server`'s `on`, `getCallback`, `listen`, the send family,
//! `sendMessage` and `finish` (also `Swoole\Server\Task::finish`). C++ keeps
//! the Server, its callbacks and ports and the task encoding
//! (`native/process/swoole_rs_phpserver_io.h`).
use crate::server::{fatal, no_payload, running, server};
use std::{
    ffi::{CStr, c_char, c_int, c_void},
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
};
use swoole_rs_zend::{
    Call, E_ERROR, E_WARNING, message,
    string::bytes,
    sys::{
        IS_NULL, IS_STRING, IS_TRUE, swr_zend_string_release, swr_zval_copy, swr_zval_get_string,
        zend_object, zend_read_property, zend_string, zend_update_property, zval,
    },
    to_long, type_of,
};

/// `swr_v1_phpserver_io_host`.
#[repr(C)]
pub struct IoHost {
    closed: unsafe extern "C" fn(*mut c_void) -> bool,
    set_callback: unsafe extern "C" fn(*mut zend_object, c_int, *mut zval) -> bool,
    port_call:
        unsafe extern "C" fn(*mut zend_object, c_int, *mut zval, *mut zval, *mut zval) -> bool,
    listen: unsafe extern "C" fn(*mut zend_object, *const c_char, i64, i64, *mut zval) -> bool,
    send_unix: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char, usize, i64) -> c_int,
    send: unsafe extern "C" fn(*mut c_void, i64, *mut zend_string) -> bool,
    sendto: unsafe extern "C" fn(
        *mut c_void,
        c_int,
        *const c_char,
        i64,
        *const c_char,
        usize,
        i64,
    ) -> c_int,
    is_master: unsafe extern "C" fn(*mut c_void) -> bool,
    sendfile: unsafe extern "C" fn(*mut c_void, i64, *const c_char, usize, i64, i64) -> bool,
    has_pipe_message: unsafe extern "C" fn(*mut c_void) -> bool,
    is_worker: unsafe extern "C" fn(*mut c_void) -> bool,
    worker_id: unsafe extern "C" fn() -> i64,
    core_worker_num: unsafe extern "C" fn(*mut c_void) -> i64,
    send_message: unsafe extern "C" fn(*mut c_void, *mut zval, i64) -> c_int,
    task_enable_coroutine: unsafe extern "C" fn(*mut c_void) -> bool,
    finish: unsafe extern "C" fn(*mut c_void, *mut zval, *mut zend_object) -> bool,
    task_server: unsafe extern "C" fn(*mut zend_object) -> *mut c_void,
    started: unsafe extern "C" fn(*mut c_void) -> bool,
}

static HOST: AtomicPtr<IoHost> = AtomicPtr::new(ptr::null_mut());

fn host() -> &'static IoHost {
    // SAFETY: MINIT installs C++'s static table before any method runs.
    unsafe { &*HOST.load(Ordering::Acquire) }
}

/// MINIT: the table the methods call.
/// # Safety
/// `table` lives for the process.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpserver_io_init(table: *const IoHost) {
    HOST.store(table.cast_mut(), Ordering::Release);
}

/// `server_event_map`: lower-case event name, callback type and property.
const EVENTS: [(&str, c_int, &CStr); 14] = [
    ("start", 0, c"onStart"),
    ("beforeshutdown", 1, c"onBeforeShutdown"),
    ("shutdown", 2, c"onShutdown"),
    ("workerstart", 3, c"onWorkerStart"),
    ("workerstop", 4, c"onWorkerStop"),
    ("beforereload", 5, c"onBeforeReload"),
    ("afterreload", 6, c"onAfterReload"),
    ("task", 7, c"onTask"),
    ("finish", 8, c"onFinish"),
    ("workerexit", 9, c"onWorkerExit"),
    ("workererror", 10, c"onWorkerError"),
    ("managerstart", 11, c"onManagerStart"),
    ("managerstop", 12, c"onManagerStop"),
    ("pipemessage", 13, c"onPipeMessage"),
];

/// The event `name` (`zend_string_tolower`, then an exact match) names.
fn event(name: &[u8]) -> Option<(c_int, &'static CStr)> {
    EVENTS
        .iter()
        .find(|(key, ..)| name.eq_ignore_ascii_case(key.as_bytes()))
        .map(|&(_, kind, property)| (kind, property))
}

/// `zend::String(name)`, looked up as an event.
fn lookup(name: *mut zval) -> Option<(c_int, &'static CStr)> {
    // SAFETY: a live argument; an owned reference, released below.
    let string = unsafe { swr_zval_get_string(name) };
    // SAFETY: the reference is live until released.
    let found = event(unsafe { bytes(string) });
    // SAFETY: releasing the reference taken above.
    unsafe { swr_zend_string_release(string) };
    found
}

/// Re-raise a bailout C++ trapped, from the handler, which owns nothing.
fn raise(bailed: bool) {
    if bailed {
        // SAFETY: Zend's own `zend_bailout()`; see docs/zend.md.
        unsafe {
            swoole_rs_zend::sys::_zend_bailout(
                c"crates/phpserver/src/io.rs".as_ptr().cast_mut(),
                line!(),
            )
        };
    }
}

fn on(call: &Call) {
    let serv = server(call);
    // SAFETY: a live server.
    if unsafe { (host().closed)(serv) } {
        fatal(
            E_WARNING,
            c"server is running, unable to register event callback function",
        );
        return call.set_bool(false);
    }
    if call.arity(2, 2).is_err() {
        return call.set_bool(false);
    }
    let (name, cb) = (call.zval(1), call.zval(2));
    let Some((kind, property)) = lookup(name) else {
        // SAFETY: a zval is plain data; all-zero bytes are `IS_UNDEF`.
        let mut retval: zval = unsafe { std::mem::zeroed() };
        // SAFETY: `$this`, the arguments and a writable result.
        raise(unsafe { (host().port_call)(call.this(), 0, name, cb, &raw mut retval) });
        // RETURN_BOOL(Z_BVAL_P(&retval))
        // SAFETY: C++ wrote the method's result.
        return call.set_bool(unsafe { type_of(&raw const retval) } == IS_TRUE);
    };
    // SAFETY: `$this` and the callback argument; Zend copies both.
    unsafe {
        zend_update_property(
            crate::server::SERVER.entry(),
            call.this(),
            property.as_ptr(),
            property.count_bytes(),
            cb,
        );
    }
    // SAFETY: `$this` and the callback argument.
    call.set_bool(unsafe { (host().set_callback)(call.this(), kind, cb) });
}

fn get_callback(call: &Call) {
    if call.arity(1, 1).is_err() {
        return call.set_bool(false);
    }
    let name = call.zval(1);
    if let Some((_, property)) = lookup(name) {
        let mut rv = std::mem::MaybeUninit::<zval>::uninit();
        // SAFETY: `$this` is live; its own class is the scope (subclasses).
        let scope = unsafe { (*call.this()).ce };
        // SAFETY: `$this`, its class and a scratch zval.
        let value = unsafe {
            zend_read_property(
                scope,
                call.this(),
                property.as_ptr(),
                property.count_bytes(),
                true,
                rv.as_mut_ptr(),
            )
        };
        // SAFETY: Zend returned a live zval.
        if unsafe { type_of(value) } != IS_NULL {
            // SAFETY: `RETURN_ZVAL(property, 1, 0)`.
            return unsafe { swr_zval_copy(call.return_value(), value) };
        }
    }
    // SAFETY: `$this`, the argument and the return slot.
    raise(unsafe {
        (host().port_call)(call.this(), 1, name, ptr::null_mut(), call.return_value())
    });
}

fn listen(call: &Call) {
    let serv = server(call);
    // SAFETY: a live server.
    if unsafe { (host().closed)(serv) } {
        fatal(E_WARNING, c"server is running, cannot add listener");
        return call.set_bool(false);
    }
    let parsed = call
        .arity(3, 3)
        .and_then(|()| Ok((call.string(1, b"")?, call.long(2, 0)?, call.long(3, 0)?)));
    let Ok((address, port, kind)) = parsed else {
        return call.set_bool(false);
    };
    // SAFETY: `$this`, NUL-terminated `Z_PARAM_STRING` bytes and the return slot.
    if !unsafe {
        (host().listen)(
            call.this(),
            address.as_ptr().cast(),
            port,
            kind,
            call.return_value(),
        )
    } {
        call.set_bool(false);
    }
}

fn send(call: &Call) {
    let Some(serv) = running(call) else { return };
    let parsed = call
        .arity(2, 3)
        .and_then(|()| Ok((call.str(2)?, call.long(3, -1)?)));
    let Ok((Some(data), server_socket)) = parsed else {
        return call.set_bool(false);
    };
    let zfd = call.zval(1);
    // SAFETY: a passed argument.
    let kind = unsafe { type_of(zfd) };
    if kind == IS_NULL {
        fatal(E_WARNING, c"fd can not be null");
        return call.set_bool(false);
    }
    // SAFETY: a live string the frame keeps.
    let payload = unsafe { bytes(data) };
    if payload.is_empty() {
        no_payload();
        return call.set_bool(false);
    }
    let host = host();
    if kind == IS_STRING {
        // SAFETY: an IS_STRING zval.
        let path = unsafe { swoole_rs_zend::string_of(zfd) }.unwrap_or_default();
        if path.first() == Some(&b'/') {
            // SAFETY: a live server, a NUL-terminated path and the payload.
            let sent = unsafe {
                (host.send_unix)(
                    serv,
                    path.as_ptr().cast(),
                    payload.as_ptr().cast(),
                    payload.len(),
                    server_socket,
                )
            };
            if sent >= 0 {
                return call.set_bool(sent != 0);
            }
        }
    }
    // SAFETY: a passed argument.
    let fd = unsafe { to_long(zfd) };
    if fd <= 0 {
        fatal(E_WARNING, message!("invalid fd[{fd}]").as_cstr());
        return call.set_bool(false);
    }
    // SAFETY: a live server and string.
    call.set_bool(unsafe { (host.send)(serv, fd, data) });
}

fn sendto(call: &Call) {
    let Some(serv) = running(call) else { return };
    let parsed = call.arity(3, 4).and_then(|()| {
        Ok((
            call.string(1, b"")?,
            call.long(2, 0)?,
            call.string(3, b"")?,
            call.long(4, -1)?,
        ))
    });
    let Ok((address, port, data, server_socket)) = parsed else {
        return call.set_bool(false);
    };
    if data.is_empty() {
        no_payload();
        return call.set_bool(false);
    }
    let (kind, missing) = if address.first() == Some(&b'/') {
        (
            3,
            c"UnixDgram listener has to be added before executing sendto",
        )
    } else if address.contains(&b':') {
        (2, c"UDP6 listener has to be added before executing sendto")
    } else {
        (1, c"UDP listener has to be added before executing sendto")
    };
    // SAFETY: a live server, the NUL-terminated address and the payload.
    let sent = unsafe {
        (host().sendto)(
            serv,
            kind,
            address.as_ptr().cast(),
            port,
            data.as_ptr().cast(),
            data.len(),
            server_socket,
        )
    };
    if sent < 0 {
        fatal(E_WARNING, missing);
        return call.set_bool(false);
    }
    call.set_bool(sent != 0);
}

fn sendfile(call: &Call) {
    let Some(serv) = running(call) else { return };
    let parsed = call.arity(2, 4).and_then(|()| {
        Ok((
            call.long(1, 0)?,
            call.string(2, b"")?,
            call.long(3, 0)?,
            call.long(4, 0)?,
        ))
    });
    let Ok((fd, filename, offset, length)) = parsed else {
        return call.set_bool(false);
    };
    let host = host();
    // SAFETY: a live server.
    if unsafe { (host.is_master)(serv) } {
        // `%s`: the name up to its first NUL.
        let shown = filename.split(|&b| b == 0).next().unwrap_or_default();
        let text = message!(
            "can't sendfile[{}] to the connections in master process",
            String::from_utf8_lossy(shown)
        );
        fatal(E_WARNING, text.as_cstr());
        return call.set_bool(false);
    }
    // SAFETY: a live server and the NUL-terminated name.
    call.set_bool(unsafe {
        (host.sendfile)(
            serv,
            fd,
            filename.as_ptr().cast(),
            filename.len(),
            offset,
            length,
        )
    });
}

fn send_message(call: &Call) {
    let Some(serv) = running(call) else { return };
    let host = host();
    // SAFETY: a live server.
    if !unsafe { (host.has_pipe_message)(serv) } {
        fatal(E_WARNING, c"onPipeMessage is null, can't use sendMessage");
        return call.set_bool(false);
    }
    let Ok(worker_id) = call.arity(2, 2).and_then(|()| call.long(2, -1)) else {
        return call.set_bool(false);
    };
    // SAFETY: a live server; reads the process's worker id.
    if unsafe { (host.is_worker)(serv) } && worker_id == unsafe { (host.worker_id)() } {
        fatal(E_WARNING, c"can't send messages to self");
        return call.set_bool(false);
    }
    // SAFETY: a live server.
    if worker_id < 0 || worker_id >= unsafe { (host.core_worker_num)(serv) } {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "(int) worker_id, as upstream"
        )]
        let shown = worker_id as i32;
        fatal(
            E_WARNING,
            message!("worker_id[{shown}] is invalid").as_cstr(),
        );
        return call.set_bool(false);
    }
    // SAFETY: a live server and the message argument.
    match unsafe { (host.send_message)(serv, call.zval(1), worker_id) } {
        -1 => call.set_bool(false),
        sent => call.set_bool(sent != 0),
    }
}

fn finish(call: &Call) {
    let Some(serv) = running(call) else { return };
    let host = host();
    // SAFETY: a live server.
    if unsafe { (host.task_enable_coroutine)(serv) } {
        // An `E_ERROR`: its bailout waits for the handler's last step.
        fatal(
            E_ERROR,
            c"please use Swoole\\Server\\Task->finish instead when task_enable_coroutine is enable",
        );
        return;
    }
    if call.arity(1, 1).is_err() {
        return call.set_bool(false);
    }
    // SAFETY: a live server and the data argument.
    call.set_bool(unsafe { (host.finish)(serv, call.zval(1), ptr::null_mut()) });
}

fn task_finish(call: &Call) {
    let host = host();
    // SAFETY: `$this` is a Task; an `E_ERROR` when it has no server.
    let serv = unsafe { (host.task_server)(call.this()) };
    // SAFETY: a live server.
    if !unsafe { (host.started)(serv) } {
        fatal(E_WARNING, c"server is not running");
        return call.set_bool(false);
    }
    if call.arity(1, 1).is_err() {
        return call.set_bool(false);
    }
    // SAFETY: a live server, the data and the task.
    call.set_bool(unsafe { (host.finish)(serv, call.zval(1), call.this()) });
}

swoole_rs_zend::handlers! {
    zim_swoole_server_on => on;
    zim_swoole_server_getCallback => get_callback;
    zim_swoole_server_listen => listen;
    zim_swoole_server_send => send;
    zim_swoole_server_sendto => sendto;
    zim_swoole_server_sendfile => sendfile;
    zim_swoole_server_sendMessage => send_message;
    zim_swoole_server_finish => finish;
    zim_swoole_server_task_finish => task_finish;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_match_their_properties_case_insensitively() {
        for (key, kind, property) in EVENTS {
            assert_eq!(event(key.to_uppercase().as_bytes()), Some((kind, property)));
            assert!(property.to_bytes()[2..].eq_ignore_ascii_case(key.as_bytes()));
        }
        assert_eq!(event(b"receive"), None);
        assert_eq!(event(b"start\0"), None);
    }
}
