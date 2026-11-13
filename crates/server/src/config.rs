//! Rust-owned configuration and native-object loans for one Server.
//!
//! C++ retains borrowed reference views for the original embedding API. The
//! configuration allocation and defaults belong to `ServerState`; Rust server
//! operations read these records directly, without per-field native callbacks.
use core::ffi::{c_int, c_void};
use core::sync::atomic::{
    AtomicBool, AtomicI64, AtomicPtr, AtomicU8, AtomicU16, AtomicU32, AtomicU64, AtomicUsize,
    Ordering::Relaxed,
};
use swoole_rs_corebase::worker_registry::WorkerRegistry;
use swoole_rs_memory::members::Shm;

const _: () = assert!(
    size_of::<libc::time_t>() == size_of::<AtomicI64>(),
    "`warning_time` mirrors C++'s `time_t`"
);

/// A value a [`Setting`] holds, and the atomic of its layout that holds it.
pub trait Plain: Copy {
    type Atomic;
    fn atomic(self) -> Self::Atomic;
    fn load(atomic: &Self::Atomic) -> Self;
    fn store(atomic: &Self::Atomic, value: Self);
}

macro_rules! plain {
    ($($value:ty => $atomic:ty;)*) => {$(
        impl Plain for $value {
            type Atomic = $atomic;
            fn atomic(self) -> $atomic {
                <$atomic>::new(self)
            }
            fn load(atomic: &$atomic) -> Self {
                atomic.load(Relaxed)
            }
            fn store(atomic: &$atomic, value: Self) {
                atomic.store(value, Relaxed);
            }
        }
    )*};
}

plain! {
    bool => AtomicBool;
    u8 => AtomicU8;
    u16 => AtomicU16;
    u32 => AtomicU32;
    u64 => AtomicU64;
    usize => AtomicUsize;
}

/// A `double` setting, kept as its bits.
impl Plain for f64 {
    type Atomic = AtomicU64;
    fn atomic(self) -> AtomicU64 {
        AtomicU64::new(self.to_bits())
    }
    fn load(atomic: &AtomicU64) -> Self {
        Self::from_bits(atomic.load(Relaxed))
    }
    fn store(atomic: &AtomicU64, value: Self) {
        atomic.store(value.to_bits(), Relaxed);
    }
}

/// A server setting: written through `&` while the server is configured and
/// read by every thread once it runs, so an atomic, read and written relaxed.
/// It is laid out as the value it holds, which C++'s view of the record reads.
#[repr(transparent)]
pub struct Setting<T: Plain>(T::Atomic);

impl<T: Plain> Setting<T> {
    pub fn new(value: T) -> Self {
        Self(value.atomic())
    }

    #[inline]
    pub fn get(&self) -> T {
        T::load(&self.0)
    }

    #[inline]
    pub fn set(&self, value: T) {
        T::store(&self.0, value);
    }
}

/// The independently configurable server fields and borrowed native handles.
#[repr(C)]
pub struct ServerConfig {
    pub reactor_num: Setting<u16>,
    pub worker_num: Setting<u32>,
    pub dgram_port_num: u8,
    pub dispatch_mode: Setting<u8>,
    /// Set and cleared by whichever thread dispatches, so an atomic.
    pub scheduler_warning: AtomicBool,
    pub max_request: Setting<u32>,
    pub max_request_grace: Setting<u32>,
    pub udp_socket_ipv4: *mut c_void,
    pub udp_socket_ipv6: *mut c_void,
    pub dgram_socket: *mut c_void,
    pub max_wait_time: Setting<u32>,
    pub worker_max_concurrency: Setting<u32>,
    pub daemonize: Setting<bool>,
    pub have_dgram_sock: bool,
    pub have_stream_sock: bool,
    pub open_cpu_affinity: Setting<bool>,
    pub disable_notify: bool,
    pub discard_timeout_request: Setting<bool>,
    pub http_parse_cookie: Setting<bool>,
    pub http_parse_post: Setting<bool>,
    pub http_parse_files: Setting<bool>,
    pub http_compression: Setting<bool>,
    pub websocket_compression: Setting<bool>,
    pub enable_static_handler: Setting<bool>,
    pub http_autoindex: Setting<bool>,
    pub enable_unsafe_event: Setting<bool>,
    pub enable_delay_receive: Setting<bool>,
    pub enable_reuse_port: Setting<bool>,
    pub reload_async: Setting<bool>,
    pub event_object: Setting<bool>,
    pub task_object: Setting<bool>,
    pub task_enable_coroutine: Setting<bool>,
    pub send_yield: Setting<bool>,
    pub enable_coroutine: Setting<bool>,
    pub single_thread: Setting<bool>,
    /// Cleared by whichever thread stops the server while the others read it
    /// ([`crate::ServerState::running`]).
    pub running: AtomicBool,
    /// The CPUs `cpu_affinity_ignore` leaves workers; empty for every CPU.
    pub cpu_affinity_available: Vec<c_int>,
    pub pipe_command: *mut c_void,
    pub send_timeout: Setting<f64>,
    pub heartbeat_check_interval: Setting<u16>,
    /// `time_t`, reset with `scheduler_warning` by whichever thread dispatches.
    pub warning_time: AtomicI64,
    pub timezone_: libc::c_long,
    /// Armed and deleted by the master (or a base-mode worker) after its threads
    /// start, so an atomic; only that thread uses the node.
    pub master_timer: AtomicPtr<c_void>,
    /// Armed by the heartbeat thread (or a single-threaded reactor), as `master_timer`.
    pub heartbeat_timer: AtomicPtr<c_void>,
    pub output_buffer_size: Setting<u32>,
    pub input_buffer_size: Setting<u32>,
    pub max_queued_bytes: Setting<u32>,
    /// Raised at start, by whichever factory starts; read through [`Self::ipc_max_size`].
    pub ipc_max_size: AtomicU32,
    pub private_data_1: *mut c_void,
    pub private_data_2: *mut c_void,
    /// The `dispatch_func` callable, swapped by the thread configuring the server.
    pub private_data_3: AtomicPtr<c_void>,
    pub private_data_4: *mut c_void,
    pub factory_: *mut c_void,
    pub manager_: *mut c_void,
    pub task_worker_num: Setting<u32>,
    pub task_ipc_mode: Setting<u8>,
    pub task_max_request: Setting<u32>,
    pub task_max_request_grace: Setting<u32>,
    pub task_results: Shm,
    /// The server's worker registry, which crates/process and the manager reach
    /// through `registry`'s pointer and write through `&`; native sees only a
    /// `void *` handle.
    pub worker_registry: Option<Box<WorkerRegistry>>,
    pub user_workers: Shm,
    pub workers: Shm,
    pub gs: Shm,
    pub upload_max_filesize: Setting<usize>,
    pub http_compression_level: Setting<u8>,
    pub compression_min_length: Setting<u32>,
    pub recv_buffer_allocator: AtomicPtr<c_void>,
    pub recv_buffer_size: usize,
    pub message_queue_key: Setting<u64>,
    /// `Server::Mode`, fixed by the constructor.
    pub mode_: u8,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            reactor_num: Setting::new(0),
            worker_num: Setting::new(0),
            dgram_port_num: 0,
            dispatch_mode: Setting::new(2),
            scheduler_warning: AtomicBool::new(false),
            max_request: Setting::new(0),
            max_request_grace: Setting::new(0),
            udp_socket_ipv4: core::ptr::null_mut(),
            udp_socket_ipv6: core::ptr::null_mut(),
            dgram_socket: core::ptr::null_mut(),
            max_wait_time: Setting::new(3),
            worker_max_concurrency: Setting::new(u32::MAX),
            daemonize: Setting::new(false),
            have_dgram_sock: false,
            have_stream_sock: false,
            open_cpu_affinity: Setting::new(false),
            disable_notify: false,
            discard_timeout_request: Setting::new(false),
            http_parse_cookie: Setting::new(true),
            http_parse_post: Setting::new(true),
            http_parse_files: Setting::new(false),
            http_compression: Setting::new(false),
            websocket_compression: Setting::new(false),
            enable_static_handler: Setting::new(false),
            http_autoindex: Setting::new(false),
            enable_unsafe_event: Setting::new(false),
            enable_delay_receive: Setting::new(false),
            enable_reuse_port: Setting::new(false),
            reload_async: Setting::new(true),
            event_object: Setting::new(false),
            task_object: Setting::new(false),
            task_enable_coroutine: Setting::new(false),
            send_yield: Setting::new(true),
            enable_coroutine: Setting::new(true),
            single_thread: Setting::new(false),
            running: AtomicBool::new(true),
            cpu_affinity_available: Vec::new(),
            pipe_command: core::ptr::null_mut(),
            send_timeout: Setting::new(0.0),
            heartbeat_check_interval: Setting::new(0),
            warning_time: AtomicI64::new(0),
            timezone_: 0,
            master_timer: AtomicPtr::new(core::ptr::null_mut()),
            heartbeat_timer: AtomicPtr::new(core::ptr::null_mut()),
            output_buffer_size: Setting::new(u32::MAX),
            input_buffer_size: Setting::new(2 * 1024 * 1024),
            max_queued_bytes: Setting::new(0),
            ipc_max_size: AtomicU32::new(if cfg!(target_vendor = "apple") {
                2048
            } else {
                8192
            }),
            private_data_1: core::ptr::null_mut(),
            private_data_2: core::ptr::null_mut(),
            private_data_3: AtomicPtr::default(),
            private_data_4: core::ptr::null_mut(),
            factory_: core::ptr::null_mut(),
            manager_: core::ptr::null_mut(),
            task_worker_num: Setting::new(0),
            task_ipc_mode: Setting::new(1),
            task_max_request: Setting::new(0),
            task_max_request_grace: Setting::new(0),
            task_results: Shm::NULL,
            worker_registry: None,
            user_workers: Shm::NULL,
            workers: Shm::NULL,
            gs: Shm::NULL,
            upload_max_filesize: Setting::new(0),
            http_compression_level: Setting::new(0),
            compression_min_length: Setting::new(20),
            recv_buffer_allocator: AtomicPtr::default(),
            recv_buffer_size: 65_536,
            message_queue_key: Setting::new(0),
            mode_: 0,
        }
    }
}

impl ServerConfig {
    /// The worker registry's address, or null before a state gives the config one.
    pub(crate) fn registry(&self) -> *mut WorkerRegistry {
        self.worker_registry
            .as_deref()
            .map_or(core::ptr::null_mut(), |registry| {
                core::ptr::from_ref(registry).cast_mut()
            })
    }

    /// `serv->ipc_max_size`.
    #[cfg(all(feature = "upstream-symbols", not(test)))]
    pub(crate) fn ipc_max_size(&self) -> u32 {
        self.ipc_max_size.load(Relaxed)
    }

    /// `Server::init_ipc_max_size()`.
    #[cfg(all(feature = "upstream-symbols", not(test)))]
    pub(crate) fn init_ipc_max_size(&self) {
        let size = swoole_rs_corebase::constants::SW_IPC_BUFFER_MAX_SIZE;
        self.ipc_max_size.store(size, Relaxed);
    }

    /// `serv->pipe_command`: the command pipe, once `OP_ST_PIPE_COMMAND_NEW` has made it.
    pub(crate) fn command_pipe(&self) -> Option<&swoole_rs_tls::socket_pair::SocketPair> {
        let pipe = self
            .pipe_command
            .cast::<swoole_rs_tls::socket_pair::SocketPair>();
        // SAFETY: null, or the pair that op boxed, which nothing frees while the
        // configuration lives.
        unsafe { pipe.as_ref() }
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
        self.reactor_num
            .set(u16::try_from(cpu_num.min(8)).unwrap_or(8));
        self.worker_num.set(cpu_num);
        self.timezone_ = timezone;
        self.http_compression.set(true);
        self.http_compression_level.set(1);
        self.compression_min_length.set(20);
        self.recv_buffer_allocator
            .store(allocator.cast_mut(), Relaxed);
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
    let config = state.config_ref();
    let tables = state.tables_ref();
    Some(match op {
        OP_MODE => u64::from(config.mode_),
        OP_SINGLE_THREAD => u64::from(config.single_thread.get()),
        OP_DELAY_RECEIVE => u64::from(config.enable_delay_receive.get()),
        OP_DISABLE_NOTIFY => u64::from(config.disable_notify),
        OP_RUNNING => u64::from(state.running()),
        OP_LC_DAEMONIZE => u64::from(config.daemonize.get()),
        OP_MAX_WAIT_TIME => u64::from(config.max_wait_time.get()),
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
                    // Only the field is written: no reference to a listed port's
                    // settings is formed.
                    let idle = config.heartbeat_check_interval.get().wrapping_mul(2);
                    port.set_heartbeat_idle_time(idle);
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
            let max = u32::try_from(arg & u64::from(u32::MAX)).unwrap_or_default();
            // SAFETY: `Server::create`, before any thread or process starts; `tables`
            // isn't read after the write.
            unsafe { &mut *state.tables() }.max_connection = max;
            0
        }
        OP_ACCEPT_TIMER_CLEAR => {
            tables
                .enable_accept_timer
                .store(core::ptr::null_mut(), Relaxed);
            0
        }
        OP_SET_RUNNING => {
            state.set_running(arg != 0);
            0
        }
        OP_SET_WARNING => {
            config.scheduler_warning.store(true, Relaxed);
            0
        }
        OP_FIELD => match u32::try_from(arg).unwrap_or(u32::MAX) {
            FIELD_REACTOR_NUM => u64::from(config.reactor_num.get()),
            FIELD_WORKER_NUM => u64::from(config.worker_num.get()),
            FIELD_TASK_WORKER_NUM => u64::from(config.task_worker_num.get()),
            FIELD_HEARTBEAT_INTERVAL => u64::from(config.heartbeat_check_interval.get()),
            FIELD_DISPATCH_MODE => u64::from(config.dispatch_mode.get()),
            FIELD_UNSAFE_EVENT => u64::from(config.enable_unsafe_event.get()),
            FIELD_TASK_IPC_MODE => u64::from(config.task_ipc_mode.get()),
            FIELD_CPU_NUM => u64::from(swoole_rs_corebase::swr_v1_core_cpu_num()),
            _ => 0,
        },
        OP_SET_FIELD => {
            // `Server::create`'s and `start`'s checks, before any thread or process
            // starts; each field is a cell, set through the shared configuration.
            let value = arg >> 8;
            match u32::try_from(arg & 0xff).unwrap_or(u32::MAX) {
                FIELD_REACTOR_NUM => {
                    config
                        .reactor_num
                        .set(u16::try_from(value & u64::from(u16::MAX)).unwrap_or_default());
                }
                FIELD_WORKER_NUM => {
                    config
                        .worker_num
                        .set(u32::try_from(value & u64::from(u32::MAX)).unwrap_or_default());
                }
                FIELD_TASK_WORKER_NUM => {
                    config
                        .task_worker_num
                        .set(u32::try_from(value & u64::from(u32::MAX)).unwrap_or_default());
                }
                FIELD_HEARTBEAT_INTERVAL => {
                    config
                        .heartbeat_check_interval
                        .set(u16::try_from(value & u64::from(u16::MAX)).unwrap_or_default());
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
    use crate::state::{free_state, swr_v1_server_state_new};

    #[test]
    fn runtime_defaults_and_native_views_share_owned_slots() {
        let state = swr_v1_server_state_new();
        unsafe {
            let config = (*state).config();
            (*config).init_defaults(16, 3600, core::ptr::null());
            assert_eq!((*config).worker_num.get(), 16);
            assert_eq!((*config).reactor_num.get(), 8);
            assert_eq!((*config).timezone_, 3600);
            assert_eq!(
                native_fact(&*state, OP_FIELD, u64::from(FIELD_WORKER_NUM)),
                Some(16)
            );
            let encoded = (19 << 8) | u64::from(FIELD_WORKER_NUM);
            assert_eq!(native_fact(&*state, OP_SET_FIELD, encoded), Some(0));
            assert_eq!((*config).worker_num.get(), 19);
            assert_eq!(native_fact(&*state, OP_SET_RUNNING, 0), Some(0));
            assert!(!(*state).running());
            assert_eq!(native_fact(&*state, OP_RUNNING, 0), Some(0));
            free_state(state);
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
            (*(*state).config()).heartbeat_check_interval.set(5);
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
            free_state(state);
            for port in ports {
                crate::port_config::swr_v1_port_config_free(port);
            }
        }
    }
}
