//! A native `Server`'s Rust-owned records, read straight from its pointer by
//! the crates that bind it (phpserver), without a native call.
//!
//! A `Server *` is `object.rs`'s record, which starts `{ state, config, tables,
//! callbacks }`, and a `ListenPort *` is the box of the port's settings.
//! `ServerGS` is `shared::ServerShared`.
use crate::Mode;
use crate::port_config::PortConfig;
use crate::records::Lookup;
use crate::{Connection, ServerConfig, ServerCounters, ServerState};
use core::ffi::{c_int, c_long, c_void};
use core::marker::PhantomData;
use core::sync::atomic::{AtomicU32, Ordering};
use std::ffi::CString;
use std::sync::Arc;
use swoole_rs_corebase::worker_registry::WorkerRegistry;
use swoole_rs_process::process_pool::{PoolRecord, PoolRef};
use swoole_rs_process::server_workers::{ServerWorkers, swr_v1_process_server_worker};
use swoole_rs_process::worker_record::{Worker, WorkerRecord};
use swoole_rs_reactor::Socket;
use swoole_rs_tls::socket_pair::SocketPair;

/// The head of every `Server`.
#[repr(C)]
pub(crate) struct Fields {
    state: *const ServerState,
    config: *mut ServerConfig,
}

#[cfg(test)]
impl Fields {
    /// A head over `state`, as a server's own: a test's server.
    pub(crate) fn of(state: &ServerState) -> Self {
        Self {
            state,
            config: state.config(),
        }
    }
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

/// The global hooks whose callbacks take the server alone.
#[derive(Clone, Copy)]
pub enum ServerHook {
    BeforeStart,
    BeforeShutdown,
    AfterShutdown,
    BeforeCreate,
    AfterCreate,
}

impl<'a> Server<'a> {
    /// `if (swoole_isset_hook(hook)) swoole_call_hook(hook, serv)`.
    pub fn call_hook(self, hook: ServerHook) {
        use swoole_rs_corebase::HookType;
        let kind = match hook {
            ServerHook::BeforeStart => HookType::BEFORE_SERVER_START,
            ServerHook::BeforeShutdown => HookType::BEFORE_SERVER_SHUTDOWN,
            ServerHook::AfterShutdown => HookType::AFTER_SERVER_SHUTDOWN,
            ServerHook::BeforeCreate => HookType::BEFORE_SERVER_CREATE,
            ServerHook::AfterCreate => HookType::AFTER_SERVER_CREATE,
        };
        if swoole_rs_corebase::swoole_isset_hook(kind) {
            // SAFETY: every `ServerHook`'s callbacks take the server, which is live.
            unsafe { swoole_rs_corebase::swoole_call_hook(kind, self.raw) };
        }
    }

    /// The server at `serv`.
    ///
    /// # Safety
    /// `serv` is a live `Server` for `'a`, and the server's own synchronization
    /// keeps writers of its configuration and counters apart from the references
    /// the safe readers return.
    #[must_use]
    pub unsafe fn from_raw(serv: *mut c_void) -> Self {
        Self {
            raw: serv,
            live: PhantomData,
        }
    }

    /// `sw_server()`: the process's server (`SwooleG.server`), or `None` while it has
    /// none.
    ///
    /// # Safety
    /// The server is not deleted during `'a`: the last `SharedServer` drop frees it before
    /// it clears `SwooleG.server`, so a view kept past that dangles.
    #[must_use]
    pub unsafe fn current() -> Option<Self> {
        let serv = swoole_rs_corebase::swr_v1_core_server();
        // SAFETY: null, or the process's server (only `swr_v1_server_new` sets it),
        // live for `'a` per the caller.
        (!serv.is_null()).then(|| unsafe { Server::from_raw(serv) })
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

    /// `serv->onX = f`: the slot `callback` names now calls its fn, a Rust
    /// `extern "C"` fn taking a `ctx` it ignores and then the member's own
    /// arguments (its `Server *` is a `Server`). Only the slot's fn is replaced,
    /// atomically, and nothing is freed, so a thread that calls the slot meanwhile
    /// runs the old fn or the new one with a live context.
    pub fn set_callback(self, callback: crate::callbacks::Callback) {
        self.state().callbacks.set(callback);
    }

    /// `serv->onX != nullptr` of callback `slot`.
    #[must_use]
    pub fn has_callback(self, slot: crate::callbacks::CallbackKind) -> bool {
        self.state().callbacks.slot(slot).is_set()
    }

    /// The server's configuration.
    #[must_use]
    pub fn config(self) -> &'a ServerConfig {
        // SAFETY: the config allocation lives as long as the server, and writers
        // hold no reference across a read.
        unsafe { &*self.fields().config }
    }

    /// The server's configuration, to write.
    ///
    /// # Safety
    /// No other reference to the configuration is live meanwhile, and no other
    /// thread reads the fields written (the server's own synchronization).
    #[must_use]
    #[cfg(test)]
    pub unsafe fn config_mut(self) -> &'a mut ServerConfig {
        // SAFETY: per the caller; the allocation lives as long as the server.
        unsafe { &mut *self.fields().config }
    }

    /// `serv->get_worker_message_bus()->move_packet()`: the bus no longer frees
    /// the packet it holds; its new owner, given its block (null for none), does.
    #[cfg(all(feature = "upstream-symbols", not(test)))]
    #[expect(
        clippy::must_use_candidate,
        reason = "a caller that read the message already holds the block it returns"
    )]
    pub fn move_worker_packet(self) -> *mut core::ffi::c_char {
        crate::master_ops::worker_message_bus(self.state())
            .rust()
            .move_packet()
    }

    /// `set_allocator(allocator)` on the server's own `MessageBus`, or with
    /// `worker` on `get_worker_message_bus()`: the bus allocates its packets with
    /// `allocator` from now on.
    #[cfg(all(feature = "upstream-symbols", not(test)))]
    pub fn set_bus_allocator(self, worker: bool, allocator: swoole_rs_corebase::TrustedAllocator) {
        let state = self.state();
        let bus = if worker {
            state.worker_bus_record()
        } else {
            state.message_bus_record()
        };
        bus.set_allocator(allocator);
    }

    /// `upload_tmp_dir = dir`.
    pub fn set_upload_tmp_dir(self, dir: &[u8]) {
        self.state().set_upload_tmp_dir(dir);
    }

    /// `serv-><text> = value` for text member `kind`.
    pub fn set_text(self, kind: crate::ServerText, value: &[u8]) {
        self.state().set_text(kind, value);
    }

    /// `insert(value)` into a static handler collection.
    pub fn insert_static_string(self, collection: crate::StaticCollection, value: &[u8]) {
        crate::static_config::config(
            self.state(),
            collection,
            crate::static_config::Op::Insert(value),
        );
    }

    /// `url_rewrite_rules`: add one rule.
    pub fn add_rewrite_rule(self, pattern: &[u8], replacement: &[u8]) {
        crate::static_config::add_rewrite_rule(self.state(), pattern, replacement);
    }

    /// `f` over a static handler collection: whether it is set, and its values.
    pub fn static_strings<R>(
        self,
        collection: crate::StaticCollection,
        f: impl FnOnce(bool, &[Arc<[u8]>]) -> R,
    ) -> R {
        crate::static_config::strings(self.state(), collection, f)
    }

    /// `apply_rewrite_rules` on a static context: true when a rule applied.
    pub fn static_rewrite(self, context: &mut swoole_rs_http::HttpStaticContext) -> bool {
        crate::static_config::apply_rewrite_rules(self.state(), context)
    }

    /// `document_root`.
    #[must_use]
    pub fn document_root(self) -> std::sync::Arc<[u8]> {
        self.state().text(crate::ServerText::DocumentRoot)
    }

    /// `upload_tmp_dir.c_str()`: the directory up to its first NUL.
    #[must_use]
    pub fn upload_tmp_dir(self) -> CString {
        self.state().upload_tmp_dir()
    }

    /// `http_compression_types` lists `kind`; `None` asks whether it lists any.
    #[must_use]
    pub fn compression_type(self, kind: Option<&[u8]>) -> bool {
        use crate::static_config::{Op, config};
        let state = self.state();
        let op = |operation| config(state, crate::StaticCollection::CompressionTypes, operation);
        let types = op(Op::Enabled) != 0;
        match kind {
            Some(kind) if types => op(Op::Find(kind)) != op(Op::Count),
            _ => types,
        }
    }

    /// The shared counters (`serv->gs`).
    #[must_use]
    pub fn counters(self) -> &'a ServerCounters {
        &crate::shared::ServerShared::of(self).counters
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
        config.worker_num.get() == 1
            && config.task_worker_num.get() == 0
            && config.max_request.get() == 0
            && self.user_worker_num() == 0
    }

    /// A single base-mode worker's server that this process ran and never
    /// destroyed: worker 0 records its pid as the master's, and `destroy` drops
    /// the factory. Its event worker pool is this process's alone.
    #[must_use]
    pub fn ran_its_single_worker_here(self) -> bool {
        let config = self.config();
        config.mode_ == Mode::Base
            && self.is_single_worker()
            && self.counters().master_pid.load(Ordering::Relaxed)
                == std::process::id().cast_signed()
            && !config.factory_.is_null()
    }

    /// `Server::is_worker_thread()`: thread mode, on a worker thread.
    #[must_use]
    pub fn is_worker_thread(self) -> bool {
        /// `THREAD_WORKER`.
        const THREAD_WORKER: u8 = 4;
        self.config().mode_ == Mode::Thread && swoole_rs_corebase::thread_kind() == THREAD_WORKER
    }

    /// `Server::get_connection_num()`: the per-worker cells when there are any.
    #[must_use]
    pub fn connection_num(self) -> usize {
        let gs = self.counters();
        self.connections(&gs.connection_num, gs.cells())
    }

    /// The sum of a record's per-worker connection `cells`, or its one shared
    /// `total` when it has none.
    fn connections(self, total: &AtomicU32, cells: &'a [AtomicU32]) -> usize {
        if cells.is_empty() {
            return total.load(Ordering::Relaxed) as usize;
        }
        (0..self.config().worker_num.get() as usize)
            .filter_map(|id| self.cell(cells, id))
            .map(|cell| cell.load(Ordering::Relaxed) as usize)
            .fold(0, usize::wrapping_add)
    }

    /// Event worker `id`'s cell of one of the server's records' per-worker connection
    /// `cells`; `None` without cells, or past the last event worker.
    fn cell(self, cells: &'a [AtomicU32], id: usize) -> Option<&'a AtomicU32> {
        cells
            .get(id)
            .filter(|_| id < self.config().worker_num.get() as usize)
    }

    /// Event worker `id`'s count of the server's connections (base mode's
    /// `gs->connection_nums[id]`), if the server counts them per worker.
    pub(crate) fn worker_connections(self, id: usize) -> Option<&'a AtomicU32> {
        self.cell(self.counters().cells(), id)
    }

    /// Event worker `id`'s count of `port`'s connections, as [`Self::worker_connections`];
    /// `port` is one of this server's.
    pub(crate) fn port_worker_connections(
        self,
        port: Port<'a>,
        id: usize,
    ) -> Option<&'a AtomicU32> {
        self.cell(port.config().counters()?.cells(), id)
    }

    /// `Server::get_connection_verify(session_id)`.
    #[must_use]
    pub fn verified(self, session_id: i64) -> Option<Connection<'a>> {
        self.state().connection(session_id, Lookup::Verified)
    }

    /// `Server::get_connection_by_session_id(session_id)`: the session's slot,
    /// unverified.
    #[must_use]
    pub fn by_session(self, session_id: i64) -> Option<Connection<'a>> {
        self.state().connection(session_id, Lookup::BySession)
    }

    /// `Server::get_connection_for_iterator(fd)`.
    #[must_use]
    pub fn iterated(self, fd: c_int) -> Option<Connection<'a>> {
        self.state().connection(c_long::from(fd), Lookup::Iterated)
    }

    /// `Server::get_connection(fd)`.
    #[must_use]
    pub fn at(self, fd: c_int) -> Option<Connection<'a>> {
        self.state().connection(c_long::from(fd), Lookup::ByFd)
    }

    /// `field` of the connection at `key`; `None` for no slot.
    fn connection_field<T>(
        self,
        key: c_long,
        kind: Lookup,
        field: impl FnOnce(Connection<'a>) -> T,
    ) -> Option<T> {
        self.state().connection(key, kind).map(field)
    }

    /// `ListenPort::get_connection_num()` of one of the server's ports.
    #[must_use]
    pub fn port_connection_num(self, port: Port<'_>) -> usize {
        self.port_connections(port.config())
    }

    /// `ListenPort::get_connection_num()` of the port whose settings are `config`.
    pub(crate) fn port_connections(self, config: &PortConfig) -> usize {
        config
            .counters()
            .map_or(0, |gs| self.connections(&gs.connection_num, gs.cells()))
    }

    /// `Server::get_max_connection()`.
    #[must_use]
    pub fn max_connection(self) -> u32 {
        self.state().tables_ref().max_connection
    }

    /// `serv->get_task_worker_pool()`.
    #[must_use]
    pub fn task_pool(self) -> *mut PoolRecord {
        self.task_pool_ref().raw()
    }

    /// The server's task pool, to run pool operations on.
    #[must_use]
    pub fn task_pool_ref(self) -> PoolRef<'a> {
        crate::shared::ServerShared::of(self).task_pool()
    }

    /// The server's event pool, right after its task pool.
    #[must_use]
    pub fn event_pool_ref(self) -> PoolRef<'a> {
        crate::shared::ServerShared::of(self).event_pool()
    }

    /// The server's worker registry, which its configuration owns for the server's life.
    #[must_use]
    pub fn registry(self) -> &'a WorkerRegistry {
        // A server's state gives its config the registry before any op runs.
        self.config()
            .worker_registry
            .as_deref()
            .unwrap_or_else(|| swoole_rs_corebase::fatal("a server without a worker registry"))
    }

    /// `Server::get_user_worker_num()`: the user workers added so far.
    #[must_use]
    pub fn user_worker_num(self) -> usize {
        self.registry().pending_count()
    }

    /// The server's worker arrays and counts.
    #[must_use]
    pub fn workers(self) -> ServerWorkers<'a> {
        let config = self.config();
        let events = (
            self.event_pool_ref(),
            config.workers.as_ptr().cast(),
            config.worker_num.get(),
        );
        let tasks = (self.task_pool_ref(), config.task_worker_num.get());
        let users = (config.user_workers.as_ptr().cast(), self.user_worker_num());
        // SAFETY: `create` maps `workers` with `worker_num` records, and the factory's
        // start `user_workers` with one per added user worker (`add_worker` refuses
        // once the factory exists); each stays mapped, as the counts stay, until the
        // server is destroyed after its pools stop. The same arrays `worker_record`
        // lends records of.
        unsafe { ServerWorkers::new(events, tasks, users) }
    }

    /// `Server::get_worker(id)`: event workers, then task workers, then user
    /// workers; null past the last.
    #[must_use]
    pub fn worker(self, id: u16) -> *mut WorkerRecord {
        swr_v1_process_server_worker(&self.workers(), id)
    }

    /// [`Server::worker`]'s record, or `None` past the last.
    #[must_use]
    pub fn worker_record(self, id: u16) -> Option<Worker<'a>> {
        // SAFETY: null or one of the server's live workers, in memory the server
        // keeps for `'a`.
        unsafe { Worker::from_raw(self.worker(id)) }
    }

    /// The `index`th user worker `worker_methods::add_worker` added, or `None` past the
    /// last.
    pub(crate) fn pending_worker(self, index: usize) -> Option<Worker<'a>> {
        let &worker = self.registry().pending().get(index)?;
        // SAFETY: null, or a live worker `add_worker` added, which outlives the server
        // (its contract; nothing else adds to the server's pending workers).
        unsafe { Worker::from_raw(worker.cast()) }
    }

    /// Worker `id`'s pipe socket, its master end or its worker end, or `None` while it has none.
    #[must_use]
    pub(crate) fn worker_pipe(self, id: u16, master: bool) -> Option<Socket<'a>> {
        worker_pipe(self.worker_record(id)?, master)
    }

    /// The listening socket of `serv->ports[index]`, or `None` past the last.
    #[must_use]
    pub(crate) fn port_socket_at(self, index: usize) -> Option<Socket<'a>> {
        // One of the server's live ports, whose socket it keeps open while it serves.
        self.port_at(index)?.config().listening_socket()
    }

    /// `Server::task_notify_pipe(worker_id)`, or null.
    #[must_use]
    pub fn task_notify_pipe(self, worker_id: u16) -> *mut SocketPair {
        self.state().task_pipe(worker_id.into())
    }

    /// `Server::get_server_socket(fd)`: the fd and `swSocketType` of the
    /// listening socket at `fd`, or `None`.
    #[must_use]
    pub fn server_socket(self, fd: c_long) -> Option<(c_int, c_int)> {
        let socket = self.connection_field(fd, Lookup::ByFd, Connection::socket)??;
        Some(socket.read(|s| (s.fd, s.socket_type)))
    }

    /// The fd and `swSocketType` of the server's own datagram socket of `kind`
    /// (1 UDP, 2 UDP6, else the unix one): upstream's `udp_socket_ipv4`,
    /// `udp_socket_ipv6` or `dgram_socket`, the listener of the last port of that
    /// type added; `None` before one listens.
    #[must_use]
    pub fn dgram_socket(self, kind: u8) -> Option<(c_int, c_int)> {
        // `SW_SOCK_UDP`, `SW_SOCK_UDP6` and `SW_SOCK_UNIX_DGRAM`.
        let socket_type = match kind {
            1 => 2,
            2 => 4,
            _ => 6,
        };
        let port = self.find_last_port(|config| config.r#type == socket_type)?;
        port.with_socket(|s| s.read(|s| (s.fd, s.socket_type)))
    }

    /// The last of `serv->ports` whose settings `wanted` accepts.
    fn find_last_port(self, wanted: impl Fn(&PortConfig) -> bool) -> Option<Port<'a>> {
        self.ports()
            .iter()
            .rev()
            .copied()
            .find(|port| wanted(port.config()))
    }

    /// `Server::get_port_by_fd(fd)`: the `ListenPort` a connection arrived on.
    #[must_use]
    pub fn port_by_fd(self, fd: c_int) -> Option<Port<'a>> {
        let server_fd =
            self.connection_field(c_long::from(fd), Lookup::ByFd, Connection::server_fd)?;
        self.port_by_server_fd(server_fd)
    }

    /// `Server::get_port_by_server_fd(server_fd)`: the `ListenPort` listening on
    /// `server_fd`.
    #[must_use]
    pub fn port_by_server_fd(self, server_fd: c_int) -> Option<Port<'a>> {
        self.state().port_by_server_fd(server_fd)
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
    pub fn healthy(self, now: f64, conn: Connection<'_>) -> bool {
        if conn.protect() || conn.last_recv_time() == 0.0 {
            return true;
        }
        // `get_port_by_session_id`.
        let Some(fd) = self.connection_field(conn.session_id(), Lookup::BySession, Connection::fd)
        else {
            return true;
        };
        let Some(port) = self.port_of_fd(fd) else {
            return true;
        };
        let idle = port.heartbeat_idle_time.get();
        idle == 0 || conn.last_recv_time() > now - f64::from(idle)
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
        self.find_port(|config| port == 0 || config.port.get() == port)
    }

    /// The first of `serv->ports` whose settings `wanted` accepts.
    pub fn find_port(self, wanted: impl Fn(&PortConfig) -> bool) -> Option<Port<'a>> {
        self.ports()
            .iter()
            .copied()
            .find(|port| wanted(port.config()))
    }
}

impl ServerState {
    /// `Server::get_port_by_server_fd(server_fd)`: the `ListenPort` listening on
    /// `server_fd`.
    pub(crate) fn port_by_server_fd(&self, server_fd: c_int) -> Option<Port<'_>> {
        self.tables_ref().connection_at(server_fd)?.port()
    }

    /// The server's port at `raw`, or `None` when `raw` is none of them: a slot's or a
    /// socket's `object` names a port only on a listening socket, and anything else
    /// (a session's protocol state, a pipe) elsewhere.
    pub(crate) fn listed_port(&self, raw: *mut c_void) -> Option<Port<'_>> {
        let ports = self.tables_ref().ports();
        ports.iter().copied().find(|port| port.raw() == raw)
    }
}

/// A live `ListenPort`, borrowed for `'a`: its settings' box
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
    /// synchronization keeps writers of its settings apart
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
        // SAFETY: a live port is its settings, with writers kept apart (`from_raw`).
        unsafe { &*self.raw.as_ptr().cast::<PortConfig>() }
    }

    /// `port->gs = gs`: the port's slot in `port_gs_list`.
    pub(crate) fn set_gs(self, gs: *mut c_void) {
        self.config().gs.set(gs);
    }
    /// `port->protocol.package_max_length = length`.
    pub(crate) fn set_package_max_length(self, length: u32) {
        self.config()
            .protocol
            .update(|protocol| protocol.package_max_length = length);
    }
    /// `f` over the port's listening socket, or `None` before it has one (or after
    /// it closes). The socket can't outlive `f`: a port's socket closes while the port
    /// lives (`OP_PORT_CLOSE_SOCKET`), so no handle to it is kept past a read.
    pub fn with_socket<R>(self, f: impl FnOnce(Socket<'_>) -> R) -> Option<R> {
        // Live now, and the handle ends with `f`.
        self.config().listening_socket().map(f)
    }

    /// `ListenPort::get_fd()`.
    #[must_use]
    pub fn fd(self) -> c_int {
        self.with_socket(|socket| socket.fd()).unwrap_or(-1)
    }
}

/// An end of a worker's pipe.
#[derive(Clone, Copy)]
pub enum PipeEnd {
    /// `pipe_master`, the master's end.
    Master,
    /// `pipe_worker`, the worker's end.
    Worker,
    /// `pipe_current`, the end this process uses: one of the other two, or none.
    Current,
}

/// The `end` of `worker`'s pipe, or `None` while it has none.
#[inline]
#[must_use]
pub fn worker_pipe_end(worker: Worker<'_>, end: PipeEnd) -> Option<Socket<'_>> {
    let pipe = match end {
        PipeEnd::Master => worker.pipe_master(),
        PipeEnd::Worker => worker.pipe_worker(),
        PipeEnd::Current => worker.pipe_current(),
    };
    // SAFETY: crates/process keeps the pipe fields private: null, or an end of the
    // pair `Worker::set_pipes` gave, live until its owner clears or frees it (and
    // `pipe_current` a copy of one of them), so for the worker's handle.
    unsafe { Socket::from_ptr(pipe.cast()) }
}

/// The master end (`master`) or the worker end of `worker`'s pipe, or `None` while
/// it has none.
#[inline]
#[must_use]
pub(crate) fn worker_pipe(worker: Worker<'_>, master: bool) -> Option<Socket<'_>> {
    worker_pipe_end(
        worker,
        if master {
            PipeEnd::Master
        } else {
            PipeEnd::Worker
        },
    )
}

/// The listening socket of `pool`'s stream IPC, or `None` without one.
#[must_use]
pub fn pool_ipc_listener(pool: PoolRef<'_>) -> Option<Socket<'_>> {
    // SAFETY: crates/process keeps a pool's IPC private: null, or the listener the
    // pool's IPC owns until the pool's `destroy` frees it.
    unsafe { Socket::from_ptr(pool.ipc_listener().cast()) }
}

/// `sock->set_nonblock()`.
pub(crate) fn set_nonblock(sock: Socket<'_>) {
    let set = swoole_rs_reactor::socket_upstream::socket_set_fd_option;
    let _ = sock.edit(|sock| set(sock, 1, -1));
}

/// The connection socket's `recv_buffer` (a `swoole::String *`), or null.
#[must_use]
pub fn recv_buffer(conn: Connection<'_>) -> *mut c_void {
    conn.socket().map_or(core::ptr::null_mut(), |socket| {
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
    #[cfg_attr(miri, ignore = "Miri has no shared mappings")]
    fn only_a_single_base_worker_that_ran_here_and_was_not_destroyed_counts() {
        let state = crate::swr_v1_server_state_new();
        let head = [state.cast::<c_void>(), unsafe { (*state).config() }.cast()];
        let serv = unsafe { Server::from_raw(head.as_ptr().cast_mut().cast()) };
        let config = unsafe { serv.config_mut() };
        config.gs =
            swoole_rs_memory::members::Shm::calloc(1, size_of::<crate::shared::ServerShared>());
        config.mode_ = Mode::Base;
        config.worker_num.set(1);
        let mut factory = 0u8;
        config.factory_ = (&raw mut factory).cast();
        let own = std::process::id().cast_signed();
        serv.counters().master_pid.store(own, Ordering::Relaxed);
        assert!(serv.ran_its_single_worker_here());
        serv.counters().master_pid.store(own + 1, Ordering::Relaxed);
        assert!(!serv.ran_its_single_worker_here(), "another process's");
        serv.counters().master_pid.store(own, Ordering::Relaxed);
        let config = unsafe { serv.config_mut() };
        config.factory_ = core::ptr::null_mut();
        assert!(!serv.ran_its_single_worker_here(), "destroyed");
        config.factory_ = (&raw mut factory).cast();
        config.mode_ = Mode::Process;
        assert!(!serv.ran_its_single_worker_here(), "process mode");
        config.mode_ = Mode::Base;
        config.worker_num.set(2);
        assert!(
            !serv.ran_its_single_worker_here(),
            "a manager and two workers"
        );
        config.worker_num.set(1);
        config.task_worker_num.set(1);
        assert!(!serv.ran_its_single_worker_here(), "a task worker");
        config.task_worker_num.set(0);
        config.factory_ = core::ptr::null_mut();
        config.gs.free();
        unsafe { crate::state::free_state(state) };
    }

    /// A port counts its connections in the per-worker cells when it has them, one
    /// per event worker, and in its one shared count otherwise.
    #[test]
    #[cfg_attr(miri, ignore = "Miri has no shared mappings")]
    fn port_counts_use_the_shared_worker_cells_when_present() {
        use crate::PortCounters;
        let state = crate::swr_v1_server_state_new();
        let head = [state.cast::<c_void>(), unsafe { (*state).config() }.cast()];
        let serv = unsafe { Server::from_raw(head.as_ptr().cast_mut().cast()) };
        let mut counters = PortCounters::default();
        counters.connection_num.store(99, Ordering::Relaxed);
        let port = PortConfig::boxed(0);
        port.gs.set((&raw mut counters).cast());
        unsafe { serv.config_mut() }.worker_num.set(2);
        assert_eq!(serv.port_connections(&port), 99);
        counters.connection_nums = swoole_rs_memory::members::Shm::calloc(2, 4);
        counters.cells()[0].store(3, Ordering::Relaxed);
        counters.cells()[1].store(5, Ordering::Relaxed);
        port.gs.set((&raw mut counters).cast());
        assert_eq!(serv.port_connections(&port), 8);
        unsafe { serv.config_mut() }.worker_num.set(0);
        assert_eq!(serv.port_connections(&port), 0);
        port.gs.set(core::ptr::null_mut());
        assert_eq!(serv.port_connections(&port), 0, "no counters yet");
        counters.connection_nums.free();
        unsafe { crate::state::free_state(state) };
    }
}
