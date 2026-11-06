//! Rust-owned `ListenPort` settings and native socket/statistic loans.
use core::ffi::{c_int, c_void};
#[repr(C)]
pub struct PortConfig {
    pub object_id: u16,
    pub backlog: u16,
    pub listening: bool,
    pub tcp_defer_accept: c_int,
    pub tcp_fastopen: c_int,
    pub tcp_keepidle: c_int,
    pub tcp_keepinterval: c_int,
    pub tcp_keepcount: c_int,
    pub tcp_user_timeout: c_int,
    pub max_idle_time: f64,
    pub socket_buffer_size: c_int,
    pub buffer_high_watermark: u32,
    pub buffer_low_watermark: u32,
    pub r#type: c_int,
    pub ssl: u8,
    pub port: c_int,
    pub socket: *mut c_void,
    pub thread_id: libc::pthread_t,
    pub heartbeat_idle_time: u16,
    pub open_eof_check: bool,
    pub open_http_protocol: bool,
    pub open_http2_protocol: bool,
    pub open_websocket_protocol: bool,
    pub open_length_check: bool,
    pub open_mqtt_protocol: bool,
    pub open_redis_protocol: bool,
    pub open_tcp_nodelay: bool,
    pub open_tcp_nopush: bool,
    pub open_tcp_keepalive: bool,
    pub kernel_socket_recv_buffer_size: c_int,
    pub kernel_socket_send_buffer_size: c_int,
    pub gs: *mut c_void,
    pub ptr: *mut c_void,
}

/// The settings of a `ListenPort`, which begins with its `PortFields` pointer to them.
///
/// # Safety
/// `port` is a live `ListenPort`, and nothing else accesses its settings while
/// the borrow lasts.
pub(crate) unsafe fn of<'a>(port: *mut c_void) -> &'a mut PortConfig {
    // SAFETY: per the caller; `ListenPort` is not polymorphic (master.cc asserts it).
    let config = unsafe { *port.cast::<*mut PortConfig>() };
    // SAFETY: the port's settings live as long as it.
    unsafe { &mut *config }
}

/// Allocates settings before the native port acquires any resources.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_port_config_new(buffer_size: c_int) -> *mut PortConfig {
    Box::into_raw(Box::new(PortConfig {
        object_id: 0,
        backlog: 512,
        listening: false,
        tcp_defer_accept: 0,
        tcp_fastopen: 0,
        tcp_keepidle: 3600,
        tcp_keepinterval: 60,
        tcp_keepcount: 5,
        tcp_user_timeout: 0,
        max_idle_time: 0.0,
        socket_buffer_size: buffer_size,
        buffer_high_watermark: 0,
        buffer_low_watermark: 0,
        r#type: 1,
        ssl: 0,
        port: 0,
        socket: core::ptr::null_mut(),
        // SAFETY: pthread_t is an integer or pointer; zero is the original native empty sentinel.
        thread_id: unsafe { core::mem::zeroed() },
        heartbeat_idle_time: 0,
        open_eof_check: false,
        open_http_protocol: false,
        open_http2_protocol: false,
        open_websocket_protocol: false,
        open_length_check: false,
        open_mqtt_protocol: false,
        open_redis_protocol: false,
        open_tcp_nodelay: false,
        open_tcp_nopush: true,
        open_tcp_keepalive: false,
        kernel_socket_recv_buffer_size: 0,
        kernel_socket_send_buffer_size: 0,
        gs: core::ptr::null_mut(),
        ptr: core::ptr::null_mut(),
    }))
}
/// # Safety
/// `config` is a unique live allocation from `config_new`; no views remain.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_port_config_free(config: *mut PortConfig) {
    if !config.is_null() {
        // SAFETY: the native base releases its unique allocation after all derived members.
        drop(unsafe { Box::from_raw(config) });
    }
}
