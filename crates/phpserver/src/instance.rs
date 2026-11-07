//! What the Server's other bindings ask of its PHP object: the running server's
//! object zval, a Server object's `Server`, its ports' callbacks and a
//! request's payload (were ext-src/swoole_server.cc's `php_swoole_server_*`).
//! They keep their upstream symbols for crates/phphttp and crates/phpredis.
use crate::port::PortProperty;
use crate::server::{ServerData, THREAD};
use std::{
    cell::UnsafeCell,
    ffi::{c_char, c_int, c_long, c_void},
    mem::MaybeUninit,
    ptr,
    sync::atomic::Ordering,
};
use swoole_rs_corebase::{
    DataHead,
    constants::EventDataFlag::{SW_EVENT_DATA_OBJ_PTR, SW_EVENT_DATA_POP_PTR},
};
use swoole_rs_server::{port_config, server_methods, view};
use swoole_rs_zend::{
    Object, object_of, string,
    sys::{swr_zend_mkdir_recursive, swr_zval_empty_string, swr_zval_stringl, zend_string, zval},
    value::set_str,
};

unsafe extern "C" {
    /// `MessageBus::move_packet` (crates/protocols).
    fn swr_v1_bus_move_packet(bus: *mut c_void) -> *mut c_char;
    /// `String::pop` (crates/buffer).
    #[link_name = "_ZN6swoole6String3popEm"]
    fn string_pop(string: *mut c_void, init_size: usize) -> *mut u8;
    #[link_name = "_ZN6swoole9coroutine6System5sleepEd"]
    fn system_sleep(seconds: f64) -> c_int;
}

/// `swoole::RecvData`.
#[repr(C)]
struct RecvData {
    info: DataHead,
    data: *mut u8,
}

/// Upstream's `static SW_THREAD_LOCAL zval swoole_server_instance`: one per
/// process, or one per thread under `SW_THREAD`.
struct Instance(UnsafeCell<MaybeUninit<zval>>);

// SAFETY: written only by the thread that constructs the Server and read by the
// PHP threads after it, as upstream's global was.
unsafe impl Sync for Instance {}

static INSTANCE: Instance = Instance(UnsafeCell::new(MaybeUninit::zeroed()));

thread_local! {
    static THREAD_INSTANCE: Instance = const { Instance(UnsafeCell::new(MaybeUninit::zeroed())) };
}

/// `php_swoole_server_zval_ptr`: the running server's object zval (`IS_UNDEF` until bound).
#[unsafe(export_name = "_Z26php_swoole_server_zval_ptrPN6swoole6ServerE")]
pub extern "C" fn php_swoole_server_zval_ptr(_serv: *mut c_void) -> *mut zval {
    if THREAD.load(Ordering::Relaxed) {
        THREAD_INSTANCE.with(|slot| slot.0.get().cast())
    } else {
        INSTANCE.0.get().cast()
    }
}

/// `php_swoole_server_get_ptr`: the `Server` of a Server object, or null.
/// # Safety
/// `zobject` is a live Server object zval.
#[unsafe(export_name = "_Z25php_swoole_server_get_ptrP12_zval_struct")]
pub unsafe extern "C" fn php_swoole_server_get_ptr(zobject: *mut zval) -> *mut c_void {
    // SAFETY: per the caller.
    let object = unsafe { object_of(zobject) };
    // SAFETY: C++ and Rust allocate Servers as `Object<ServerData>`.
    unsafe { Object::<ServerData>::data(object) }.serv
}

/// `php_swoole_server_get_port_property`: the port's `ServerPortProperty`, or null.
fn port_property<'a>(port: *mut c_void) -> Option<&'a PortProperty> {
    if port.is_null() {
        return None;
    }
    // SAFETY: a live `ListenPort`.
    let config = unsafe { port_config::of(port) };
    // SAFETY: the port's own `ptr` field.
    let property = unsafe {
        crate::server::swr_v1_phpserver_port_property(
            config.object_id,
            &raw mut config.ptr,
            ptr::null_mut(),
            false,
        )
    };
    // SAFETY: null, or the property its Port object holds while the port lives.
    unsafe { property.cast::<PortProperty>().as_ref() }
}

/// The port's callback `kind`, or null.
fn port_callback(port: *mut c_void, kind: c_int) -> *mut c_void {
    port_property(port).map_or(ptr::null_mut(), |property| property.callback(kind))
}

/// `php_swoole_server_isset_callback`: the port, or the primary port, has callback `kind`.
/// # Safety
/// `serv` is a live Server and `port` null or one of its ports.
#[unsafe(export_name = "_Z32php_swoole_server_isset_callbackPN6swoole6ServerEPNS_10ListenPortEi")]
pub unsafe extern "C" fn php_swoole_server_isset_callback(
    serv: *mut c_void,
    port: *mut c_void,
    kind: c_int,
) -> bool {
    // SAFETY: per the caller.
    let primary = unsafe { view::port_at(serv, 0) };
    !port_callback(port, kind).is_null() || !port_callback(primary, kind).is_null()
}

/// `php_swoole_server_get_callback`: callback `kind` of the port listening on
/// `server_fd`, else the primary port's; null without that port.
/// # Safety
/// `serv` is a live Server.
#[unsafe(export_name = "_Z30php_swoole_server_get_callbackPN6swoole6ServerEii")]
pub unsafe extern "C" fn php_swoole_server_get_callback(
    serv: *mut c_void,
    server_fd: c_int,
    kind: c_int,
) -> *mut c_void {
    // SAFETY: per the caller.
    let port = unsafe { view::port_by_server_fd(serv, server_fd) };
    if port.is_null() {
        return ptr::null_mut();
    }
    let callback = port_callback(port, kind);
    if !callback.is_null() {
        return callback;
    }
    // SAFETY: per the caller.
    port_callback(unsafe { view::port_at(serv, 0) }, kind)
}

/// `serv->get_worker_message_bus()->move_packet()`.
fn move_packet(serv: *mut c_void) {
    let bus = crate::dispatch::message_bus(serv);
    // SAFETY: a `MessageBus` starts with its `swr_bus *`.
    let bus = unsafe { *bus.cast::<*mut c_void>() };
    // SAFETY: the worker's live bus; the packet is now the caller's.
    unsafe { swr_v1_bus_move_packet(bus) };
}

/// `zend::assign_zend_string_by_val`: the zend string whose bytes start at
/// `data`, cut to `length`, into `zdata`.
/// # Safety
/// `data` is `ZSTR_VAL` of a live, unshared string of at least `length` bytes;
/// `zdata` is writable.
unsafe fn assign_by_val(zdata: *mut zval, data: *mut u8, length: usize) {
    let text = string::from_val(data);
    // SAFETY: per the caller.
    unsafe { string::truncate(text, length) };
    // SAFETY: per the caller; `zdata` takes the reference.
    unsafe { set_str(zdata, text) };
}

/// `php_swoole_get_recv_data`: a request's payload into `zdata`, taking the
/// message bus's packet or the connection's receive buffer when it holds them.
/// # Safety
/// `serv` is the running server, `req` its live `RecvData`, `zdata` writable.
#[unsafe(export_name = "_Z24php_swoole_get_recv_dataPN6swoole6ServerEP12_zval_structPNS_8RecvDataE")]
pub unsafe extern "C" fn php_swoole_get_recv_data(
    serv: *mut c_void,
    zdata: *mut zval,
    req: *mut c_void,
) {
    // SAFETY: per the caller.
    let req = unsafe { &*req.cast::<RecvData>() };
    let (flags, length) = (req.info.flags, req.info.len as usize);
    if length == 0 {
        // SAFETY: per the caller.
        unsafe { swr_zval_empty_string(zdata) };
    } else if flags & SW_EVENT_DATA_OBJ_PTR as u8 != 0 {
        // SAFETY: an OBJ_PTR payload is the bus's packet, a zend string.
        unsafe { assign_by_val(zdata, req.data, length) };
        move_packet(serv);
    } else if let Some(buffer) = (flags & SW_EVENT_DATA_POP_PTR as u8 != 0)
        // SAFETY: per the caller; a POP_PTR request's session is live.
        .then(|| unsafe { view::by_session(serv, req.info.fd) })
        .flatten()
        // SAFETY: the connection's live socket.
        .map(|conn| unsafe { view::recv_buffer(conn) })
        .filter(|buffer| !buffer.is_null())
    {
        // SAFETY: per the caller.
        let size = unsafe { view::config(serv) }.recv_buffer_size;
        // SAFETY: the connection's receive buffer, which holds the payload.
        let data = unsafe { string_pop(buffer, size) };
        // SAFETY: `pop` hands over its zend-allocated block of the payload.
        unsafe { assign_by_val(zdata, data, length) };
    } else {
        // SAFETY: `length` bytes at `data`.
        unsafe { swr_zval_stringl(zdata, req.data.cast(), length) };
    }
}

/// `php_swoole_create_dir`: 0 if `path` exists or was made with its parents, else -1.
/// # Safety
/// `path` is NUL-terminated after `length` bytes.
#[unsafe(export_name = "_Z21php_swoole_create_dirPKcm")]
#[must_use]
pub unsafe extern "C" fn php_swoole_create_dir(path: *const c_char, _length: usize) -> c_int {
    // SAFETY: per the caller.
    if unsafe { libc::access(path, libc::F_OK) } == 0 {
        return 0;
    }
    // SAFETY: per the caller.
    if unsafe { swr_zend_mkdir_recursive(path) } {
        0
    } else {
        -1
    }
}

/// `php_swoole_server_check_kernel_nobufs`: in a coroutine, pause 10 ms after a
/// send the kernel refused for lack of buffers.
fn check_kernel_nobufs(serv: *mut c_void, session_id: c_long) {
    if swoole_rs_phpcoroutines::in_coroutine()
        // SAFETY: a live server.
        && unsafe { server_methods::has_kernel_nobufs_error(serv, session_id) }
    {
        // SAFETY: in a coroutine.
        unsafe { system_sleep(0.01) };
    }
}

/// `serv->send(fd, data)`, then the `send_yield` wait or the kernel-nobufs pause.
/// # Safety
/// `serv` is the running server and `data` a live string.
pub(crate) unsafe fn send(serv: *mut c_void, fd: c_long, data: *mut zend_string) -> bool {
    // SAFETY: per the caller.
    let bytes = unsafe { string::bytes(data) };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Server::send takes a uint32_t length, as upstream's ZSTR_LEN narrowing"
    )]
    let length = bytes.len() as u32;
    // SAFETY: per the caller; `length` bytes.
    let sent = unsafe { server_methods::send(serv, fd, bytes.as_ptr().cast(), length) };
    if !sent
        && swoole_rs_corebase::log::last_error() == crate::lifecycle::SW_ERROR_OUTPUT_SEND_YIELD
    {
        // SAFETY: per the caller.
        return unsafe { crate::dispatch::php_swoole_server_send_yield(serv, fd, data) };
    }
    check_kernel_nobufs(serv, fd);
    sent
}
