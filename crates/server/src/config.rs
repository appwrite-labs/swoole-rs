//! Rust-owned configuration and native-object loans for one Server.
//!
//! C++ retains borrowed reference views for the original embedding API. The
//! configuration allocation and defaults belong to `ServerState`; Rust server
//! operations read these records directly, without per-field native callbacks.
use core::ffi::{c_int, c_void};
use swoole_rs_memory::members::Shm;

/// The independently configurable server fields and borrowed native handles.
#[repr(C)]
pub struct ServerConfig {
    pub reactor_num: u16,
    pub worker_num: u32,
    pub dgram_port_num: u8,
    pub dispatch_mode: u8,
    pub scheduler_warning: bool,
    pub max_request: u32,
    pub max_request_grace: u32,
    pub udp_socket_ipv4: *mut c_void,
    pub udp_socket_ipv6: *mut c_void,
    pub dgram_socket: *mut c_void,
    pub max_wait_time: u32,
    pub worker_max_concurrency: u32,
    pub daemonize: bool,
    pub have_dgram_sock: bool,
    pub have_stream_sock: bool,
    pub open_cpu_affinity: bool,
    pub disable_notify: bool,
    pub discard_timeout_request: bool,
    pub http_parse_cookie: bool,
    pub http_parse_post: bool,
    pub http_parse_files: bool,
    pub http_compression: bool,
    pub websocket_compression: bool,
    pub enable_static_handler: bool,
    pub http_autoindex: bool,
    pub enable_unsafe_event: bool,
    pub enable_delay_receive: bool,
    pub enable_reuse_port: bool,
    pub reload_async: bool,
    pub event_object: bool,
    pub task_object: bool,
    pub task_enable_coroutine: bool,
    pub send_yield: bool,
    pub enable_coroutine: bool,
    pub single_thread: bool,
    pub running: bool,
    /// The CPUs `cpu_affinity_ignore` leaves workers; empty for every CPU.
    pub cpu_affinity_available: Vec<c_int>,
    pub pipe_command: *mut c_void,
    pub send_timeout: f64,
    pub heartbeat_check_interval: u16,
    pub warning_time: libc::time_t,
    pub timezone_: libc::c_long,
    pub master_timer: *mut c_void,
    pub heartbeat_timer: *mut c_void,
    pub output_buffer_size: u32,
    pub input_buffer_size: u32,
    pub max_queued_bytes: u32,
    pub ipc_max_size: u32,
    pub private_data_1: *mut c_void,
    pub private_data_2: *mut c_void,
    pub private_data_3: *mut c_void,
    pub private_data_4: *mut c_void,
    pub factory_: *mut c_void,
    pub manager_: *mut c_void,
    pub task_worker_num: u32,
    pub task_ipc_mode: u8,
    pub task_max_request: u32,
    pub task_max_request_grace: u32,
    pub task_results: Shm,
    pub worker_registry: *mut c_void,
    pub user_workers: Shm,
    pub workers: Shm,
    pub gs: Shm,
    pub upload_max_filesize: usize,
    pub http_compression_level: u8,
    pub compression_min_length: u32,
    pub recv_buffer_allocator: *const c_void,
    pub recv_buffer_size: usize,
    pub message_queue_key: u64,
    /// `Server::Mode`, fixed by the constructor.
    pub mode_: u8,
}

impl Drop for ServerConfig {
    fn drop(&mut self) {
        let registry = self
            .worker_registry
            .cast::<swoole_rs_corebase::worker_registry::WorkerRegistry>();
        if !registry.is_null() {
            // SAFETY: ServerState boxed this registry; native configuration views
            // only borrow its handle and have ended before the owning config is freed.
            drop(unsafe { Box::from_raw(registry) });
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            reactor_num: 0,
            worker_num: 0,
            dgram_port_num: 0,
            dispatch_mode: 2,
            scheduler_warning: false,
            max_request: 0,
            max_request_grace: 0,
            udp_socket_ipv4: core::ptr::null_mut(),
            udp_socket_ipv6: core::ptr::null_mut(),
            dgram_socket: core::ptr::null_mut(),
            max_wait_time: 3,
            worker_max_concurrency: u32::MAX,
            daemonize: false,
            have_dgram_sock: false,
            have_stream_sock: false,
            open_cpu_affinity: false,
            disable_notify: false,
            discard_timeout_request: false,
            http_parse_cookie: true,
            http_parse_post: true,
            http_parse_files: false,
            http_compression: false,
            websocket_compression: false,
            enable_static_handler: false,
            http_autoindex: false,
            enable_unsafe_event: false,
            enable_delay_receive: false,
            enable_reuse_port: false,
            reload_async: true,
            event_object: false,
            task_object: false,
            task_enable_coroutine: false,
            send_yield: true,
            enable_coroutine: true,
            single_thread: false,
            running: true,
            cpu_affinity_available: Vec::new(),
            pipe_command: core::ptr::null_mut(),
            send_timeout: 0.0,
            heartbeat_check_interval: 0,
            warning_time: 0,
            timezone_: 0,
            master_timer: core::ptr::null_mut(),
            heartbeat_timer: core::ptr::null_mut(),
            output_buffer_size: u32::MAX,
            input_buffer_size: 2 * 1024 * 1024,
            max_queued_bytes: 0,
            ipc_max_size: if cfg!(target_vendor = "apple") {
                2048
            } else {
                8192
            },
            private_data_1: core::ptr::null_mut(),
            private_data_2: core::ptr::null_mut(),
            private_data_3: core::ptr::null_mut(),
            private_data_4: core::ptr::null_mut(),
            factory_: core::ptr::null_mut(),
            manager_: core::ptr::null_mut(),
            task_worker_num: 0,
            task_ipc_mode: 1,
            task_max_request: 0,
            task_max_request_grace: 0,
            task_results: Shm::NULL,
            worker_registry: core::ptr::null_mut(),
            user_workers: Shm::NULL,
            workers: Shm::NULL,
            gs: Shm::NULL,
            upload_max_filesize: 0,
            http_compression_level: 0,
            compression_min_length: 20,
            recv_buffer_allocator: core::ptr::null(),
            recv_buffer_size: 65_536,
            message_queue_key: 0,
            mode_: 0,
        }
    }
}

#[cfg(any(test, feature = "upstream-symbols"))]
impl ServerConfig {
    /// The platform/runtime-dependent defaults a new server starts with.
    pub(crate) fn init_defaults(
        &mut self,
        cpu_num: u32,
        timezone: libc::c_long,
        allocator: *const c_void,
    ) {
        self.reactor_num = u16::try_from(cpu_num.min(8)).unwrap_or(8);
        self.worker_num = cpu_num;
        self.timezone_ = timezone;
        self.http_compression = true;
        self.http_compression_level = 1;
        self.compression_min_length = 20;
        self.recv_buffer_allocator = allocator;
    }
}

/// Handles only owned configuration/table facts; all effects on other objects stay native.
///
/// # Safety
/// The caller lends a live state and synchronizes its field access as for the native view.
#[inline(always)]
pub(crate) unsafe fn native_fact(state: &crate::ServerState, op: u32, arg: u64) -> Option<u64> {
    use crate::ops::{
        FIELD_CPU_NUM, FIELD_DISPATCH_MODE, FIELD_HEARTBEAT_INTERVAL, FIELD_REACTOR_NUM,
        FIELD_TASK_IPC_MODE, FIELD_TASK_WORKER_NUM, FIELD_UNSAFE_EVENT, FIELD_WORKER_NUM,
        OP_ACCEPT_TIMER_CLEAR, OP_CONNECTION_LIST, OP_DELAY_RECEIVE, OP_DISABLE_NOTIFY, OP_ERRNO,
        OP_FIELD, OP_LAST_PORT_FD, OP_LC_DAEMONIZE, OP_MAX_CONNECTION, OP_MAX_WAIT_TIME, OP_MODE,
        OP_PORT_COUNT, OP_PORT_DEFAULT_IDLE, OP_PORT_DGRAM, OP_PORT_FD, OP_PORT_IDLE,
        OP_PORT_REMOVED, OP_PORT_SSL_AT, OP_PORT_STREAM, OP_PORT_WEBSOCKET, OP_RUNNING,
        OP_SET_FIELD, OP_SET_MAX_CONNECTION, OP_SET_RUNNING, OP_SET_WARNING, OP_SINGLE_THREAD,
    };
    if !matches!(
        op,
        OP_MODE
            | OP_SINGLE_THREAD
            | OP_LC_DAEMONIZE
            | OP_DELAY_RECEIVE
            | OP_DISABLE_NOTIFY
            | OP_RUNNING
            | OP_SET_RUNNING
            | OP_MAX_WAIT_TIME
            | OP_FIELD
            | OP_SET_FIELD
            | OP_SET_WARNING
            | OP_MAX_CONNECTION
            | OP_SET_MAX_CONNECTION
            | OP_CONNECTION_LIST
            | OP_ACCEPT_TIMER_CLEAR
            | OP_ERRNO
            | OP_PORT_COUNT
            | OP_PORT_DGRAM
            | OP_PORT_STREAM
            | OP_PORT_SSL_AT
            | OP_PORT_WEBSOCKET
            | OP_PORT_IDLE
            | OP_PORT_DEFAULT_IDLE
            | OP_PORT_REMOVED
            | OP_PORT_FD
            | OP_LAST_PORT_FD
    ) {
        return None;
    }
    // SAFETY: no native callback runs during this leaf record access.
    let config = unsafe { &mut *state.config() };
    // SAFETY: no native callback runs during this leaf table access.
    let tables = unsafe { &mut *state.tables() };
    Some(match op {
        OP_MODE => u64::from(config.mode_),
        OP_SINGLE_THREAD => u64::from(config.single_thread),
        OP_DELAY_RECEIVE => u64::from(config.enable_delay_receive),
        OP_DISABLE_NOTIFY => u64::from(config.disable_notify),
        OP_RUNNING => u64::from(config.running),
        OP_LC_DAEMONIZE => u64::from(config.daemonize),
        OP_MAX_WAIT_TIME => u64::from(config.max_wait_time),
        OP_MAX_CONNECTION => u64::from(tables.max_connection),
        OP_CONNECTION_LIST => u64::from(!tables.connection_list.is_null()),
        OP_ERRNO => {
            i64::from(std::io::Error::last_os_error().raw_os_error().unwrap_or(0)).cast_unsigned()
        }
        OP_PORT_COUNT => u64::from(tables.ports.count),
        OP_PORT_DGRAM | OP_PORT_STREAM | OP_PORT_SSL_AT | OP_PORT_WEBSOCKET | OP_PORT_IDLE
        | OP_PORT_DEFAULT_IDLE | OP_PORT_REMOVED | OP_PORT_FD | OP_LAST_PORT_FD => {
            let ports = tables.ports();
            let port = if op == OP_LAST_PORT_FD {
                ports.last()
            } else {
                ports.get(usize::try_from(arg).ok()?)
            };
            let port = *port?;
            match op {
                OP_PORT_DEFAULT_IDLE => {
                    // SAFETY: a listed port is live, and no native callback runs while
                    // it is borrowed.
                    unsafe { port.config_mut() }.heartbeat_idle_time =
                        config.heartbeat_check_interval.wrapping_mul(2);
                    return Some(0);
                }
                OP_PORT_REMOVED => {
                    return Some(u64::from(port.with_socket(|s| s.read(|s| s.removed))?));
                }
                _ => {}
            }
            let fd = port.fd();
            let port = port.config();
            match op {
                OP_PORT_DGRAM => u64::from(matches!(port.r#type, 2 | 4 | 6)),
                OP_PORT_STREAM => u64::from(matches!(port.r#type, 1 | 3 | 5)),
                OP_PORT_SSL_AT => u64::from(port.ssl),
                OP_PORT_WEBSOCKET => u64::from(port.open_websocket_protocol),
                OP_PORT_IDLE => u64::from(port.heartbeat_idle_time),
                _ => i64::from(fd).cast_unsigned(),
            }
        }
        OP_SET_MAX_CONNECTION => {
            tables.max_connection = u32::try_from(arg & u64::from(u32::MAX)).unwrap_or_default();
            0
        }
        OP_ACCEPT_TIMER_CLEAR => {
            tables.enable_accept_timer = core::ptr::null_mut();
            0
        }
        OP_SET_RUNNING => {
            config.running = arg != 0;
            0
        }
        OP_SET_WARNING => {
            config.scheduler_warning = true;
            0
        }
        OP_FIELD => match u32::try_from(arg).unwrap_or(u32::MAX) {
            FIELD_REACTOR_NUM => u64::from(config.reactor_num),
            FIELD_WORKER_NUM => u64::from(config.worker_num),
            FIELD_TASK_WORKER_NUM => u64::from(config.task_worker_num),
            FIELD_HEARTBEAT_INTERVAL => u64::from(config.heartbeat_check_interval),
            FIELD_DISPATCH_MODE => u64::from(config.dispatch_mode),
            FIELD_UNSAFE_EVENT => u64::from(config.enable_unsafe_event),
            FIELD_TASK_IPC_MODE => u64::from(config.task_ipc_mode),
            FIELD_CPU_NUM => u64::from(swoole_rs_corebase::swr_v1_core_cpu_num()),
            _ => 0,
        },
        OP_SET_FIELD => {
            let value = arg >> 8;
            match u32::try_from(arg & 0xff).unwrap_or(u32::MAX) {
                FIELD_REACTOR_NUM => {
                    config.reactor_num =
                        u16::try_from(value & u64::from(u16::MAX)).unwrap_or_default();
                }
                FIELD_WORKER_NUM => {
                    config.worker_num =
                        u32::try_from(value & u64::from(u32::MAX)).unwrap_or_default();
                }
                FIELD_TASK_WORKER_NUM => {
                    config.task_worker_num =
                        u32::try_from(value & u64::from(u32::MAX)).unwrap_or_default();
                }
                FIELD_HEARTBEAT_INTERVAL => {
                    config.heartbeat_check_interval =
                        u16::try_from(value & u64::from(u16::MAX)).unwrap_or_default();
                }
                _ => {}
            }
            0
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::{FIELD_WORKER_NUM, OP_FIELD, OP_RUNNING, OP_SET_FIELD, OP_SET_RUNNING};
    use crate::state::{swr_v1_server_state_free, swr_v1_server_state_new};

    #[test]
    fn runtime_defaults_and_native_views_share_owned_slots() {
        let state = swr_v1_server_state_new();
        unsafe {
            let config = (*state).config();
            (*config).init_defaults(16, 3600, core::ptr::null());
            assert_eq!((*config).worker_num, 16);
            assert_eq!((*config).reactor_num, 8);
            assert_eq!((*config).timezone_, 3600);
            assert_eq!(
                native_fact(&*state, OP_FIELD, u64::from(FIELD_WORKER_NUM)),
                Some(16)
            );
            let encoded = (19 << 8) | u64::from(FIELD_WORKER_NUM);
            assert_eq!(native_fact(&*state, OP_SET_FIELD, encoded), Some(0));
            assert_eq!((*config).worker_num, 19);
            assert_eq!(native_fact(&*state, OP_SET_RUNNING, 0), Some(0));
            assert!(!(*config).running);
            assert_eq!(native_fact(&*state, OP_RUNNING, 0), Some(0));
            swr_v1_server_state_free(state);
        }
    }

    #[test]
    fn port_facts_read_the_pushed_ports() {
        use crate::ops::{
            OP_LAST_PORT_FD, OP_PORT_COUNT, OP_PORT_DEFAULT_IDLE, OP_PORT_DGRAM, OP_PORT_FD,
            OP_PORT_IDLE, OP_PORT_REMOVED, OP_PORT_STREAM,
        };
        let state = swr_v1_server_state_new();
        let mut socket = swoole_rs_reactor::SocketRecord::new();
        socket.fd = 7;
        socket.removed = 1;
        let ports: Vec<_> = [1, 2]
            .into_iter()
            .map(|kind| {
                let config = crate::port_config::swr_v1_port_config_new(0);
                unsafe {
                    (*config).r#type = kind;
                    (*config).socket = (&raw mut socket).cast();
                }
                // A `ListenPort` is its settings.
                config
            })
            .collect();
        unsafe {
            assert_eq!(native_fact(&*state, OP_PORT_COUNT, 0), Some(0));
            assert_eq!(
                native_fact(&*state, OP_PORT_DGRAM, 0),
                None,
                "no port 0 yet"
            );
            for &port in &ports {
                (*state).push_port(port.cast());
            }
            (*(*state).config()).heartbeat_check_interval = 5;
            assert_eq!(native_fact(&*state, OP_PORT_COUNT, 0), Some(2));
            assert_eq!((*ports[1]).object_id, 2);
            assert_eq!(native_fact(&*state, OP_PORT_STREAM, 0), Some(1));
            assert_eq!(native_fact(&*state, OP_PORT_DGRAM, 0), Some(0));
            assert_eq!(native_fact(&*state, OP_PORT_DGRAM, 1), Some(1));
            assert_eq!(native_fact(&*state, OP_PORT_REMOVED, 1), Some(1));
            assert_eq!(native_fact(&*state, OP_PORT_FD, 1), Some(7));
            assert_eq!(native_fact(&*state, OP_LAST_PORT_FD, 0), Some(7));
            assert_eq!(native_fact(&*state, OP_PORT_DEFAULT_IDLE, 1), Some(0));
            assert_eq!(native_fact(&*state, OP_PORT_IDLE, 1), Some(10));
            swr_v1_server_state_free(state);
            for port in ports {
                crate::port_config::swr_v1_port_config_free(port);
            }
        }
    }
}
