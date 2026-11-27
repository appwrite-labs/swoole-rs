//! The server's Rust-owned state (`Server::state_`: the round-robin counter and the
//! command id counter) and the operations over the rest of `Server` that
//! `master.cc` kept: worker scheduling, commands, the document root, worker counters, aborts, connection iteration, sends, timers
//! and the startup error message.
use core::ffi::c_int;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicUsize, Ordering};
use std::sync::OnceLock;

use crate::port_config::PortConfig;
use crate::view::Port;
use std::ffi::CString;
use swoole_rs_corebase::tls::socket_pair::SocketPair;

/// Reactor threads use their IDs as thread keys.
pub(crate) const HEARTBEAT: u64 = u64::MAX;

/// Owned by `Server::state_`, from the constructor to the destructor. Reactor threads
/// share it, so each counter is atomic.
pub struct ServerState {
    /// `worker_round_id`: the next worker `DISPATCH_ROUND` and idle dispatch try.
    round: AtomicU32,
    /// `command_current_id`: the id the next command gets, from 1.
    pub(crate) next_command: AtomicI32,
    /// Each kept in its own allocation, whose address native code and the writers
    /// under the server's own synchronization hold: an `Rc`, never cloned, is never retagged unique.
    config: std::rc::Rc<crate::config::ServerConfig>,
    tables: std::rc::Rc<crate::records::ServerTables>,
    pub(crate) static_config: crate::static_config::Configs,
    pub(crate) commands: std::sync::Mutex<crate::commands::Commands>,
    /// `Server::ports`, which the state deletes as it drops, as `~Server` did.
    pub(crate) ports: Ports,
    /// The on* members and `dispatch_func` (`callbacks.rs`).
    pub(crate) callbacks: crate::callbacks::ServerCallbacks,
    /// `upload_tmp_dir`, the HTTP server's directory for uploaded files
    /// (`None`: `/tmp`), as `set()` gave it.
    upload_tmp_dir: std::sync::Mutex<Option<Box<[u8]>>>,
    /// `chroot_`, `user_`, `group_`, `pid_file` and `document_root`, in
    /// [`ServerText`] order. Readers share the value; a write replaces it.
    texts: [std::sync::Mutex<std::sync::Arc<[u8]>>; 5],
    /// `Server::lock_`, the connection list's fd range lock.
    pub(crate) locked: FdLock,
    local_bounds: crate::shared::FdBounds,
    /// The reactor threads and the heartbeat thread, by `start_thread`'s `arg`.
    threads: std::sync::Mutex<Vec<(u64, std::thread::JoinHandle<()>)>>,
    heartbeat_stopped: AtomicBool,
    /// The interface addresses `Server::create` first read (`local_addr`); none before.
    pub(crate) local_addrs: std::sync::OnceLock<crate::local_addr::LocalAddrs>,
    /// `gs->pipe_packet_msg_id`, which numbers every bus's messages: the shared
    /// counter `swr_v1_server_shared_init` returned, mapped as long as the buses use it.
    pub(crate) msg_ids: std::sync::OnceLock<&'static core::sync::atomic::AtomicU64>,
    /// `Server::task_notify_pipes`: a blocking `Pipe` per event worker, which
    /// `task_sync` waits on. Set once, so a pipe lent out stays put; the state's drop
    /// closes them, as `~Server` did.
    task_pipes: OnceLock<Box<[SocketPair]>>,
    /// `Server::worker_pipes`: each event worker's `UnixSocket`, which
    /// the state's drop closes, as `~Server` did.
    #[expect(clippy::vec_box, reason = "a worker holds its pair's address")]
    worker_pipes: std::sync::Mutex<Vec<Box<SocketPair>>>,
    /// A worker thread asked to shut down: the thread manager respawns no worker.
    pub(crate) stopping: AtomicBool,
    /// Thread mode's factory's manager, from its factory's creation to its deletion.
    pub(crate) thread_manager:
        std::sync::Mutex<Option<std::sync::Arc<crate::thread_mode::ThreadManager>>>,
    /// `Server::reactor_thread_barrier`.
    pub(crate) reactor_thread_barrier: swoole_rs_corebase::locks::Barrier,
}

/// A port's place in [`Ports`], set once.
type Slot = OnceLock<Box<PortConfig>>;

/// `Server::ports`: the listen ports in listen order, each owned from its push to the
/// state's drop. Append-only, in chunks that never move, so a [`Port`] lent out stays
/// valid while more ports are added, as upstream's `ListenPort *`s did.
#[derive(Default)]
pub struct Ports {
    /// Chunk `k` holds ports `2^k - 1` to `2^(k+1) - 2`: sixteen hold 65,535, more than
    /// `add_port` allows.
    chunks: [OnceLock<Box<[Slot]>>; 16],
    /// How many are pushed: every port below it is set before the count is published.
    len: AtomicUsize,
    /// Held by a push.
    push: std::sync::Mutex<()>,
}

impl Ports {
    /// The chunk and the slot in it of port `index`.
    fn slot(index: usize) -> (usize, usize) {
        let n = index + 1;
        let chunk = usize::try_from(n.ilog2()).unwrap_or(0);
        (chunk, n - (1 << chunk))
    }

    /// How many ports the server has.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len.load(Ordering::Acquire)
    }

    /// Whether the server has no port.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// `ports[index]`, or `None` past the last.
    #[must_use]
    #[inline]
    pub fn get(&self, index: usize) -> Option<Port<'_>> {
        if index >= self.len() {
            return None;
        }
        let (chunk, at) = Self::slot(index);
        let port = self.chunks.get(chunk)?.get()?.get(at)?.get()?;
        Some(Port::from(&**port))
    }

    /// The ports in listen order.
    #[must_use]
    pub fn iter(&self) -> PortsIter<'_> {
        PortsIter {
            ports: self,
            range: 0..self.len(),
        }
    }

    /// The last port added.
    #[must_use]
    pub fn last(&self) -> Option<Port<'_>> {
        self.get(self.len().checked_sub(1)?)
    }

    /// `ports.push_back(port)`: the list owns the port from now on, and the port's
    /// `object_id` is the new count.
    pub(crate) fn push(&self, mut port: Box<PortConfig>) -> Port<'_> {
        let push = lock(&self.push);
        let index = self.len.load(Ordering::Relaxed);
        port.object_id = u16::try_from(index + 1).unwrap_or(u16::MAX);
        let (chunk, at) = Self::slot(index);
        let Some(slots) = self.chunks.get(chunk) else {
            swoole_rs_corebase::fatal("more ports than the list holds");
        };
        let slots = slots.get_or_init(|| (0..1 << chunk).map(|_| OnceLock::new()).collect());
        if slots.get(at).is_none_or(|slot| slot.set(port).is_err()) {
            swoole_rs_corebase::fatal("a port slot is taken");
        }
        self.len.store(index + 1, Ordering::Release);
        drop(push);
        self.get(index)
            .unwrap_or_else(|| swoole_rs_corebase::fatal("a pushed port is not listed"))
    }

    /// The ports, which the list no longer holds.
    fn drain(&mut self) -> impl Iterator<Item = Box<PortConfig>> {
        *self.len.get_mut() = 0;
        self.chunks
            .iter_mut()
            .filter_map(OnceLock::take)
            .flat_map(|slots| slots.into_iter().filter_map(OnceLock::into_inner))
    }
}

/// [`Ports::iter`]: the ports listed when it began.
pub struct PortsIter<'a> {
    ports: &'a Ports,
    range: core::ops::Range<usize>,
}

impl<'a> Iterator for PortsIter<'a> {
    type Item = Port<'a>;

    fn next(&mut self) -> Option<Port<'a>> {
        self.ports.get(self.range.next()?)
    }
}

impl DoubleEndedIterator for PortsIter<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.ports.get(self.range.next_back()?)
    }
}

impl<'a> IntoIterator for &'a Ports {
    type Item = Port<'a>;
    type IntoIter = PortsIter<'a>;

    fn into_iter(self) -> PortsIter<'a> {
        self.iter()
    }
}

/// `Server::lock_`: whether it is held, and how many threads wait for it.
#[derive(Default)]
pub(crate) struct FdLock(std::sync::Mutex<(bool, u32)>, std::sync::Condvar);

impl FdLock {
    /// `Server::lock()`: blocks until this thread holds the lock.
    pub(crate) fn lock(&self) {
        let Self(lock, released) = self;
        let mut lock = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while lock.0 {
            lock.1 += 1;
            lock = released
                .wait(lock)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            lock.1 -= 1;
        }
        lock.0 = true;
    }

    /// `Server::unlock()`, by the thread holding the lock. std's condvar makes a
    /// futex syscall on every notify, so only a waiter is woken: `add` and
    /// `close_connection` unlock once each a connection (#2964).
    pub(crate) fn unlock(&self) {
        let Self(lock, released) = self;
        let mut lock = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        lock.0 = false;
        if lock.1 > 0 {
            released.notify_one();
        }
    }
}

/// The server's text members, `ServerState::texts`' slots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerText {
    Chroot,
    User,
    Group,
    PidFile,
    DocumentRoot,
}

impl Default for ServerState {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerState {
    pub(crate) fn new() -> Self {
        Self::with(
            crate::config::ServerConfig::default(),
            crate::records::ServerTables::default(),
        )
    }

    /// A state that owns `config` and `tables`, so neither pointer is ever null.
    pub(crate) fn with(
        config: crate::config::ServerConfig,
        tables: crate::records::ServerTables,
    ) -> Self {
        Self {
            round: AtomicU32::new(0),
            next_command: AtomicI32::new(1),
            config: std::rc::Rc::new(config),
            tables: std::rc::Rc::new(tables),
            static_config: std::sync::OnceLock::new(),
            commands: std::sync::Mutex::new(crate::commands::Commands::new()),
            ports: Ports::default(),
            callbacks: crate::callbacks::ServerCallbacks::new(),
            upload_tmp_dir: std::sync::Mutex::new(None),
            texts: Default::default(),
            locked: FdLock::default(),
            local_bounds: crate::shared::FdBounds::default(),
            threads: std::sync::Mutex::new(Vec::new()),
            heartbeat_stopped: AtomicBool::new(false),
            local_addrs: std::sync::OnceLock::new(),
            msg_ids: std::sync::OnceLock::new(),
            task_pipes: OnceLock::new(),
            worker_pipes: std::sync::Mutex::new(Vec::new()),
            stopping: AtomicBool::new(false),
            thread_manager: std::sync::Mutex::new(None),
            reactor_thread_barrier: swoole_rs_corebase::locks::Barrier::new(),
        }
    }

    /// `new UnixSocket(true, SOCK_DGRAM)` for `worker`'s pipes: false, closing it,
    /// when the pair isn't ready.
    pub(crate) fn push_worker_pipe(
        &self,
        worker: swoole_rs_corebase::process::worker_record::Worker<'_>,
    ) -> bool {
        let mut pair = Box::new(swoole_rs_corebase::tls::socket_pair::unix_socket(
            true,
            libc::SOCK_DGRAM,
        ));
        let (master, end) = pair.raw_ends();
        if master.is_null() || end.is_null() {
            swoole_rs_corebase::tls::socket_pair::drop(&mut pair);
            return false;
        }
        let mut pipes = lock(&self.worker_pipes);
        pipes.push(pair);
        // The box keeps the pair at this address while the list owns it.
        let pipe = pipes
            .last_mut()
            .map_or(core::ptr::null_mut(), |pair| &raw mut **pair);
        // SAFETY: the two ends of the boxed pair the server's list now owns, which
        // the state keeps for as long as the server has its workers.
        unsafe { worker.set_pipes(pipe.cast(), master.cast(), end.cast()) };
        true
    }

    /// `task_notify_pipes.emplace_back(new Pipe(true))` for each of `count` workers:
    /// Publish the pipes only once all are open and the task workers initialize.
    pub(crate) fn open_task_pipes(&self, count: u32, init: impl FnOnce() -> bool) -> bool {
        let mut pipes = Vec::new();
        let ready = (0..count).all(|_| {
            let pair = swoole_rs_corebase::tls::socket_pair::pipe(true);
            let ready = pair.master_end().is_some() && pair.worker_end().is_some();
            pipes.push(pair);
            ready
        }) && init();
        if ready {
            pipes = match self.task_pipes.set(pipes.into_boxed_slice()) {
                Ok(()) => return true,
                Err(existing) => existing.into_vec(),
            };
        }
        for pipe in &mut pipes {
            swoole_rs_corebase::tls::socket_pair::drop(pipe);
        }
        ready
    }

    /// `task_notify_pipes.at(index)`, or `None` past the end.
    pub(crate) fn task_pipe(&self, index: usize) -> Option<&SocketPair> {
        self.task_pipes.get()?.get(index)
    }

    fn slot(&self, kind: ServerText) -> &std::sync::Mutex<std::sync::Arc<[u8]>> {
        let [chroot, user, group, pid_file, document_root] = &self.texts;
        match kind {
            ServerText::Chroot => chroot,
            ServerText::User => user,
            ServerText::Group => group,
            ServerText::PidFile => pid_file,
            ServerText::DocumentRoot => document_root,
        }
    }

    /// Text member `kind`.
    pub(crate) fn text(&self, kind: ServerText) -> std::sync::Arc<[u8]> {
        std::sync::Arc::clone(&lock(self.slot(kind)))
    }

    /// Sets text member `kind`.
    pub(crate) fn set_text(&self, kind: ServerText, value: &[u8]) {
        *lock(self.slot(kind)) = value.into();
    }

    /// `Server::set_document_root(path)`: the path's real path, if it exists.
    pub(crate) fn set_document_root(&self, path: &[u8]) -> bool {
        use std::os::unix::ffi::OsStrExt;
        /// `SW_LOG_WARNING`.
        use swoole_rs_corebase::log::LogLevel::SW_LOG_WARNING as LOG_WARNING;
        let origin = c"Server::set_document_root";
        if path.len() > usize::try_from(libc::PATH_MAX).unwrap_or(usize::MAX) {
            let code = swoole_rs_error::ErrorCode::SW_ERROR_NAME_TOO_LONG as c_int;
            let m = format_args!(
                "The length of document_root must be less than {}",
                libc::PATH_MAX
            );
            swoole_rs_corebase::log::error_log(LOG_WARNING, code, origin, m);
            return false;
        }
        // `realpath` reads the path up to its first NUL.
        let path = path.split(|byte| *byte == 0).next().unwrap_or_default();
        let real = std::fs::canonicalize(std::ffi::OsStr::from_bytes(path));
        let Ok(real) = real else {
            let code = swoole_rs_error::ErrorCode::SW_ERROR_DIR_NOT_EXIST as c_int;
            let m = format_args!(
                "document_root[{}] does not exist",
                String::from_utf8_lossy(path)
            );
            swoole_rs_corebase::log::error_log(LOG_WARNING, code, origin, m);
            return false;
        };
        self.set_text(ServerText::DocumentRoot, real.as_os_str().as_bytes());
        true
    }

    /// `swoole_set_isolation(group_, user_, chroot_)`: a worker drops to the
    /// configured group and user, then chroots.
    pub(crate) fn set_isolation(&self) {
        let text = |kind| CString::new(&*self.text(kind)).unwrap_or_default();
        let root = text(ServerText::Chroot);
        let (user, group) = (text(ServerText::User), text(ServerText::Group));
        swoole_rs_corebase::ossupport::set_isolation(&group, &user, &root);
    }

    /// Starts a thread of `serv`, whose state this is, running `body`, which owns
    /// what it reads of the server (a share); `take_thread(arg)` hands it back. A
    /// thread that can't start aborts, as `std::thread`'s uncaught `system_error` does.
    pub(crate) fn start_thread(
        serv: crate::view::Server<'_>,
        arg: u64,
        body: impl FnOnce() + Send + 'static,
    ) {
        let thread = std::thread::Builder::new().spawn(body);
        let Ok(thread) = thread else {
            swoole_rs_corebase::fatal("a server thread can't start");
        };
        serv.state()
            .threads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((arg, thread));
    }

    pub(crate) fn start_heartbeat(
        serv: crate::view::Server<'_>,
        body: impl FnOnce() + Send + 'static,
    ) {
        serv.state()
            .heartbeat_stopped
            .store(false, Ordering::Release);
        Self::start_thread(serv, HEARTBEAT, body);
    }

    pub(crate) fn join_heartbeat(&self) {
        if let Some(heartbeat) = self.take_thread(HEARTBEAT) {
            self.heartbeat_stopped.store(true, Ordering::Release);
            heartbeat.thread().unpark();
            heartbeat.join().unwrap_or_default();
        }
    }

    pub(crate) fn heartbeat_running(&self) -> bool {
        self.running() && !self.heartbeat_stopped.load(Ordering::Acquire)
    }

    /// The running thread `start_thread` started with `arg`, if any.
    pub(crate) fn take_thread(&self, arg: u64) -> Option<std::thread::JoinHandle<()>> {
        let mut threads = self
            .threads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let index = threads.iter().position(|(id, _)| *id == arg)?;
        Some(threads.swap_remove(index).1)
    }

    /// `serv->running`.
    pub(crate) fn running(&self) -> bool {
        self.config_ref().running.load(Ordering::Acquire)
    }

    /// `serv->running = running`, seen by every thread's next [`Self::running`].
    pub(crate) fn set_running(&self, running: bool) {
        self.config_ref().running.store(running, Ordering::Release);
    }

    /// The configuration's address, which tests write through.
    #[cfg(test)]
    pub(crate) fn config(&self) -> *mut crate::config::ServerConfig {
        std::rc::Rc::as_ptr(&self.config).cast_mut()
    }

    pub(crate) fn tables(&self) -> *mut crate::records::ServerTables {
        std::rc::Rc::as_ptr(&self.tables).cast_mut()
    }

    /// The configuration, which native code writes only before the server
    /// starts or under the server's own synchronization.
    pub(crate) fn config_ref(&self) -> &crate::config::ServerConfig {
        &self.config
    }

    /// BASE workers inherit a private connection table and bounds at fork.
    pub(crate) fn fd_bounds<'a>(
        &'a self,
        shared: &'a crate::ServerCounters,
    ) -> &'a crate::shared::FdBounds {
        if self.config_ref().mode_ == crate::Mode::Base {
            &self.local_bounds
        } else {
            &shared.bounds
        }
    }

    /// The table view, written as `config_ref` says.
    pub(crate) fn tables_ref(&self) -> &crate::records::ServerTables {
        &self.tables
    }

    #[cfg(test)]
    pub(crate) fn with_tables(tables: crate::records::ServerTables) -> Self {
        Self::with(crate::config::ServerConfig::default(), tables)
    }

    /// `upload_tmp_dir = dir`; only before the server starts.
    pub(crate) fn set_upload_tmp_dir(&self, dir: &[u8]) {
        *self
            .upload_tmp_dir
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(dir.into());
    }

    /// `upload_tmp_dir.c_str()`: the directory up to its first NUL, copied under its lock.
    pub(crate) fn upload_tmp_dir(&self) -> CString {
        let dir = self
            .upload_tmp_dir
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        c_path(dir.as_deref().unwrap_or(b"/tmp"))
    }

    /// The master's pid in the pid file, when there is one.
    pub(crate) fn write_pid(&self) {
        let file = self.text(ServerText::PidFile);
        if !file.is_empty() {
            let pid = std::process::id().cast_signed().to_string();
            swoole_rs_corebase::ossupport::file_put_contents(&c_path(&file), pid.as_bytes());
        }
    }

    /// The pid file removed, when there is one.
    pub(crate) fn unlink_pid(&self) {
        let file = self.text(ServerText::PidFile);
        if !file.is_empty() {
            _ = swoole_rs_corebase::sys::unlink(&c_path(&file));
        }
    }

    /// The round before this call; it wraps as `sw_atomic_fetch_add` does.
    pub(crate) fn next_round(&self) -> u32 {
        self.round.fetch_add(1, Ordering::SeqCst)
    }
}

/// `text.c_str()`: the bytes up to the first NUL.
fn c_path(text: &[u8]) -> CString {
    let end = text
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(text.len());
    CString::new(text.get(..end).unwrap_or_default()).unwrap_or_default()
}

/// A new server's records, before a state owns them: the defaults and an empty
/// worker registry.
pub(crate) fn fresh_records() -> (crate::config::ServerConfig, crate::records::ServerTables) {
    let config = crate::config::ServerConfig {
        worker_registry: Some(Box::default()),
        ..Default::default()
    };
    (config, crate::records::ServerTables::default())
}

/// The state of a server with these records.
pub(crate) fn boxed(
    config: crate::config::ServerConfig,
    tables: crate::records::ServerTables,
) -> Box<ServerState> {
    Box::new(ServerState::with(config, tables))
}

#[cfg(test)]
pub(crate) use tests::TestServer;

/// A lock of one of the state's lists, which a panic can't leave inconsistent.
fn lock<T>(list: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    list.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Drop for ServerState {
    fn drop(&mut self) {
        // `~Server` closes `task_notify_pipes` and `worker_pipes`.
        {
            let worker = self.worker_pipes.get_mut();
            let worker = worker.unwrap_or_else(std::sync::PoisonError::into_inner);
            let task = self
                .task_pipes
                .get_mut()
                .into_iter()
                .flat_map(|pipes| pipes.iter_mut());
            for pipe in task.chain(worker.iter_mut().map(|pipe| &mut **pipe)) {
                swoole_rs_corebase::tls::socket_pair::drop(pipe);
            }
        }
        for port in self.ports.drain() {
            port.delete();
        }
        // The tables' message bus numbers its messages from a counter in the shared
        // record, so the tables go first.
        drop(core::mem::replace(
            &mut self.tables,
            std::rc::Rc::new(crate::records::ServerTables::default()),
        ));
        // The state's own configuration, so nothing borrows its shared record.
        if let Some(config) = std::rc::Rc::get_mut(&mut self.config) {
            config.gs.free();
        }
    }
}

/// `Server::get_startup_error_message`: the last error message, or the last error
/// code as text when there is no message.
pub(crate) fn startup_error(message: CString, code: c_int) -> CString {
    if !message.is_empty() || code <= 0 {
        return message;
    }
    CString::new(code.to_string()).unwrap_or_default()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    impl ServerState {
        /// The configuration, to write: borrowing the state whole, so no reference
        /// this state lent is live.
        fn config_mut(&mut self) -> &mut crate::config::ServerConfig {
            std::rc::Rc::get_mut(&mut self.config).expect("a state's own configuration")
        }

        /// The table view, to write, as [`Self::config_mut`].
        fn tables_mut(&mut self) -> &mut crate::records::ServerTables {
            std::rc::Rc::get_mut(&mut self.tables).expect("a state's own tables")
        }
    }

    /// A new server's state and the head a `Server *` points at, owned by a test: its
    /// views borrow it, and its writes take it whole.
    pub(crate) struct TestServer {
        head: Box<crate::view::Fields>,
        // Not a `Box`: moving one asserts it unique, which would leave `head` stale.
        state: std::rc::Rc<ServerState>,
    }

    impl TestServer {
        /// A server with a new server's records.
        pub(crate) fn new() -> Self {
            let (config, tables) = fresh_records();
            Self::of(boxed(config, tables))
        }

        /// A server with these tables.
        pub(crate) fn with_tables(tables: crate::records::ServerTables) -> Self {
            Self::of(Box::new(ServerState::with_tables(tables)))
        }

        fn of(state: Box<ServerState>) -> Self {
            let state = std::rc::Rc::from(state);
            let head = Box::new(crate::view::Fields::of(&state));
            Self { head, state }
        }

        pub(crate) fn serv(&self) -> crate::view::Server<'_> {
            // SAFETY: the head of a live server, which this value owns for the borrow.
            unsafe { crate::view::Server::from_raw((&raw const *self.head).cast_mut().cast()) }
        }

        pub(crate) fn state(&self) -> &ServerState {
            &self.state
        }

        pub(crate) fn config_mut(&mut self) -> &mut crate::config::ServerConfig {
            self.state_mut().config_mut()
        }

        pub(crate) fn tables_mut(&mut self) -> &mut crate::records::ServerTables {
            self.state_mut().tables_mut()
        }

        fn state_mut(&mut self) -> &mut ServerState {
            std::rc::Rc::get_mut(&mut self.state).expect("a test's own state")
        }
    }

    impl Drop for TestServer {
        /// As a server's owner does: this thread's reactor holds it no more.
        fn drop(&mut self) {
            crate::reactor_handle::release_server(self.serv());
        }
    }

    #[test]
    fn document_root_is_the_real_path() {
        let state = ServerState::new();
        let dir = std::env::temp_dir().join(format!("swoole-rs-root-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("inner")).unwrap();
        let dotted = dir.join("inner/..");
        assert!(state.set_document_root(dotted.to_str().unwrap().as_bytes()));
        let real = std::fs::canonicalize(&dir).unwrap();
        assert_eq!(
            &*state.text(ServerText::DocumentRoot),
            real.to_str().unwrap().as_bytes(),
            "the real path"
        );
        let mut nul = real.to_str().unwrap().as_bytes().to_vec();
        nul.extend_from_slice(b"\0/ignored");
        assert!(state.set_document_root(&nul), "read up to the first NUL");
        assert!(!state.set_document_root(b"/does/not/exist/swoole-rs"));
        let long = vec![b'a'; usize::try_from(libc::PATH_MAX).unwrap() + 1];
        assert!(!state.set_document_root(&long), "too long");
        assert_eq!(
            &*state.text(ServerText::DocumentRoot),
            real.to_str().unwrap().as_bytes(),
            "failures keep the root"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn startup_error_falls_back_to_the_code() {
        let text = |message: &std::ffi::CStr, code| startup_error(message.to_owned(), code);
        assert_eq!(text(c"bind failed", 98), c"bind failed");
        assert_eq!(text(c"", 1004), c"1004");
        assert_eq!(text(c"", 0), c"");
    }

    #[test]
    fn state_round_trips() {
        assert_eq!(TestServer::new().state().next_round(), 0);
    }

    #[test]
    fn texts_and_the_lock_are_answered_without_native() {
        let state = ServerState::new();
        state.set_text(ServerText::DocumentRoot, b"/srv/www");
        assert_eq!(&*state.text(ServerText::DocumentRoot), b"/srv/www");
        assert_eq!(&*state.text(ServerText::PidFile), b"");

        let lock = std::sync::Arc::new(FdLock::default());
        lock.lock();
        // Two waiters: the unlock wakes one, whose unlock wakes the other.
        let waiters: Vec<_> = (0..2)
            .map(|_| {
                let lock = std::sync::Arc::clone(&lock);
                std::thread::spawn(move || {
                    lock.lock();
                    lock.unlock();
                })
            })
            .collect();
        // Both blocked in the wait before the unlock, so it must hand over twice.
        while lock.0.lock().unwrap().1 < 2 {
            std::thread::yield_now();
        }
        assert!(
            !waiters.iter().any(std::thread::JoinHandle::is_finished),
            "the lock is held"
        );
        lock.unlock();
        for waiter in waiters {
            waiter.join().unwrap();
        }
        assert_eq!(*lock.0.lock().unwrap(), (false, 0), "free, no waiters");
    }

    #[test]
    fn the_pid_file_is_written_then_unlinked() {
        // What swoole_init sets.
        swoole_rs_corebase::swr_v1_core_set_max_file_content(64 * 1024 * 1024);
        let state = ServerState::new();
        // No pid_file: neither touches the file system.
        state.write_pid();
        state.unlink_pid();
        let path = std::env::temp_dir().join(format!("swoole-rs-pid-{}", std::process::id()));
        let bytes = path.to_str().unwrap().as_bytes();
        state.set_text(ServerText::PidFile, bytes);
        state.write_pid();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            std::process::id().to_string()
        );
        state.unlink_pid();
        assert!(!path.exists());
    }
}
