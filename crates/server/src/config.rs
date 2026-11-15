//! Rust-owned configuration and native-object loans for one Server.
//!
//! C++ retains borrowed reference views for the original embedding API. The
//! configuration allocation and defaults belong to `ServerState`; Rust server
//! operations read these records directly, without per-field native callbacks.
use core::ffi::{c_int, c_void};
use core::sync::atomic::{
    AtomicBool, AtomicI32, AtomicI64, AtomicPtr, AtomicU8, AtomicU16, AtomicU32, AtomicU64,
    AtomicUsize,
    Ordering::{Acquire, Relaxed, Release},
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
    i32 => AtomicI32;
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

/// A port field no atomic holds (a struct native code reads by reference, or an
/// owner), written through `&` while the port is configured. Reads copy and
/// writes replace, so no reference into it outlives a call; the port's own
/// synchronization ([`crate::view::Port::from_raw`]) keeps writers apart from
/// readers, as for every other setting. It is laid out as the value it holds.
#[repr(transparent)]
pub struct Shared<T>(core::cell::UnsafeCell<T>);

impl<T> Shared<T> {
    pub const fn new(value: T) -> Self {
        Self(core::cell::UnsafeCell::new(value))
    }

    #[inline]
    pub fn get(&self) -> T
    where
        T: Copy,
    {
        // SAFETY: no reference into the cell escapes a call, so none is live.
        unsafe { *self.0.get() }
    }

    /// The value before `value`, which the caller drops once the cell holds `value`.
    #[must_use]
    pub fn replace(&self, value: T) -> T {
        // SAFETY: no reference into the cell escapes a call, so none is live.
        core::mem::replace(unsafe { &mut *self.0.get() }, value)
    }

    pub fn set(&self, value: T) {
        drop(self.replace(value));
    }

    /// `f` over a copy, which then replaces the value.
    pub fn update<R>(&self, f: impl FnOnce(&mut T) -> R) -> R
    where
        T: Copy,
    {
        let mut value = self.get();
        let result = f(&mut value);
        self.set(value);
        result
    }

    pub fn get_mut(&mut self) -> &mut T {
        self.0.get_mut()
    }

    /// The value's address, which native code reads and writes by reference.
    pub const fn as_ptr(&self) -> *mut T {
        self.0.get()
    }
}

/// A value set while the server or a port is configured and read by every thread
/// once it runs. A reader takes its own reference under the lock and a write swaps
/// it, so neither sees a value the other frees. A thin box: native code's mirror
/// holds a `void *` here.
#[repr(transparent)]
pub struct Published<T>(Box<std::sync::Mutex<Option<std::sync::Arc<T>>>>);

impl<T> Default for Published<T> {
    fn default() -> Self {
        Self(Box::default())
    }
}

impl<T> Published<T> {
    #[must_use]
    pub fn get(&self) -> Option<std::sync::Arc<T>> {
        self.slot().clone()
    }

    /// Publishes `value`, or none, and hands back the one it replaces, for the
    /// caller to drop once the lock is released.
    #[must_use]
    pub fn replace(&self, value: Option<T>) -> Option<std::sync::Arc<T>> {
        core::mem::replace(&mut *self.slot(), value.map(std::sync::Arc::new))
    }

    /// Publishes `value`; the one it replaces drops once the lock is released.
    pub fn set(&self, value: T) {
        drop(self.replace(Some(value)));
    }

    fn slot(&self) -> std::sync::MutexGuard<'_, Option<std::sync::Arc<T>>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The independently configurable server fields and borrowed native handles.
#[repr(C)]
pub struct ServerConfig {
    pub reactor_num: Setting<u16>,
    pub worker_num: Setting<u32>,
    pub dgram_port_num: Setting<u8>,
    pub dispatch_mode: Setting<u8>,
    /// Set and cleared by whichever thread dispatches, so an atomic.
    pub scheduler_warning: AtomicBool,
    pub max_request: Setting<u32>,
    pub max_request_grace: Setting<u32>,
    /// Upstream's last UDP, UDP6 and unix datagram listeners, kept null for the
    /// record's layout: [`crate::view::Server::dgram_socket`] finds them among the ports.
    pub udp_socket_ipv4: *mut c_void,
    pub udp_socket_ipv6: *mut c_void,
    pub dgram_socket: *mut c_void,
    pub max_wait_time: Setting<u32>,
    pub worker_max_concurrency: Setting<u32>,
    pub daemonize: Setting<bool>,
    pub have_dgram_sock: Setting<bool>,
    pub have_stream_sock: Setting<bool>,
    pub open_cpu_affinity: Setting<bool>,
    pub disable_notify: Setting<bool>,
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
    /// The CPUs `cpu_affinity_ignore` leaves workers; none for every CPU.
    pub cpu_affinity_available: Published<Vec<c_int>>,
    pub pipe_command: CommandPipe,
    pub send_timeout: Setting<f64>,
    pub heartbeat_check_interval: Setting<u16>,
    /// `time_t`, reset with `scheduler_warning` by whichever thread dispatches.
    pub warning_time: AtomicI64,
    pub timezone_: libc::c_long,
    /// Armed and deleted by the master (or a base-mode worker) after its threads
    /// start, so an atomic; only that thread uses the node.
    pub master_timer: swoole_rs_corebase::timer_key::AtomicHeld,
    /// Armed by the heartbeat thread (or a single-threaded reactor), as `master_timer`.
    pub heartbeat_timer: swoole_rs_corebase::timer_key::AtomicHeld,
    pub output_buffer_size: Setting<u32>,
    pub input_buffer_size: Setting<u32>,
    pub max_queued_bytes: Setting<u32>,
    /// Raised at start, by whichever factory starts; read through [`Self::ipc_max_size`].
    pub ipc_max_size: AtomicU32,
    pub private_data_1: *mut c_void,
    pub private_data_2: *mut c_void,
    /// The `dispatch_func` callable's owner, which phpserver sets and downcasts.
    pub private_data_3: Published<Box<dyn core::any::Any>>,
    pub private_data_4: *mut c_void,
    pub factory_: *mut c_void,
    pub manager_: Shared<*mut c_void>,
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
    pub mode_: Mode,
}

/// `Server::Mode`: where connections and workers run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Mode {
    Base = 1,
    Process = 2,
    Thread = 3,
}

impl Mode {
    /// The mode of a `Server::Mode` code.
    #[must_use]
    pub const fn from_code(code: i64) -> Option<Self> {
        match code {
            1 => Some(Self::Base),
            2 => Some(Self::Process),
            3 => Some(Self::Thread),
            _ => None,
        }
    }
}

/// `Server::DispatchMode`: how a connection's data picks its worker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DispatchMode {
    Round = 1,
    FdMod = 2,
    IdleWorker = 3,
    IpMod = 4,
    UidMod = 5,
    UserFunc = 6,
    Stream = 7,
    CoConnLb = 8,
    CoReqLb = 9,
    ConcurrentLb = 10,
}

impl DispatchMode {
    /// The mode of a `dispatch_mode` setting.
    #[must_use]
    pub const fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            1 => Self::Round,
            2 => Self::FdMod,
            3 => Self::IdleWorker,
            4 => Self::IpMod,
            5 => Self::UidMod,
            6 => Self::UserFunc,
            7 => Self::Stream,
            8 => Self::CoConnLb,
            9 => Self::CoReqLb,
            10 => Self::ConcurrentLb,
            _ => return None,
        })
    }

    /// `Server::is_hash_dispatch_mode()`: the mode binds a connection to one worker.
    #[must_use]
    pub const fn is_hash(self) -> bool {
        matches!(self, Self::FdMod | Self::IpMod | Self::CoConnLb)
    }
}

/// `Server::TaskIpcMode`: how task workers receive their tasks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum TaskIpcMode {
    UnixSock = 1,
    MsgQueue = 2,
    Preemptive = 3,
    Stream = 4,
}

impl TaskIpcMode {
    /// The mode of a `task_ipc_mode` setting.
    #[must_use]
    pub const fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            1 => Self::UnixSock,
            2 => Self::MsgQueue,
            3 => Self::Preemptive,
            4 => Self::Stream,
            _ => return None,
        })
    }
}

impl ServerConfig {
    /// The `dispatch_mode` setting, `None` for a code no mode has.
    #[must_use]
    pub fn dispatch(&self) -> Option<DispatchMode> {
        DispatchMode::from_code(self.dispatch_mode.get())
    }

    /// Whether the dispatch mode binds a connection to one worker.
    #[must_use]
    pub fn hash_dispatch(&self) -> bool {
        self.dispatch().is_some_and(DispatchMode::is_hash)
    }

    /// The `task_ipc_mode` setting, `None` for a code no mode has.
    #[must_use]
    pub fn task_ipc(&self) -> Option<TaskIpcMode> {
        TaskIpcMode::from_code(self.task_ipc_mode.get())
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            reactor_num: Setting::new(0),
            worker_num: Setting::new(0),
            dgram_port_num: Setting::new(0),
            dispatch_mode: Setting::new(DispatchMode::FdMod as u8),
            scheduler_warning: AtomicBool::new(false),
            max_request: Setting::new(0),
            max_request_grace: Setting::new(0),
            udp_socket_ipv4: core::ptr::null_mut(),
            udp_socket_ipv6: core::ptr::null_mut(),
            dgram_socket: core::ptr::null_mut(),
            max_wait_time: Setting::new(3),
            worker_max_concurrency: Setting::new(u32::MAX),
            daemonize: Setting::new(false),
            have_dgram_sock: Setting::new(false),
            have_stream_sock: Setting::new(false),
            open_cpu_affinity: Setting::new(false),
            disable_notify: Setting::new(false),
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
            cpu_affinity_available: Published::default(),
            pipe_command: CommandPipe(AtomicPtr::new(core::ptr::null_mut())),
            send_timeout: Setting::new(0.0),
            heartbeat_check_interval: Setting::new(0),
            warning_time: AtomicI64::new(0),
            timezone_: 0,
            master_timer: swoole_rs_corebase::timer_key::AtomicHeld::new(),
            heartbeat_timer: swoole_rs_corebase::timer_key::AtomicHeld::new(),
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
            private_data_3: Published::default(),
            private_data_4: core::ptr::null_mut(),
            factory_: core::ptr::null_mut(),
            manager_: Shared::new(core::ptr::null_mut()),
            task_worker_num: Setting::new(0),
            task_ipc_mode: Setting::new(TaskIpcMode::UnixSock as u8),
            task_max_request: Setting::new(0),
            task_max_request_grace: Setting::new(0),
            task_results: Shm::empty(),
            worker_registry: None,
            user_workers: Shm::empty(),
            workers: Shm::empty(),
            gs: Shm::empty(),
            upload_max_filesize: Setting::new(0),
            http_compression_level: Setting::new(0),
            compression_min_length: Setting::new(20),
            recv_buffer_allocator: AtomicPtr::default(),
            recv_buffer_size: 65_536,
            message_queue_key: Setting::new(0),
            mode_: Mode::Base,
        }
    }
}

impl ServerConfig {
    /// `serv->ipc_max_size`.
    #[cfg(all(feature = "upstream-symbols", not(test)))]
    pub(crate) fn ipc_max_size(&self) -> u32 {
        self.ipc_max_size.load(Relaxed)
    }

    /// `Server::init_ipc_max_size()`.
    pub(crate) fn init_ipc_max_size(&self) {
        let size = swoole_rs_corebase::constants::SW_IPC_BUFFER_MAX_SIZE;
        self.ipc_max_size.store(size, Relaxed);
    }

    /// `serv->pipe_command`: the command pipe, once `add_command` has made it.
    pub(crate) fn command_pipe(&self) -> Option<&swoole_rs_tls::socket_pair::SocketPair> {
        let pipe = self.pipe_command.0.load(Acquire);
        // SAFETY: null, or a pair `CommandPipe::keep` leaked, which nothing frees:
        // only `keep` writes the private field.
        unsafe { pipe.as_ref() }
    }
}

/// `serv->pipe_command`: null, or the command pipe `add_command` opened, which the
/// server keeps for the rest of the process. Laid out as the pointer.
#[repr(transparent)]
pub struct CommandPipe(AtomicPtr<swoole_rs_tls::socket_pair::SocketPair>);

impl CommandPipe {
    /// `serv->pipe_command = pipe`: the server keeps it, and a pipe it kept before.
    pub(crate) fn keep(&self, pipe: Box<swoole_rs_tls::socket_pair::SocketPair>) {
        self.0.store(Box::into_raw(pipe), Release);
    }

    /// The pipe's address, or null.
    #[must_use]
    pub fn as_ptr(&self) -> *mut c_void {
        self.0.load(Acquire).cast()
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

#[cfg(test)]
mod tests {
    use crate::state::{free_state, swr_v1_server_state_new};

    #[test]
    fn runtime_defaults() {
        let state = swr_v1_server_state_new();
        unsafe {
            let config = (*state).config();
            (*config).init_defaults(16, 3600, core::ptr::null());
            assert_eq!((*config).worker_num.get(), 16);
            assert_eq!((*config).reactor_num.get(), 8);
            assert_eq!((*config).timezone_, 3600);
            (*state).set_running(false);
            assert!(!(*state).running());
            free_state(state);
        }
    }

    #[test]
    fn pushed_ports_are_numbered_in_order() {
        let state = swr_v1_server_state_new();
        let ports: Vec<_> = [1, 2]
            .into_iter()
            .map(|_| crate::port_config::swr_v1_port_config_new(0))
            .collect();
        unsafe {
            assert!((*state).tables_ref().ports().is_empty());
            for &port in &ports {
                let _ = (*state).push_port(Box::from_raw(port));
            }
            assert_eq!((*state).tables_ref().ports().len(), 2);
            assert_eq!((*ports[1]).object_id, 2);
            free_state(state);
            for port in ports {
                drop(Box::from_raw(port));
            }
        }
    }
}
