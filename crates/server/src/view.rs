//! A native `Server`'s Rust-owned records, read straight from its pointer by
//! the crates that bind it (phpserver), without a native call.
//!
//! A `Server *` is `object.rs`'s record, which starts `{ state, config, tables,
//! callbacks }`, and a `ListenPort *` is its box of the port's settings pointer
//! (`native/include/swoole_rs_server_view.h`). `ServerGS` is `shared::ServerShared`.
use crate::callbacks::CallbackSlot;
use crate::port_config::PortConfig;
use crate::{ConnectionRecord, PortCounters, ServerConfig, ServerCounters, ServerState};
use core::ffi::{c_int, c_long, c_void};
use core::marker::PhantomData;
use core::sync::atomic::Ordering;
use std::ffi::CString;
use std::sync::Arc;
use swoole_rs_corebase::worker_registry::WorkerRegistry;
use swoole_rs_process::process_pool::PoolRecord;
use swoole_rs_process::server_workers::{ServerWorkers, swr_v1_process_server_worker};
use swoole_rs_process::worker_record::{Worker, WorkerRecord};
use swoole_rs_reactor::Socket;
use swoole_rs_tls::socket_pair::SocketPair;

/// The head of every `Server`.
#[repr(C)]
struct Fields {
    state: *const ServerState,
    config: *mut ServerConfig,
}

/// A live `Server`, borrowed for `'a`. [`Server::from_raw`] is the one unsafe
/// step; the reads it then allows are safe, and only the methods that hand out
/// `&mut` to state other threads share, or take another raw pointer, stay unsafe.
/// It is laid out as the `Server *` it views, so a slot fn
/// ([`Server::set_callback`]) takes one where native code passes its server.
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct Server<'a> {
    raw: *mut c_void,
    live: PhantomData<&'a Fields>,
}

impl<'a> Server<'a> {
    /// The server at `serv`.
    ///
    /// # Safety
    /// `serv` is a live `Server` for `'a`, and the server's own synchronization
    /// keeps writers of its configuration and counters (such as
    /// [`Server::config_mut`]) apart from the references the safe readers return.
    #[must_use]
    pub unsafe fn from_raw(serv: *mut c_void) -> Self {
        Self {
            raw: serv,
            live: PhantomData,
        }
    }

    fn fields(self) -> &'a Fields {
        // SAFETY: the head of a live Server, per the module comment and `from_raw`'s caller.
        unsafe { &*self.raw.cast::<Fields>() }
    }

    /// `serv` itself, for the native-shaped calls that still take it.
    #[must_use]
    pub fn raw(self) -> *mut c_void {
        self.raw
    }

    /// The server's Rust-owned state.
    pub(crate) fn state(self) -> &'a ServerState {
        // SAFETY: the state lives as long as the server.
        unsafe { &*self.fields().state }
    }

    /// `serv->ports`, in listen order.
    #[must_use]
    pub fn ports(self) -> &'a [Port<'a>] {
        self.state().tables_ref().ports()
    }

    /// `serv->onX = f`: callback slot `index` (`callbacks::CB_*`) now calls `invoke`, a
    /// Rust `extern "C"` fn taking a null `ctx` and then the member's own arguments
    /// (its `Server *` may be a `Server`).
    ///
    /// # Safety
    /// No other thread reads the slot meanwhile, and `invoke` has the slot's signature.
    pub unsafe fn set_callback(self, index: u32, invoke: *const c_void) {
        if let Some(slot) = self.state().callbacks.slot(index) {
            slot.set(invoke);
        }
    }

    /// `serv->onX != nullptr` of callback slot `index`.
    #[must_use]
    pub fn has_callback(self, index: u32) -> bool {
        self.state()
            .callbacks
            .slot(index)
            .is_some_and(CallbackSlot::is_set)
    }

    /// The server's configuration.
    #[must_use]
    pub fn config(self) -> &'a ServerConfig {
        // SAFETY: the config allocation lives as long as the server, and writers
        // hold no reference across a read (`config_mut`).
        unsafe { &*self.fields().config }
    }

    /// The server's configuration, to write.
    ///
    /// # Safety
    /// No other reference to the configuration is live meanwhile, and no other
    /// thread reads the fields written (the server's own synchronization).
    #[must_use]
    pub unsafe fn config_mut(self) -> &'a mut ServerConfig {
        // SAFETY: per the caller; the allocation lives as long as the server.
        unsafe { &mut *self.fields().config }
    }

    /// `&serv->message_bus`, the server's own `MessageBus`.
    #[must_use]
    pub fn message_bus(self) -> *mut c_void {
        self.state().tables_ref().message_bus
    }

    /// `upload_tmp_dir = dir`.
    pub fn set_upload_tmp_dir(self, dir: &[u8]) {
        self.state().set_upload_tmp_dir(dir);
    }

    /// `serv-><text> = value` for text member `index`: 0 `chroot_`, 1 `user_`,
    /// 2 `group_`, 3 `pid_file` (`SWR_SERVER_TEXT_*`).
    ///
    /// # Safety
    /// The server is not started, so no other thread reads the text.
    pub unsafe fn set_text(self, index: usize, value: &[u8]) {
        let slot = self.state().text_slot(index);
        if slot.is_null() {
            return;
        }
        // SAFETY: a live slot, set only while the server is configured.
        let status = unsafe {
            swoole_rs_corebase::swr_v1_core_string_set(slot, value.as_ptr().cast(), value.len())
        };
        if status < 0 {
            // C++'s assignment threw `std::bad_alloc`, which nothing catches.
            std::process::abort();
        }
    }

    /// `insert(value)` into the static collection `kind`: 0 `locations`,
    /// 1 `http_index_files`, 2 `http_compression_types`.
    pub fn insert_static_string(self, kind: u32, value: &[u8]) {
        // SAFETY: the live state and a byte span (non-null even when empty).
        unsafe {
            crate::swr_v1_server_static_config(self.state(), kind, 4, value.as_ptr(), value.len())
        };
    }

    /// `url_rewrite_rules`: add one rule.
    pub fn add_rewrite_rule(self, pattern: &[u8], replacement: &[u8]) {
        crate::static_config::add_rewrite_rule(self.state(), pattern, replacement);
    }

    /// `f` over the static handler's collection `kind` (0 locations, 1 index files):
    /// whether it is set, and its values, borrowed under the collection's lock.
    pub fn static_strings<R>(self, kind: u32, f: impl FnOnce(bool, &[Arc<[u8]>]) -> R) -> R {
        crate::static_config::strings(self.state(), kind, f)
    }

    /// `apply_rewrite_rules` on a static context: true when a rule applied.
    pub fn static_rewrite(self, context: &mut swoole_rs_http::HttpStaticContext) -> bool {
        crate::static_config::apply_rewrite_rules(self.state(), context)
    }

    /// `document_root`.
    #[must_use]
    pub fn document_root(self) -> &'a [u8] {
        self.state().text(crate::state::TEXT_DOCUMENT_ROOT)
    }

    /// `upload_tmp_dir.c_str()`: the directory up to its first NUL.
    #[must_use]
    pub fn upload_tmp_dir(self) -> CString {
        let dir = self.state().upload_tmp_dir();
        let dir = dir.split(|&byte| byte == 0).next().unwrap_or_default();
        CString::new(dir).unwrap_or_default()
    }

    /// `http_compression_types` lists `kind`; `None` asks whether it lists any.
    #[must_use]
    pub fn compression_type(self, kind: Option<&[u8]>) -> bool {
        let state = self.state();
        let op = |operation, data: &[u8]| {
            // SAFETY: the live state and a byte span (non-null even when empty).
            unsafe {
                crate::swr_v1_server_static_config(state, 2, operation, data.as_ptr(), data.len())
            }
        };
        let types = op(1, &[]) != 0;
        match kind {
            Some(kind) if types => op(5, kind) != op(0, &[]),
            _ => types,
        }
    }

    /// The shared counters (`serv->gs`).
    #[must_use]
    pub fn counters(self) -> &'a ServerCounters {
        let gs = self.config().gs.as_ptr().cast::<ServerCounters>();
        // SAFETY: `swr_v1_server_new` maps `ServerGS`, which starts with the
        // counters, for the server's life.
        unsafe { &*gs }
    }

    /// The shared counters, for the settings `Server::set()` writes before the
    /// server starts.
    ///
    /// # Safety
    /// No concurrent reader of the fields written, and no other reference to them.
    #[must_use]
    pub unsafe fn counters_mut(self) -> &'a mut ServerCounters {
        let gs = self.config().gs.as_ptr().cast::<ServerCounters>();
        // SAFETY: as `counters`, with the caller's exclusion.
        unsafe { &mut *gs }
    }

    /// `Server::is_started()`.
    #[must_use]
    pub fn is_started(self) -> bool {
        self.counters().start.load(Ordering::Relaxed) != 0
    }

    /// `Server::is_single_worker()`: one event worker, which the master runs itself.
    #[must_use]
    pub fn is_single_worker(self) -> bool {
        let config = self.config();
        config.worker_num == 1
            && config.task_worker_num == 0
            && config.max_request == 0
            && self.user_worker_num() == 0
    }

    /// A single base-mode worker's server that this process ran and never
    /// destroyed: worker 0 records its pid as the master's, and `destroy` drops
    /// the factory. Its event worker pool is this process's alone.
    #[must_use]
    pub fn ran_its_single_worker_here(self) -> bool {
        let config = self.config();
        u64::from(config.mode_) == crate::MODE_BASE
            && self.is_single_worker()
            && self.counters().master_pid == std::process::id().cast_signed()
            && !config.factory_.is_null()
    }

    /// `Server::is_worker_thread()`: thread mode, on a worker thread.
    #[must_use]
    pub fn is_worker_thread(self) -> bool {
        /// `Server::MODE_THREAD` and `THREAD_WORKER`.
        const MODE_THREAD: u8 = 3;
        const THREAD_WORKER: u8 = 4;
        self.config().mode_ == MODE_THREAD && swoole_rs_corebase::thread_kind() == THREAD_WORKER
    }

    /// `Server::get_connection_num()`: the per-worker cells when there are any.
    #[must_use]
    pub fn connection_num(self) -> usize {
        let gs = self.counters();
        if gs.cells().is_null() {
            return gs.connection_num.load(Ordering::Relaxed) as usize;
        }
        let workers = self.config().worker_num as usize;
        (0..workers)
            .map(|i| {
                let cell = gs.cells().wrapping_add(i);
                // SAFETY: one cell per event worker, allocated with the counters.
                let cell = unsafe { &*cell };
                cell.load(Ordering::Relaxed) as usize
            })
            .fold(0, usize::wrapping_add)
    }

    /// `Server::get_connection_verify(session_id)`.
    #[must_use]
    pub fn verified(self, session_id: i64) -> Option<&'a mut ConnectionRecord> {
        self.state().connection(session_id, 2)
    }

    /// `Server::get_connection_by_session_id(session_id)`: the session's slot,
    /// unverified.
    #[must_use]
    pub fn by_session(self, session_id: i64) -> Option<&'a mut ConnectionRecord> {
        self.state().connection(session_id, 1)
    }

    /// `Server::get_connection_for_iterator(fd)`.
    #[must_use]
    pub fn iterated(self, fd: c_int) -> Option<&'a mut ConnectionRecord> {
        self.state().connection(c_long::from(fd), 4)
    }

    /// `Server::get_connection(fd)`.
    #[must_use]
    pub fn at(self, fd: c_int) -> Option<&'a mut ConnectionRecord> {
        self.state().connection(c_long::from(fd), 0)
    }

    /// `field` of the connection at `key`; `None` for no slot.
    fn connection_field<T>(
        self,
        key: c_long,
        kind: u32,
        field: impl FnOnce(&ConnectionRecord) -> T,
    ) -> Option<T> {
        self.state().connection(key, kind).map(|conn| field(conn))
    }

    /// `ListenPort::get_connection_num()` of one of the server's ports.
    #[must_use]
    pub fn port_connection_num(self, port: Port<'_>) -> usize {
        self.port_connections(port.config())
    }

    /// `ListenPort::get_connection_num()` of the port whose settings are `config`.
    pub(crate) fn port_connections(self, config: &PortConfig) -> usize {
        let gs = config.gs;
        // SAFETY: the port's mapped counters.
        let cells = unsafe { &*gs.cast::<PortCounters>() }.cells();
        let workers = if cells.is_null() {
            0
        } else {
            self.config().worker_num
        };
        // SAFETY: the port's live counters, with one cell per event worker.
        unsafe { crate::swr_v1_server_port_connections(gs, workers) }
    }

    /// `Server::get_max_connection()`.
    #[must_use]
    pub fn max_connection(self) -> u32 {
        self.state().tables_ref().max_connection
    }

    /// `serv->get_task_worker_pool()`.
    #[must_use]
    pub fn task_pool(self) -> *mut PoolRecord {
        let gs = self.config().gs.as_ptr().cast::<u8>();
        gs.wrapping_add(core::mem::offset_of!(crate::shared::ServerShared, pools))
            .cast()
    }

    /// The server's worker registry, which `ServerState` boxed for the server's life.
    #[must_use]
    pub fn registry(self) -> &'a WorkerRegistry {
        let registry = self.config().worker_registry.cast::<WorkerRegistry>();
        // SAFETY: the server's live registry, whose writers the server's own
        // synchronization keeps apart from readers (`registry_mut`).
        unsafe { &*registry }
    }

    /// The server's worker registry, to write.
    ///
    /// # Safety
    /// No other reference to the registry is live meanwhile, and no other thread
    /// reads it (only the master configures it, and only the manager tracks pids).
    #[must_use]
    pub unsafe fn registry_mut(self) -> &'a mut WorkerRegistry {
        let registry = self.config().worker_registry.cast::<WorkerRegistry>();
        // SAFETY: per the caller; the registry lives as long as the server.
        unsafe { &mut *registry }
    }

    /// `Server::get_user_worker_num()`: the user workers added so far.
    #[must_use]
    pub fn user_worker_num(self) -> usize {
        self.registry().pending().len()
    }

    /// The server's worker arrays and counts.
    #[must_use]
    pub fn workers(self) -> ServerWorkers {
        let task_pool = self.task_pool();
        let config = self.config();
        ServerWorkers {
            event_pool: task_pool.wrapping_add(1),
            task_pool,
            event_workers: config.workers.as_ptr().cast(),
            user_workers: config.user_workers.as_ptr().cast(),
            event_count: config.worker_num,
            task_count: config.task_worker_num,
            user_count: self.user_worker_num(),
        }
    }

    /// `Server::get_worker(id)`: event workers, then task workers, then user
    /// workers; null past the last.
    #[must_use]
    pub fn worker(self, id: u16) -> *mut WorkerRecord {
        let view = self.workers();
        // SAFETY: the server's live pools and worker arrays, with their counts.
        unsafe { swr_v1_process_server_worker(&view, id) }
    }

    /// [`Server::worker`]'s record, or `None` past the last.
    #[must_use]
    pub fn worker_record(self, id: u16) -> Option<Worker<'a>> {
        // SAFETY: null or one of the server's live workers, in memory the server
        // keeps for `'a`.
        unsafe { Worker::from_raw(self.worker(id)) }
    }

    /// Worker `id`'s pipe socket, its master end or its worker end, or `None` while it has none.
    #[must_use]
    pub(crate) fn worker_pipe(self, id: u16, master: bool) -> Option<Socket<'a>> {
        let worker = self.worker_record(id)?;
        let pipe = if master {
            worker.pipe_master()
        } else {
            worker.pipe_worker()
        };
        // SAFETY: null or a pipe socket of one of the server's live workers, which the
        // server keeps until it frees its workers.
        unsafe { Socket::from_ptr(pipe.cast()) }
    }

    /// The listening socket of `serv->ports[index]`, or `None` past the last.
    #[must_use]
    pub(crate) fn port_socket_at(self, index: usize) -> Option<Socket<'a>> {
        self.port_at(index)?.socket()
    }

    /// `Server::task_notify_pipe(worker_id)`, or null.
    #[must_use]
    pub fn task_notify_pipe(self, worker_id: u16) -> *mut SocketPair {
        self.state().task_pipe(worker_id.into())
    }

    /// `Server::get_server_socket(fd)`: the listening socket at `fd`, or null.
    #[must_use]
    pub fn server_socket(self, fd: c_long) -> *mut c_void {
        self.connection_field(fd, 0, |conn| conn.socket)
            .unwrap_or(core::ptr::null_mut())
    }

    /// `Server::get_port_by_fd(fd)`: the `ListenPort` a connection arrived on.
    #[must_use]
    pub fn port_by_fd(self, fd: c_int) -> Option<Port<'a>> {
        let server_fd = self.connection_field(c_long::from(fd), 0, |conn| conn.server_fd)?;
        self.port_by_server_fd(server_fd)
    }

    /// `Server::get_port_by_server_fd(server_fd)`: the `ListenPort` listening on
    /// `server_fd`.
    #[must_use]
    pub fn port_by_server_fd(self, server_fd: c_int) -> Option<Port<'a>> {
        // A listening socket's record holds its port.
        let port = self.connection_field(c_long::from(server_fd), 0, |conn| conn.object)?;
        // SAFETY: null, or one of the server's ports, which live as long as it.
        unsafe { Port::from_raw(port) }
    }

    /// The settings of the `ListenPort` listening on `server_fd`, if there is one.
    #[must_use]
    pub fn port_of_server_fd(self, server_fd: c_int) -> Option<&'a PortConfig> {
        self.port_by_server_fd(server_fd).map(Port::config)
    }

    /// The settings of the `ListenPort` connection `fd` arrived on, if there is one.
    #[must_use]
    pub fn port_of_fd(self, fd: c_int) -> Option<&'a PortConfig> {
        self.port_by_fd(fd).map(Port::config)
    }

    /// `Server::is_healthy_connection(now, conn)`: protected, never received, or
    /// received within its port's heartbeat idle time.
    #[must_use]
    pub fn healthy(self, now: f64, conn: &ConnectionRecord) -> bool {
        if conn.protect != 0 || conn.last_recv_time == 0.0 {
            return true;
        }
        // `get_port_by_session_id`.
        let Some(fd) = self.connection_field(conn.session_id, 1, |conn| conn.fd) else {
            return true;
        };
        let Some(port) = self.port_of_fd(fd) else {
            return true;
        };
        let idle = port.heartbeat_idle_time;
        idle == 0 || conn.last_recv_time > now - f64::from(idle)
    }

    /// `serv->ports[index]`: the `ListenPort`s in listen order, `None` past the last
    /// (index 0 is `Server::get_primary_port()`).
    #[must_use]
    pub fn port_at(self, index: usize) -> Option<Port<'a>> {
        self.ports().get(index).copied()
    }

    /// `Server::get_port(port)`: the first port listening on `port` (any at 0).
    #[must_use]
    pub fn port_number(self, port: c_int) -> Option<Port<'a>> {
        self.find_port(|config| port == 0 || config.port == port)
    }

    /// The first of `serv->ports` whose settings `wanted` accepts.
    pub fn find_port(self, wanted: impl Fn(&PortConfig) -> bool) -> Option<Port<'a>> {
        self.ports()
            .iter()
            .copied()
            .find(|port| wanted(port.config()))
    }
}

/// A live `ListenPort`, borrowed for `'a`: a box of the pointer to its settings
/// (`server_object::port_new`). [`Port::from_raw`] is the one unsafe step; the
/// reads it then allows are safe, as [`Server`]'s are.
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct Port<'a> {
    raw: core::ptr::NonNull<c_void>,
    live: PhantomData<&'a PortConfig>,
}

impl<'a> Port<'a> {
    /// The port at `port`, or `None` for null.
    ///
    /// # Safety
    /// `port` is null or a live `ListenPort` for `'a`, and the port's own
    /// synchronization keeps writers of its settings ([`Port::config_mut`]) apart
    /// from the references the safe readers return.
    #[must_use]
    pub unsafe fn from_raw(port: *mut c_void) -> Option<Self> {
        core::ptr::NonNull::new(port).map(|raw| Self {
            raw,
            live: PhantomData,
        })
    }

    /// The `ListenPort *` itself, for the native-shaped calls that still take it.
    #[must_use]
    pub fn raw(self) -> *mut c_void {
        self.raw.as_ptr()
    }

    /// The port's settings.
    #[must_use]
    pub fn config(self) -> &'a PortConfig {
        // SAFETY: the head of a live port, per the module comment and `from_raw`'s caller.
        let config = unsafe { *self.raw.as_ptr().cast::<*const PortConfig>() };
        // SAFETY: its settings live as long as it, with writers kept apart (`from_raw`).
        unsafe { &*config }
    }

    /// The port's settings, to write.
    ///
    /// # Safety
    /// No other reference to the settings is live meanwhile, and no other thread
    /// reads the fields written.
    #[must_use]
    pub unsafe fn config_mut(self) -> &'a mut PortConfig {
        // SAFETY: per the caller.
        unsafe { crate::port_config::of(self.raw()) }
    }

    /// The port's listening socket, or `None` before it has one.
    #[must_use]
    pub fn socket(self) -> Option<Socket<'a>> {
        // SAFETY: null, or the live port's listening socket, which it owns.
        unsafe { Socket::from_ptr(self.config().socket.cast()) }
    }

    /// `ListenPort::get_fd()`.
    #[must_use]
    pub fn fd(self) -> c_int {
        self.socket().map_or(-1, Socket::fd)
    }
}

/// A `network::Socket`'s fd and `swSocketType`.
///
/// # Safety
/// `socket` is a live socket.
pub unsafe fn socket_fd(socket: *mut c_void) -> (c_int, c_int) {
    // SAFETY: per the caller.
    let socket = unsafe { Socket::from_ptr(socket.cast()) };
    socket.map_or((-1, 0), |socket| socket.read(|s| (s.fd, s.socket_type)))
}

/// The connection socket's `recv_buffer` (a `swoole::String *`), or null.
///
/// # Safety
/// `conn` is a live connection whose `socket` is null or live.
pub unsafe fn recv_buffer(conn: &ConnectionRecord) -> *mut c_void {
    // SAFETY: per the caller.
    let socket = unsafe { Socket::from_ptr(conn.socket.cast()) };
    socket.map_or(core::ptr::null_mut(), |socket| {
        socket.read(|s| s.recv_buffer)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::ffi::c_void;

    /// Only a single base-mode worker's own, undestroyed server has an event pool
    /// MSHUTDOWN may destroy: not another process's, a destroyed one, a process-mode
    /// one, or one with a manager or a task worker. Unit tests link no
    /// `swr_v1_server_new`, so the test lends the server its shared record.
    #[test]
    fn only_a_single_base_worker_that_ran_here_and_was_not_destroyed_counts() {
        let state = crate::swr_v1_server_state_new();
        let head = [state.cast::<c_void>(), unsafe { (*state).config() }.cast()];
        let serv = unsafe { Server::from_raw(head.as_ptr().cast_mut().cast()) };
        let config = unsafe { serv.config_mut() };
        config.gs =
            swoole_rs_memory::members::Shm::calloc(1, size_of::<crate::shared::ServerShared>());
        config.mode_ = 1;
        config.worker_num = 1;
        let mut factory = 0u8;
        config.factory_ = (&raw mut factory).cast();
        let own = std::process::id().cast_signed();
        unsafe { serv.counters_mut() }.master_pid = own;
        assert!(serv.ran_its_single_worker_here());
        unsafe { serv.counters_mut() }.master_pid = own + 1;
        assert!(!serv.ran_its_single_worker_here(), "another process's");
        unsafe { serv.counters_mut() }.master_pid = own;
        let config = unsafe { serv.config_mut() };
        config.factory_ = core::ptr::null_mut();
        assert!(!serv.ran_its_single_worker_here(), "destroyed");
        config.factory_ = (&raw mut factory).cast();
        config.mode_ = 2;
        assert!(!serv.ran_its_single_worker_here(), "process mode");
        config.mode_ = 1;
        config.worker_num = 2;
        assert!(
            !serv.ran_its_single_worker_here(),
            "a manager and two workers"
        );
        config.worker_num = 1;
        config.task_worker_num = 1;
        assert!(!serv.ran_its_single_worker_here(), "a task worker");
        config.task_worker_num = 0;
        config.factory_ = core::ptr::null_mut();
        config.gs.free();
        unsafe { crate::swr_v1_server_state_free(state) };
    }
}
