//! Server events into PHP callbacks: the arguments (`Swoole\Server\Event` and
//! friends, or plain values), the call and its "handler error" warning. C++
//! keeps the trampolines the Server calls and re-raises a bailout Rust reports
//! (`native/process/swoole_rs_phpserver_dispatch.h`, docs/zend.md).
use crate::server::{DATA, SERVER};
use std::{
    ffi::{CStr, c_char, c_int, c_void},
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
};
use swoole_rs_zend::{
    E_WARNING,
    bailout::Bailout,
    class_name, init_object, message, object_of, php_error,
    sys::{
        _zval_struct__bindgen_ty_1, IS_LONG, add_assoc_double_ex, add_assoc_long_ex,
        add_assoc_string_ex, swr_zend_array_init, zend_class_entry, zend_object,
        zend_update_property, zend_update_property_bool, zend_update_property_double,
        zend_update_property_long, zend_update_property_string, zend_update_property_stringl,
        zend_value, zval, zval_ptr_dtor,
    },
};

/// `SW_ERROR_PHP_RUNTIME_NOTICE`.
const SW_ERROR_PHP_RUNTIME_NOTICE: c_int = 510;

use swoole_rs_server::view;

/// What the dispatch reads of the Server, per event.
struct Facts {
    master_pid: i64,
    manager_pid: i64,
    event_object: bool,
    enable_coroutine: bool,
    coroutine: bool,
    enable_library: bool,
    task_worker: bool,
}

/// `swr_phpserver_packet`.
#[repr(C)]
pub struct Packet {
    data: *const c_char,
    length: usize,
    address: *const c_char,
    port: i64,
    server_port: i64,
}

/// `swr_v1_phpserver_dispatch_host`.
#[repr(C)]
pub struct DispatchHost {
    zserv: unsafe extern "C" fn(*mut c_void) -> *mut zval,
    port_callback: unsafe extern "C" fn(*mut c_void, c_int, c_int) -> *mut c_void,
    call: unsafe extern "C" fn(*mut c_void, u32, *mut zval, bool) -> c_int,
    helper: unsafe extern "C" fn(*const c_char, u32, *mut zval) -> bool,
    recv_data: unsafe extern "C" fn(*mut c_void, *mut zval, *mut c_void),
    close: unsafe extern "C" fn(*mut c_void, i64),
    worker_start: unsafe extern "C" fn(*mut c_void),
    pid: unsafe extern "C" fn() -> i64,
}

static HOST: AtomicPtr<DispatchHost> = AtomicPtr::new(ptr::null_mut());

#[inline]
fn host() -> &'static DispatchHost {
    // SAFETY: MINIT installs C++'s static table before the Server can start.
    unsafe { &*HOST.load(Ordering::Acquire) }
}

/// MINIT: the table the dispatch calls.
/// # Safety
/// `table` lives for the process.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpserver_dispatch_init(table: *const DispatchHost) {
    HOST.store(table.cast_mut(), Ordering::Release);
}

/// `SW_SERVER_CB_*` (server-level callbacks).
const ON_START: c_int = 0;
const ON_BEFORE_SHUTDOWN: c_int = 1;
const ON_SHUTDOWN: c_int = 2;
const ON_WORKER_START: c_int = 3;
const ON_WORKER_STOP: c_int = 4;
const ON_BEFORE_RELOAD: c_int = 5;
const ON_AFTER_RELOAD: c_int = 6;
const ON_WORKER_EXIT: c_int = 9;
const ON_WORKER_ERROR: c_int = 10;
const ON_MANAGER_START: c_int = 11;
const ON_MANAGER_STOP: c_int = 12;
const ON_PIPE_MESSAGE: c_int = 13;
/// Port-level callbacks.
const ON_PACKET: c_int = 3;
const ON_BUFFER_FULL: c_int = 10;
const ON_BUFFER_EMPTY: c_int = 11;

/// `DATA`'s classes.
const EVENT: usize = 0;
const PACKET: usize = 1;
const PIPE_MESSAGE: usize = 2;
const STATUS_INFO: usize = 3;

/// The result of a call through the host.
#[derive(PartialEq, Eq)]
enum Called {
    Failed,
    Done,
    Bailout,
}

/// `ZVAL_LONG`.
#[inline]
fn long(value: i64) -> zval {
    // SAFETY: a zval is plain data; all-zero bytes are `IS_UNDEF`.
    let mut zv: zval = unsafe { std::mem::zeroed() };
    zv.value = zend_value { lval: value };
    zv.u1 = _zval_struct__bindgen_ty_1 { type_info: IS_LONG };
    zv
}

#[inline]
fn undef() -> zval {
    // SAFETY: all-zero bytes are `IS_UNDEF`.
    unsafe { std::mem::zeroed() }
}

/// `zend::function::call(cb, argc, args, nullptr, coroutine)`, warning
/// "`<class>->on<event> handler error`" (`php_swoole_error`) when it fails.
#[inline]
fn call(cb: *mut c_void, args: &mut [zval], coroutine: bool, event: &str) -> Called {
    #[expect(clippy::cast_possible_truncation, reason = "at most five arguments")]
    let argc = args.len() as u32;
    // SAFETY: a live callable and `argc` initialised arguments.
    match unsafe { (host().call)(cb, argc, args.as_mut_ptr(), coroutine) } {
        1 => Called::Done,
        2 => Called::Bailout,
        _ => failed(&args[0], event),
    }
}

/// The warning of a failed call; a user error handler that bails out ends
/// the event there, as upstream.
#[cold]
fn failed(server: &zval, event: &str) -> Called {
    set_last_error(SW_ERROR_PHP_RUNTIME_NOTICE);
    if crate::server::display_errors() {
        // SAFETY: the server object.
        let object = unsafe { object_of(server) };
        // SAFETY: a live object.
        let name = unsafe { class_name(object) };
        let text = message!("{}->{event} handler error", name.to_string_lossy());
        // SAFETY: a warning, from a callback Zend runs.
        if unsafe { php_error(E_WARNING, text.as_cstr()) }.is_err() {
            return Called::Bailout;
        }
    }
    Called::Failed
}

/// `\Swoole\Server\Helper::<name>(...args)` when the library is enabled.
#[inline]
fn helper(facts: &Facts, name: &CStr, args: &mut [zval]) -> bool {
    if !facts.enable_library {
        return false;
    }
    #[expect(clippy::cast_possible_truncation, reason = "at most two arguments")]
    let argc = args.len() as u32;
    // SAFETY: a static name and `argc` initialised arguments.
    unsafe { (host().helper)(name.as_ptr(), argc, args.as_mut_ptr()) }
}

fn facts(server: *mut c_void) -> Facts {
    // SAFETY: a live server.
    let config = unsafe { view::config(server) };
    // SAFETY: as above.
    let gs = unsafe { view::counters(server) };
    let task_worker = crate::server::worker_type() == crate::server::TASK_WORKER;
    Facts {
        master_pid: i64::from(gs.master_pid),
        manager_pid: i64::from(gs.manager_pid),
        event_object: config.event_object,
        enable_coroutine: config.enable_coroutine,
        // `Server::is_enable_coroutine()`.
        coroutine: if task_worker {
            config.task_enable_coroutine
        } else {
            crate::server::worker_type() != crate::server::MANAGER && config.enable_coroutine
        },
        enable_library: swoole_rs_phpswoole::enable_library(),
        task_worker,
    }
}

fn zserv(server: *mut c_void) -> zval {
    // SAFETY: a live server.
    let zserv = unsafe { (host().zserv)(server) };
    // SAFETY: the server's live object zval; copied without a reference, as
    // upstream's `args[0] = *zserv`.
    unsafe { *zserv }
}

/// `object_init_ex(target, ce)` and the object.
fn object(target: &mut zval, class: usize) -> (*mut zend_class_entry, *mut zend_object) {
    let ce = DATA[class].entry();
    // SAFETY: a writable zval and a registered data class.
    unsafe { init_object(target, ce) };
    // SAFETY: an object zval.
    (ce, unsafe { object_of(target) })
}

fn set_long(ce: *mut zend_class_entry, object: *mut zend_object, name: &CStr, value: i64) {
    // SAFETY: a live object of `ce`; Zend copies the name.
    unsafe { zend_update_property_long(ce, object, name.as_ptr(), name.count_bytes(), value) };
}

fn set_double(ce: *mut zend_class_entry, object: *mut zend_object, name: &CStr, value: f64) {
    // SAFETY: as above.
    unsafe { zend_update_property_double(ce, object, name.as_ptr(), name.count_bytes(), value) };
}

fn dtor(value: &mut zval) {
    // SAFETY: a value this dispatch owns.
    unsafe { zval_ptr_dtor(value) };
}

/// `php_swoole_server_onReceive`'s arguments, after `args[0]`, the server,
/// which C++ set. The C++ trampoline calls the callback itself: the hot path
/// needs no `zend_try`, since no Rust frame sits under the call. Returns argc.
/// # Safety
/// A live server and its `RecvData`; `args` has room for four zvals.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpserver_receive_args(
    server: *mut c_void,
    request: *mut c_void,
    fd: i64,
    reactor_id: i64,
    time: f64,
    event_object: bool,
    args: *mut zval,
) -> u32 {
    // SAFETY: per the caller.
    let args = unsafe { std::slice::from_raw_parts_mut(args, 4) };
    let recv_data = host().recv_data;
    if event_object {
        let (ce, object) = object(&mut args[1], EVENT);
        set_long(ce, object, c"fd", fd);
        set_long(ce, object, c"reactor_id", reactor_id);
        set_double(ce, object, c"dispatch_time", time);
        let mut data = undef();
        // SAFETY: a live server and request; `data` receives the payload.
        unsafe { recv_data(server, &raw mut data, request) };
        // SAFETY: a live object of `ce`; the property takes a reference.
        unsafe { zend_update_property(ce, object, c"data".as_ptr(), 4, &raw mut data) };
        dtor(&mut data);
        2
    } else {
        args[1] = long(fd);
        args[2] = long(reactor_id);
        // SAFETY: as above.
        unsafe { recv_data(server, &raw mut args[3], request) };
        4
    }
}

/// After the trampoline's call: the warning and close of a failed call, and
/// the arguments' release. True when the trampoline must re-raise a bailout.
/// # Safety
/// The server, and the arguments `swr_v1_phpserver_receive_args` built.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpserver_receive_done(
    server: *mut c_void,
    succeeded: bool,
    fd: i64,
    args: *mut zval,
    argc: u32,
) -> bool {
    // SAFETY: per the caller.
    let args = unsafe { std::slice::from_raw_parts_mut(args, argc as usize) };
    if !succeeded {
        if failed(&args[0], "onReceive") == Called::Bailout {
            return true;
        }
        // SAFETY: a live server.
        unsafe { (host().close)(server, fd) };
    }
    dtor(&mut args[args.len() - 1]);
    false
}

/// `php_swoole_server_onConnect`/`onClose`'s call, with the callback C++ chose.
/// # Safety
/// A live server and callable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpserver_on_connection(
    server: *mut c_void,
    cb: *mut c_void,
    close: bool,
    fd: i64,
    reactor_id: i64,
    time: f64,
) -> bool {
    let facts = facts(server);
    let mut args = [zserv(server), undef(), undef()];
    let argc = if facts.event_object {
        let (ce, object) = object(&mut args[1], EVENT);
        set_long(ce, object, c"fd", fd);
        set_long(ce, object, c"reactor_id", reactor_id);
        set_double(ce, object, c"dispatch_time", time);
        2
    } else {
        args[1] = long(fd);
        args[2] = long(reactor_id);
        3
    };
    let event = if close { "onClose" } else { "onConnect" };
    if call(cb, &mut args[..argc], facts.enable_coroutine, event) == Called::Bailout {
        return true;
    }
    if facts.event_object {
        dtor(&mut args[1]);
    }
    false
}

/// `php_swoole_server_onBufferFull`/`onBufferEmpty`'s call.
/// # Safety
/// A live server.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpserver_on_buffer(
    server: *mut c_void,
    full: bool,
    fd: i64,
    server_fd: c_int,
) -> bool {
    let (kind, event) = if full {
        (ON_BUFFER_FULL, "onBufferFull")
    } else {
        (ON_BUFFER_EMPTY, "onBufferEmpty")
    };
    // SAFETY: a live server.
    let cb = unsafe { (host().port_callback)(server, server_fd, kind) };
    if cb.is_null() {
        return false;
    }
    let mut args = [zserv(server), long(fd)];
    call(cb, &mut args, false, event) == Called::Bailout
}

fn assoc_long(array: &mut zval, key: &CStr, value: i64) {
    // SAFETY: an array zval; Zend copies the key.
    unsafe { add_assoc_long_ex(array, key.as_ptr(), key.count_bytes(), value) };
}

/// `php_swoole_server_onPacket`'s body after C++ read the datagram.
/// # Safety
/// A live server and a packet view whose strings live for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpserver_on_packet(
    server: *mut c_void,
    packet: *const Packet,
    server_fd: c_int,
    time: f64,
) -> bool {
    // SAFETY: per the caller.
    let packet = unsafe { &*packet };
    let facts = facts(server);
    let mut args = [zserv(server), undef(), undef()];
    let argc = if facts.event_object {
        let (ce, object) = object(&mut args[1], PACKET);
        set_long(ce, object, c"server_socket", i64::from(server_fd));
        set_double(ce, object, c"dispatch_time", time);
        if packet.server_port >= 0 {
            set_long(ce, object, c"server_port", packet.server_port);
        }
        if !packet.address.is_null() {
            // SAFETY: a NUL-terminated address; Zend copies it.
            unsafe {
                zend_update_property_string(ce, object, c"address".as_ptr(), 7, packet.address);
            };
            if packet.port >= 0 {
                set_long(ce, object, c"port", packet.port);
            }
        }
        // SAFETY: the datagram's bytes; Zend copies them.
        unsafe {
            zend_update_property_stringl(
                ce,
                object,
                c"data".as_ptr(),
                4,
                packet.data,
                packet.length,
            );
        }
        2
    } else {
        let mut address = undef();
        // SAFETY: a writable zval.
        unsafe { swr_zend_array_init(&raw mut address) };
        assoc_long(&mut address, c"server_socket", i64::from(server_fd));
        // SAFETY: an array zval; Zend copies the key.
        unsafe { add_assoc_double_ex(&raw mut address, c"dispatch_time".as_ptr(), 13, time) };
        if packet.server_port >= 0 {
            assoc_long(&mut address, c"server_port", packet.server_port);
        }
        if !packet.address.is_null() {
            // SAFETY: an array zval and a NUL-terminated address; Zend copies both.
            unsafe {
                add_assoc_string_ex(&raw mut address, c"address".as_ptr(), 7, packet.address);
            };
            if packet.port >= 0 {
                assoc_long(&mut address, c"port", packet.port);
            }
        }
        // SAFETY: the datagram's bytes, copied into a new string.
        unsafe {
            swoole_rs_zend::sys::swr_zval_stringl(&raw mut args[1], packet.data, packet.length);
        };
        args[2] = address;
        3
    };
    // SAFETY: a live server.
    let cb = unsafe { (host().port_callback)(server, server_fd, ON_PACKET) };
    // Upstream names onPipeMessage in onPacket's warning.
    if call(
        cb,
        &mut args[..argc],
        facts.enable_coroutine,
        "onPipeMessage",
    ) == Called::Bailout
    {
        return true;
    }
    dtor(&mut args[1]);
    if !facts.event_object {
        dtor(&mut args[2]);
    }
    false
}

/// `php_swoole_server_onPipeMessage`'s call with the unpacked `data`.
/// # Safety
/// A live server and a live `data`, which C++ still owns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpserver_on_pipe_message(
    server: *mut c_void,
    data: *mut zval,
    source_worker_id: i64,
    time: f64,
) -> bool {
    let cb = server_callback(server, ON_PIPE_MESSAGE);
    let facts = facts(server);
    let mut args = [zserv(server), undef(), undef()];
    let argc = if facts.event_object {
        let (ce, object) = object(&mut args[1], PIPE_MESSAGE);
        set_long(ce, object, c"worker_id", source_worker_id);
        set_long(ce, object, c"source_worker_id", source_worker_id);
        set_double(ce, object, c"dispatch_time", time);
        // SAFETY: a live object of `ce`; the property takes a reference.
        unsafe { zend_update_property(ce, object, c"data".as_ptr(), 4, data) };
        2
    } else {
        args[1] = long(source_worker_id);
        // SAFETY: borrowed without a reference, as upstream's `args[2] = zresult.value`.
        args[2] = unsafe { *data };
        3
    };
    if call(cb, &mut args[..argc], facts.coroutine, "onPipeMessage") == Called::Bailout {
        return true;
    }
    if facts.event_object {
        dtor(&mut args[1]);
    }
    false
}

/// What a lifecycle event does: its warning name, the helper it runs first and
/// whether its callback may run in a coroutine.
fn lifecycle(kind: c_int) -> Option<(&'static str, &'static CStr, bool)> {
    Some(match kind {
        ON_START => ("onStart", c"onStart", true),
        ON_MANAGER_START => ("onManagerStart", c"onManagerStart", false),
        ON_MANAGER_STOP => ("onManagerStop", c"onManagerStop", false),
        ON_BEFORE_SHUTDOWN => ("onBeforeShutdown", c"onBeforeShutdown", true),
        ON_SHUTDOWN => ("onShutdown", c"onShutdown", false),
        ON_WORKER_START => ("onWorkerStart", c"onWorkerStart", true),
        ON_BEFORE_RELOAD => ("onBeforeReload", c"onBeforeReload", false),
        ON_AFTER_RELOAD => ("onAfterReload", c"onAfterReload", false),
        ON_WORKER_STOP => ("onWorkerStop", c"onWorkerStop", false),
        ON_WORKER_EXIT => ("onWorkerExit", c"onWorkerExit", false),
        _ => return None,
    })
}

fn set_server_long(object: *mut zend_object, name: &CStr, value: i64) {
    set_long(SERVER.entry(), object, name, value);
}

/// The lifecycle events' bodies (`php_swoole_server_onStart` and friends).
/// # Safety
/// A live server; `worker_id` is the worker's for the worker events.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpserver_on_lifecycle(
    server: *mut c_void,
    kind: c_int,
    worker_id: i64,
) -> bool {
    let Some((event, helper_name, coroutine)) = lifecycle(kind) else {
        return false;
    };
    let host = host();
    let cb = server_callback(server, kind);
    let facts = facts(server);
    let mut args = [zserv(server), long(worker_id)];
    // SAFETY: the server's object zval.
    let object = unsafe { object_of(&raw const args[0]) };
    if matches!(kind, ON_START | ON_MANAGER_START | ON_WORKER_START) {
        set_server_long(object, c"master_pid", facts.master_pid);
        set_server_long(object, c"manager_pid", facts.manager_pid);
    }
    if kind == ON_WORKER_START {
        set_server_long(object, c"worker_id", worker_id);
        // SAFETY: a live Server object; Zend copies the name.
        unsafe {
            zend_update_property_bool(
                SERVER.entry(),
                object,
                c"taskworker".as_ptr(),
                10,
                i64::from(facts.task_worker),
            );
        }
        // SAFETY: reads the process id.
        set_server_long(object, c"worker_pid", unsafe { (host.pid)() });
        // SAFETY: a live server.
        unsafe { (host.worker_start)(server) };
    }
    let argc = if matches!(kind, ON_WORKER_START | ON_WORKER_STOP | ON_WORKER_EXIT) {
        2
    } else {
        1
    };
    if helper(&facts, helper_name, &mut args[..argc]) {
        return true;
    }
    if cb.is_null() {
        return false;
    }
    let coroutine = coroutine && facts.coroutine;
    call(cb, &mut args[..argc], coroutine, event) == Called::Bailout
}

/// `php_swoole_server_onWorkerError`'s body.
/// # Safety
/// A live server.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpserver_on_worker_error(
    server: *mut c_void,
    worker_id: i64,
    pid: i64,
    status: i64,
    code: i64,
    signal: i64,
) -> bool {
    let cb = server_callback(server, ON_WORKER_ERROR);
    let facts = facts(server);
    let mut args = [zserv(server), undef(), undef(), undef(), undef()];
    let argc = if facts.event_object {
        let (ce, object) = object(&mut args[1], STATUS_INFO);
        set_long(ce, object, c"worker_id", worker_id);
        set_long(ce, object, c"worker_pid", pid);
        set_long(ce, object, c"status", status);
        set_long(ce, object, c"exit_code", code);
        set_long(ce, object, c"signal", signal);
        2
    } else {
        args[1] = long(worker_id);
        args[2] = long(pid);
        args[3] = long(code);
        args[4] = long(signal);
        5
    };
    if helper(&facts, c"onWorkerError", &mut args[..1]) {
        return true;
    }
    if !cb.is_null() && call(cb, &mut args[..argc], false, "onWorkerError") == Called::Bailout {
        return true;
    }
    if facts.event_object {
        dtor(&mut args[1]);
    }
    false
}

/// The server's object zval, for task.rs's events.
pub(crate) fn server_zval(server: *mut c_void) -> zval {
    zserv(server)
}

/// `server_object->get_callback(kind)`: a `zend::Callable *`, or null.
pub(crate) fn server_callback(server: *mut c_void, kind: c_int) -> *mut c_void {
    crate::lifecycle::callback(server, kind)
}

/// `*php_swoole_server_zval_ptr(serv) = *zserv`: no reference is taken.
pub(crate) fn set_server_zval(server: *mut c_void, zserv: *mut zval) {
    // SAFETY: a live server.
    let slot = unsafe { (host().zserv)(server) };
    // SAFETY: the live object zval.
    let value = unsafe { *zserv };
    // SAFETY: the server's instance slot.
    unsafe { *slot = value };
}

/// The "`<class>->on<event> handler error`" warning of a failed call.
pub(crate) fn handler_error(server: &zval, event: &str) -> Result<(), Bailout> {
    if failed(server, event) == Called::Bailout {
        return Err(Bailout);
    }
    Ok(())
}

pub(crate) use crate::server::set_last_error;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_lifecycle_event_is_named() {
        for kind in [
            ON_START,
            ON_MANAGER_START,
            ON_MANAGER_STOP,
            ON_BEFORE_SHUTDOWN,
            ON_SHUTDOWN,
            ON_WORKER_START,
            ON_BEFORE_RELOAD,
            ON_AFTER_RELOAD,
            ON_WORKER_STOP,
            ON_WORKER_EXIT,
        ] {
            let (event, helper, _) = lifecycle(kind).unwrap();
            assert_eq!(event.as_bytes(), helper.to_bytes());
        }
        assert!(lifecycle(ON_WORKER_ERROR).is_none());
        assert!(lifecycle(ON_PIPE_MESSAGE).is_none());
    }
}
