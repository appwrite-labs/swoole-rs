//! The server's Rust-owned state (`Server::state_`: the round-robin counter and the
//! command id counter) and the operations over the rest of `Server` that
//! `master.cc` kept: worker scheduling, commands, the document root, pipe sockets, worker counters, aborts, connection iteration, sends, timers
//! and the startup error message.
use crate::base_factory::{SEND_DATA, SendData, sends_here};
use crate::ops::{
    FIELD_TASK_IPC_MODE, FIELD_TASK_WORKER_NUM, FIELD_WORKER_NUM, LOG_ST_ROOT_MISSING,
    LOG_ST_ROOT_TOO_LONG, OP_FIELD, OP_MAXFD, OP_MODE, OP_SET_MAXFD, OP_SET_WARNING,
    OP_ST_ABORT_COUNT, OP_ST_FINISH, OP_ST_INIT_PIPE, OP_ST_PIPE_BIND, OP_ST_PIPE_FD,
    OP_ST_REACTOR_CLOSE, OP_ST_ROOT_PATH, OP_ST_SET_ROOT, OP_ST_SOCK_FREE, OP_ST_SOCK_OBJECT,
    OP_ST_TIMER, OP_ST_TIMER_DEL, OP_WORKER_IDLE,
};
use crate::ops::{OP_LC_UNLINK_PID, OP_LC_WRITE_PID, OP_LOCK, OP_SET_STOPPING, OP_UNLOCK};
#[cfg(test)]
use crate::ops::{OP_WORKER_CONCURRENCY, OP_WORKER_COROUTINES};
use crate::send::send_to_connection;
use crate::{Host, MODE_PROCESS, SW_OK};
use core::ffi::{c_char, c_int};
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, AtomicU32, Ordering};
use std::ffi::{CStr, CString};
use swoole_rs_corebase::StringSlot;
use swoole_rs_tls::socket_pair::SocketPair;

/// Owned by `Server::state_`, from the constructor to the destructor. Reactor threads
/// share it, so each counter is atomic.
pub struct ServerState {
    /// `worker_round_id`: the next worker `DISPATCH_ROUND` and idle dispatch try.
    round: AtomicU32,
    /// `command_current_id`: the id the next command gets, from 1.
    pub(crate) next_command: AtomicI32,
    config: AtomicPtr<crate::config::ServerConfig>,
    tables: AtomicPtr<crate::records::ServerTables>,
    pub(crate) static_config:
        std::sync::OnceLock<std::sync::Mutex<crate::static_config::StaticConfig>>,
    pub(crate) commands: std::sync::Mutex<crate::commands::Commands>,
    /// `Server::ports`, published to `ServerTables::ports` on each push.
    /// The ports are native's to delete, in `~Server`.
    ports: std::sync::Mutex<Vec<AtomicPtr<core::ffi::c_void>>>,
    /// The on* members and `dispatch_func` (`callbacks.rs`).
    pub(crate) callbacks: crate::callbacks::ServerCallbacks,
    /// `upload_tmp_dir`, the HTTP server's directory for uploaded files
    /// (`None`: `/tmp`), as `set()` gave it.
    upload_tmp_dir: std::sync::Mutex<Option<Box<[u8]>>>,
    /// `chroot_`, `user_`, `group_`, `pid_file` and `document_root`, in `TEXT_*`
    /// order; native views them as `RustStringView`s.
    texts: [StringSlot; TEXT_COUNT],
    /// `Server::lock_`, the connection list's fd range lock: whether it is held,
    /// and how many threads wait for it.
    locked: (std::sync::Mutex<(bool, u32)>, std::sync::Condvar),
    /// The reactor threads and the heartbeat thread, by `start_thread`'s `arg`.
    threads: std::sync::Mutex<Vec<(u64, std::thread::JoinHandle<()>)>>,
    /// The interface addresses `Server::create` first read (`local_addr`); none before.
    pub(crate) local_addrs: std::sync::OnceLock<crate::local_addr::LocalAddrs>,
    /// `Server::task_notify_pipes`: a blocking `Pipe` per event worker, which
    /// `task_sync` waits on; `swr_v1_server_delete` closes them, as `~Server` did.
    #[expect(
        clippy::vec_box,
        reason = "`task_pipe` hands out addresses that outlive the lock"
    )]
    task_pipes: std::sync::Mutex<Vec<Box<SocketPair>>>,
    /// `Server::worker_pipes`: each event worker's `UnixSocket`, which
    /// `swr_v1_server_delete` closes, as `~Server` did.
    #[cfg_attr(
        not(all(feature = "upstream-symbols", not(test))),
        expect(dead_code, reason = "only the hybrid makes pipes")
    )]
    #[expect(clippy::vec_box, reason = "a worker holds its pair's address")]
    worker_pipes: std::sync::Mutex<Vec<Box<SocketPair>>>,
    /// A worker thread asked to shut down: the thread manager respawns no worker.
    pub(crate) stopping: AtomicBool,
}

/// A native thread entry: `void (*)(void *serv, uint64_t arg)`.
type ThreadEntry = unsafe extern "C" fn(*mut core::ffi::c_void, u64);

/// `ServerState::texts`' slots (`SWR_SERVER_TEXT_*`).
pub(crate) const TEXT_PID_FILE: usize = 3;
pub(crate) const TEXT_DOCUMENT_ROOT: usize = 4;
const TEXT_COUNT: usize = 5;

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
    fn with(config: crate::config::ServerConfig, tables: crate::records::ServerTables) -> Self {
        Self {
            round: AtomicU32::new(0),
            next_command: AtomicI32::new(1),
            config: AtomicPtr::new(Box::into_raw(Box::new(config))),
            tables: AtomicPtr::new(Box::into_raw(Box::new(tables))),
            static_config: std::sync::OnceLock::new(),
            commands: std::sync::Mutex::new(crate::commands::Commands::new()),
            ports: std::sync::Mutex::new(Vec::new()),
            callbacks: crate::callbacks::ServerCallbacks::new(),
            upload_tmp_dir: std::sync::Mutex::new(None),
            texts: [const { StringSlot::new() }; TEXT_COUNT],
            locked: (std::sync::Mutex::new((false, 0)), std::sync::Condvar::new()),
            threads: std::sync::Mutex::new(Vec::new()),
            local_addrs: std::sync::OnceLock::new(),
            task_pipes: std::sync::Mutex::new(Vec::new()),
            worker_pipes: std::sync::Mutex::new(Vec::new()),
            stopping: AtomicBool::new(false),
        }
    }

    /// `new UnixSocket(true, SOCK_DGRAM)` for `worker`'s pipes: false, closing it,
    /// when the pair isn't ready.
    #[cfg(all(feature = "upstream-symbols", not(test)))]
    pub(crate) fn push_worker_pipe(
        &self,
        worker: &mut swoole_rs_process::worker_record::WorkerRecord,
    ) -> bool {
        let mut pair = Box::new(swoole_rs_tls::socket_pair::unix_socket(
            true,
            libc::SOCK_DGRAM,
        ));
        let (master, end) = (pair.master_socket, pair.worker_socket);
        if master.is_null() || end.is_null() {
            swoole_rs_tls::socket_pair::drop(&mut pair);
            return false;
        }
        let mut pipes = lock(&self.worker_pipes);
        pipes.push(pair);
        // The box keeps the pair at this address while the list owns it.
        let pipe = pipes
            .last_mut()
            .map_or(core::ptr::null_mut(), |pair| &raw mut **pair);
        worker.pipe_master = master.cast();
        worker.pipe_worker = end.cast();
        worker.pipe_object = pipe.cast();
        true
    }

    #[cfg(all(feature = "upstream-symbols", not(test)))]
    /// `task_notify_pipes.emplace_back(new Pipe(true))`: false, closing it, when
    /// the pipe isn't ready.
    pub(crate) fn push_task_pipe(&self) -> bool {
        let mut storage = Box::new(core::mem::MaybeUninit::<SocketPair>::uninit());
        swoole_rs_tls::socket_pair::pipe_new(&mut storage, true);
        // SAFETY: the constructor initialised it.
        let mut pair = unsafe { storage.assume_init() };
        let ready = !pair.master_socket.is_null() && !pair.worker_socket.is_null();
        if !ready {
            swoole_rs_tls::socket_pair::drop(&mut pair);
            return false;
        }
        lock(&self.task_pipes).push(pair);
        true
    }

    /// `~Server`'s close of `task_notify_pipes` and `worker_pipes`. Not the state's
    /// drop: an archive user that never runs a server links no C++ for `Socket::free`.
    #[cfg(all(feature = "upstream-symbols", not(test)))]
    pub(crate) fn close_pipes(&self) {
        let task = core::mem::take(&mut *lock(&self.task_pipes));
        let worker = core::mem::take(&mut *lock(&self.worker_pipes));
        for mut pipe in task.into_iter().chain(worker) {
            swoole_rs_tls::socket_pair::drop(&mut pipe);
        }
    }

    /// `task_notify_pipes.at(index)`, or null past the end.
    pub(crate) fn task_pipe(&self, index: usize) -> *mut SocketPair {
        lock(&self.task_pipes)
            .get_mut(index)
            .map_or(core::ptr::null_mut(), |pipe| &raw mut **pipe)
    }

    /// One of `texts`' slots.
    pub(crate) fn text_slot(&self, index: usize) -> Option<&StringSlot> {
        self.texts.get(index)
    }

    /// The bytes of one of `texts`, which no other thread sets meanwhile.
    pub(crate) fn text(&self, index: usize) -> &[u8] {
        self.text_slot(index).map_or(&[], StringSlot::bytes)
    }

    /// `swoole_set_isolation(group_, user_, chroot_)`: a worker drops to the
    /// configured group and user, then chroots.
    #[cfg(all(feature = "upstream-symbols", not(test)))]
    pub(crate) fn set_isolation(&self) {
        let text = |index| CString::new(self.text(index)).unwrap_or_default();
        let (root, user, group) = (text(0), text(1), text(2));
        swoole_rs_ossupport::set_isolation(&group, &user, &root);
    }

    /// `Server::lock()`: blocks until this thread holds the lock.
    pub(crate) fn lock(&self) {
        let (lock, released) = &self.locked;
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
        let (lock, released) = &self.locked;
        let mut lock = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        lock.0 = false;
        if lock.1 > 0 {
            released.notify_one();
        }
    }

    /// Starts a thread running `entry(serv, arg)`, which `take_thread(arg)` hands back.
    /// A null `entry` starts nothing; a thread that can't start aborts, as
    /// `std::thread`'s uncaught `system_error` does.
    pub(crate) fn start_thread(&self, entry: u64, serv: u64, arg: u64) {
        let entry = core::ptr::with_exposed_provenance_mut::<core::ffi::c_void>(
            usize::try_from(entry).unwrap_or(0),
        );
        if entry.is_null() {
            return;
        }
        // SAFETY: native code answered with a `void (*)(void *, uint64_t)`.
        let entry = unsafe { core::mem::transmute::<*mut core::ffi::c_void, ThreadEntry>(entry) };
        let serv = usize::try_from(serv).unwrap_or(0);
        let thread = std::thread::Builder::new().spawn(move || {
            // SAFETY: the server outlives its threads, which it joins before it goes.
            unsafe { entry(core::ptr::with_exposed_provenance_mut(serv), arg) };
        });
        let Ok(thread) = thread else {
            std::process::abort();
        };
        self.threads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((arg, thread));
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

    /// `Server::ports.push_back(port)`; the port's `object_id` is the new count.
    ///
    /// # Safety
    /// `port` is a live `ListenPort`, owned by this server from now on, and no
    /// other thread reads the list during the push.
    pub(crate) unsafe fn push_port(&self, port: *mut core::ffi::c_void) {
        let mut ports = self
            .ports
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ports.push(AtomicPtr::new(port));
        let count = u32::try_from(ports.len()).unwrap_or(u32::MAX);
        // SAFETY: a live port's settings, per the caller.
        unsafe { crate::port_config::of(port) }.object_id =
            u16::try_from(count).unwrap_or(u16::MAX);
        // SAFETY: the state's table view; no reader runs during the push, per the caller.
        let tables = unsafe { &mut *self.tables() };
        // `AtomicPtr` has the layout of the pointer it holds.
        tables.ports = crate::records::PortList {
            items: ports.as_ptr().cast(),
            count,
        };
    }

    /// `serv->running` alone: no reference to the rest of the record, which an
    /// op context on another thread may hold as `&mut ServerConfig`.
    fn running_flag(&self) -> &AtomicBool {
        // SAFETY: a field of the box this state owns until it drops.
        unsafe { &(*self.config()).running }
    }

    /// `serv->running`.
    pub(crate) fn running(&self) -> bool {
        self.running_flag().load(Ordering::Acquire)
    }

    /// `serv->running = running`, seen by every thread's next [`Self::running`].
    pub(crate) fn set_running(&self, running: bool) {
        self.running_flag().store(running, Ordering::Release);
    }

    pub(crate) fn config(&self) -> *mut crate::config::ServerConfig {
        self.config.load(Ordering::Relaxed)
    }

    pub(crate) fn tables(&self) -> *mut crate::records::ServerTables {
        self.tables.load(Ordering::Relaxed)
    }

    /// The configuration, which native code writes only before the server
    /// starts or under the server's own synchronization.
    pub(crate) fn config_ref(&self) -> &crate::config::ServerConfig {
        // SAFETY: the box this state owns until it drops.
        unsafe { &*self.config() }
    }

    /// The table view, written as `config_ref` says.
    pub(crate) fn tables_ref(&self) -> &crate::records::ServerTables {
        // SAFETY: the box this state owns until it drops.
        unsafe { &*self.tables() }
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

    /// `upload_tmp_dir`'s bytes, valid until the next `set_upload_tmp_dir`.
    pub(crate) fn upload_tmp_dir(&self) -> &[u8] {
        let dir = self
            .upload_tmp_dir
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(dir) = dir.as_deref() else {
            return b"/tmp";
        };
        // SAFETY: the boxed bytes stay put until `set_upload_tmp_dir` replaces
        // them, which only `set()` does, before any request reads them.
        unsafe { core::slice::from_raw_parts(dir.as_ptr(), dir.len()) }
    }

    /// The round before this call; it wraps as `sw_atomic_fetch_add` does.
    pub(crate) fn next_round(&self) -> u32 {
        self.round.fetch_add(1, Ordering::SeqCst)
    }
}

/// `Server`'s text member `index` (`SWR_SERVER_TEXT_*`), or null past the last;
/// the slot lives as long as `state`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_server_text_slot(state: &ServerState, index: usize) -> *const StringSlot {
    state
        .text_slot(index)
        .map_or(core::ptr::null(), core::ptr::from_ref)
}

/// `text.c_str()`: the bytes up to the first NUL.
fn c_path(text: &[u8]) -> CString {
    let end = text
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(text.len());
    CString::new(text.get(..end).unwrap_or_default()).unwrap_or_default()
}

/// Answers the ops over `texts` and the lock, which need only the state.
///
/// # Safety
/// `state` is live; `OP_ST_SET_ROOT`'s `arg` addresses a NUL-terminated path.
#[inline(always)]
pub(crate) unsafe fn native_fact(state: &ServerState, op: u32, arg: u64) -> Option<u64> {
    match op {
        OP_SET_STOPPING => state.stopping.store(true, Ordering::Release),
        OP_LOCK => state.lock(),
        OP_UNLOCK => state.unlock(),
        OP_ST_SET_ROOT => {
            let path = core::ptr::with_exposed_provenance::<c_char>(usize::try_from(arg).ok()?);
            // SAFETY: per the caller.
            let path = unsafe { CStr::from_ptr(path) }.to_bytes();
            // SAFETY: a slot of this state, set only while the server is configured.
            unsafe { state.texts[TEXT_DOCUMENT_ROOT].set_shared(path) };
        }
        OP_LC_WRITE_PID => {
            let file = state.text(TEXT_PID_FILE);
            if !file.is_empty() {
                let file = c_path(file);
                let pid = std::process::id().cast_signed().to_string();
                swoole_rs_ossupport::file_put_contents(&file, pid.as_bytes());
            }
        }
        OP_LC_UNLINK_PID => {
            let file = state.text(TEXT_PID_FILE);
            if !file.is_empty() {
                swoole_rs_corebase::sys::unlink(&c_path(file));
            }
        }
        _ => return None,
    }
    Some(0)
}

/// # Safety
/// The result is freed once, as a `Box`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_server_state_new() -> *mut ServerState {
    let (config, tables) = fresh_records();
    boxed(config, tables)
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

/// The state of a server with these records, freed by `swr_v1_server_state_free`.
pub(crate) fn boxed(
    config: crate::config::ServerConfig,
    tables: crate::records::ServerTables,
) -> *mut ServerState {
    Box::into_raw(Box::new(ServerState::with(config, tables)))
}

/// # Safety
/// `state` is null or came from `swr_v1_server_state_new` and is not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_state_free(state: *mut ServerState) {
    if !state.is_null() {
        // SAFETY: the caller transfers the box back.
        drop(unsafe { Box::from_raw(state) });
    }
}

#[cfg(test)]
pub(crate) use swr_v1_server_state_free as free_state;

/// A lock of one of the state's lists, which a panic can't leave inconsistent.
fn lock<T>(list: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    list.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Drop for ServerState {
    fn drop(&mut self) {
        // SAFETY: this state owns its unique table-view box; the shared arena is a loan.
        drop(unsafe { Box::from_raw(*self.tables.get_mut()) });
        // SAFETY: this state owns its unique configuration box; native reference
        // views have ended before the base owner frees the state.
        drop(unsafe { Box::from_raw(*self.config.get_mut()) });
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

fn field<H: Host>(h: &mut H, id: u32) -> crate::Effect<u64> {
    h.op(OP_FIELD, u64::from(id))
}

fn worker_count<H: Host>(h: &mut H) -> crate::Effect<u64> {
    field(h, FIELD_WORKER_NUM)
}

/// `get_lowest_load_worker_id` (`OP_WORKER_COROUTINES`) and
/// `get_lowest_concurrent_worker_id` (`OP_WORKER_CONCURRENCY`): the first worker
/// with the fewest.
pub(crate) fn lowest<H: Host>(h: &mut H, metric: u32) -> crate::Effect<i64> {
    let workers = worker_count(h)?;
    let mut lowest = 0;
    let mut min = h.op(metric, 0)?;
    for id in 1..workers {
        let value = h.op(metric, id)?;
        if value < min {
            min = value;
            lowest = id;
        }
    }
    Ok(lowest.cast_signed())
}

/// `get_idle_worker_id`: up to `worker_num + 1` round-robin tries for an idle worker,
/// else the last one tried, with the scheduler warning raised.
pub(crate) fn idle_worker<H: Host>(h: &mut H) -> crate::Effect<i64> {
    let workers = u32::try_from(worker_count(h)?).unwrap_or(u32::MAX).max(1);
    let mut key = 0;
    let mut found = false;
    for _ in 0..=workers {
        key = h.state()?.next_round() % workers;
        if h.op(OP_WORKER_IDLE, u64::from(key))? != 0 {
            found = true;
            break;
        }
    }
    if !found {
        h.get(OP_SET_WARNING)?;
    }
    Ok(i64::from(key))
}

/// `Server::set_document_root(path)`: the path's real path, if it exists.
fn document_root<H: Host>(h: &mut H, path: &[u8]) -> crate::Effect<bool> {
    if path.len() > usize::try_from(libc::PATH_MAX).unwrap_or(usize::MAX) {
        h.get(LOG_ST_ROOT_TOO_LONG)?;
        return Ok(false);
    }
    // `realpath` reads the path up to its first NUL.
    let path = path.split(|byte| *byte == 0).next().unwrap_or_default();
    let real = {
        use std::os::unix::ffi::OsStrExt;
        std::fs::canonicalize(std::ffi::OsStr::from_bytes(path)).ok()
    }
    .and_then(|real| {
        use std::os::unix::ffi::OsStringExt;
        CString::new(real.into_os_string().into_vec()).ok()
    });
    let Some(real) = real else {
        h.get(LOG_ST_ROOT_MISSING)?;
        return Ok(false);
    };
    h.op(OP_ST_SET_ROOT, real.as_ptr().expose_provenance() as u64)?;
    Ok(true)
}

/// `Server::store_pipe_fd`: both ends of a worker pipe map to the pipe, and the
/// highest fd grows to cover them.
fn store_pipe_fd<H: Host>(h: &mut H) -> crate::Effect<()> {
    let master = h.signed(OP_ST_PIPE_FD)?;
    let worker = h.op(OP_ST_PIPE_FD, 1)?.cast_signed();
    h.op(OP_ST_PIPE_BIND, master.cast_unsigned())?;
    h.op(OP_ST_PIPE_BIND, worker.cast_unsigned())?;
    for fd in [master, worker] {
        if fd > h.signed(OP_MAXFD)? {
            h.op(OP_SET_MAXFD, fd.cast_unsigned())?;
        }
    }
    Ok(())
}

/// `Server::TASK_IPC_UNIXSOCK`.
const TASK_IPC_UNIXSOCK: u64 = 1;

/// `Server::init_pipe_sockets`: the pipes of every event worker, and of the task
/// workers when they talk over unix sockets.
fn init_pipe_sockets<H: Host>(h: &mut H) -> crate::Effect<()> {
    let workers = worker_count(h)?;
    let total = workers + field(h, FIELD_TASK_WORKER_NUM)?;
    let task_pipes = field(h, FIELD_TASK_IPC_MODE)? == TASK_IPC_UNIXSOCK;
    for id in 0..total {
        if id >= workers && !task_pipes {
            continue;
        }
        h.op(OP_ST_INIT_PIPE, id)?;
    }
    Ok(())
}

/// `Server::abort_connection`: counted, then closed through the reactor when it is
/// a connection, else freed.
fn abort_connection<H: Host>(h: &mut H) -> crate::Effect<()> {
    h.get(OP_ST_ABORT_COUNT)?;
    if h.is(OP_ST_SOCK_OBJECT)? {
        h.get(OP_ST_REACTOR_CLOSE)?;
    } else {
        h.get(OP_ST_SOCK_FREE)?;
    }
    Ok(())
}

/// `Server::send`: through the factory, then the server's, port's and worker's counts.
/// A base or thread mode worker that owns the session sends it here, as
/// `BaseFactory::finish` would, without a second trip through native code.
pub(crate) fn send<H: Host>(h: &mut H, send: &SendData) -> crate::Effect<bool> {
    let here = h.get(OP_MODE)? != MODE_PROCESS && sends_here(h, send)?;
    let sent = if here {
        match h.send_now(send)? {
            Some(sent) if sent == i64::from(send.info.len) => true,
            first => send_to_connection(h, u64::from(SEND_DATA), first)? == SW_OK,
        }
    } else {
        h.is(OP_ST_FINISH)?
    };
    if !sent {
        return Ok(false);
    }
    crate::shared::count_send(h.state()?, send.info.fd, send.info.len);
    Ok(true)
}

/// `master_timer`, `heartbeat_timer`, `enable_accept_timer`.
const TIMERS: u64 = 3;

/// `Server::clear_timer`: each armed timer is deleted and forgotten.
pub(crate) fn clear_timer<H: Host>(h: &mut H) -> crate::Effect<()> {
    for timer in 0..TIMERS {
        if h.op(OP_ST_TIMER, timer)? != 0 {
            h.op(OP_ST_TIMER_DEL, timer)?;
        }
    }
    Ok(())
}

/// The operations of `crate::run` this module runs.
pub(crate) fn run<H: Host>(h: &mut H, operation: u32, arg: u64) -> crate::Effect<Option<i64>> {
    use crate::ops::{
        RUN_ST_ABORT, RUN_ST_ADD_COMMAND, RUN_ST_CLEAR_TIMER, RUN_ST_COMMAND, RUN_ST_DOCUMENT_ROOT,
        RUN_ST_INIT_PIPE_SOCKETS, RUN_ST_STORE_PIPE_FD,
    };
    Ok(Some(match operation {
        RUN_ST_ADD_COMMAND => {
            let add = core::ptr::with_exposed_provenance::<crate::commands::AddCommand>(
                usize::try_from(arg).unwrap_or(0),
            );
            // SAFETY: `swr_v1_server_add_command` lends its arguments for the call.
            i64::from(crate::commands::add_command(h, unsafe { &*add })?)
        }
        RUN_ST_COMMAND => {
            let send = core::ptr::with_exposed_provenance::<crate::commands::SendCommand>(
                usize::try_from(arg).unwrap_or(0),
            );
            // SAFETY: `swr_v1_server_command` lends its arguments for the call.
            i64::from(crate::commands::command(h, unsafe { &*send })?)
        }
        RUN_ST_DOCUMENT_ROOT => {
            let address = usize::try_from(h.get(OP_ST_ROOT_PATH)?).unwrap_or(0);
            let len = usize::try_from(arg).unwrap_or(0);
            // SAFETY: native passes the path's `len` bytes, live for the call.
            let path = unsafe {
                core::slice::from_raw_parts(core::ptr::with_exposed_provenance::<u8>(address), len)
            };
            i64::from(document_root(h, path)?)
        }
        RUN_ST_STORE_PIPE_FD => {
            store_pipe_fd(h)?;
            0
        }
        RUN_ST_INIT_PIPE_SOCKETS => {
            init_pipe_sockets(h)?;
            0
        }
        RUN_ST_ABORT => {
            abort_connection(h)?;
            0
        }
        RUN_ST_CLEAR_TIMER => {
            clear_timer(h)?;
            0
        }
        _ => return Ok(None),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MODE_PROCESS;
    use crate::script::Script;

    fn workers(n: u64) -> Script {
        Script::with(&[(OP_FIELD, n)])
    }

    #[test]
    fn lowest_takes_the_first_minimum() {
        let mut s = workers(4).queue(OP_WORKER_COROUTINES, &[5, 3, 3, 9]);
        assert_eq!(lowest(&mut s, OP_WORKER_COROUTINES).unwrap(), 1);
        assert_eq!(s.args(OP_WORKER_COROUTINES), vec![0, 1, 2, 3]);
        let mut s = workers(1);
        assert_eq!(
            lowest(&mut s, OP_WORKER_CONCURRENCY).unwrap(),
            0,
            "a single worker"
        );
    }

    #[test]
    fn idle_worker_tries_round_robin_then_warns() {
        let mut s = workers(3).queue(OP_WORKER_IDLE, &[0, 1]);
        assert_eq!(idle_worker(&mut s).unwrap(), 1);
        assert_eq!(s.args(OP_WORKER_IDLE), vec![0, 1]);
        assert_eq!(s.called(OP_SET_WARNING), 0);
        // The round counter continues where the last schedule left it.
        let mut busy = workers(3);
        busy.state = ServerState::new();
        busy.state.round.store(2, Ordering::SeqCst);
        assert_eq!(
            idle_worker(&mut busy).unwrap(),
            2,
            "the last of worker_num + 1 tries"
        );
        assert_eq!(busy.args(OP_WORKER_IDLE), vec![2, 0, 1, 2]);
        assert_eq!(busy.called(OP_SET_WARNING), 1);
    }

    #[test]
    fn document_root_is_the_real_path() {
        let mut s = Script::default();
        assert!(document_root(&mut s, b"/").unwrap());
        assert_eq!(s.called(OP_ST_SET_ROOT), 1);
        let mut s = Script::default();
        assert!(!document_root(&mut s, b"/does/not/exist/swoole-rs").unwrap());
        assert_eq!(s.called(LOG_ST_ROOT_MISSING), 1);
        assert_eq!(s.called(OP_ST_SET_ROOT), 0);
        let long = vec![b'a'; usize::try_from(libc::PATH_MAX).unwrap() + 1];
        let mut s = Script::default();
        assert!(!document_root(&mut s, &long).unwrap());
        assert_eq!(s.called(LOG_ST_ROOT_TOO_LONG), 1);
    }

    #[test]
    fn pipe_fds_raise_the_highest_fd() {
        let mut s = Script::with(&[(OP_MAXFD, 7)]).queue(OP_ST_PIPE_FD, &[5, 9]);
        store_pipe_fd(&mut s).unwrap();
        assert_eq!(s.args(OP_ST_PIPE_FD), vec![0, 1]);
        assert_eq!(s.args(OP_ST_PIPE_BIND), vec![5, 9]);
        assert_eq!(s.args(OP_SET_MAXFD), vec![9]);
    }

    #[test]
    fn task_pipes_only_over_unix_sockets() {
        let mut s = Script::default().queue(OP_FIELD, &[2, 2, TASK_IPC_UNIXSOCK]);
        init_pipe_sockets(&mut s).unwrap();
        assert_eq!(s.args(OP_ST_INIT_PIPE), vec![0, 1, 2, 3]);
        let mut s = Script::default().queue(OP_FIELD, &[2, 2, 2]);
        init_pipe_sockets(&mut s).unwrap();
        assert_eq!(s.args(OP_ST_INIT_PIPE), vec![0, 1]);
    }

    #[test]
    fn connections_abort_iterate_and_send() {
        let mut s = Script::with(&[(OP_ST_SOCK_OBJECT, 1)]);
        abort_connection(&mut s).unwrap();
        assert_eq!(
            (
                s.called(OP_ST_ABORT_COUNT),
                s.called(OP_ST_REACTOR_CLOSE),
                s.called(OP_ST_SOCK_FREE)
            ),
            (1, 1, 0)
        );
        let mut s = Script::default();
        abort_connection(&mut s).unwrap();
        assert_eq!(s.called(OP_ST_SOCK_FREE), 1);

        let data = four_bytes();
        let process = |facts: &[(u32, u64)]| {
            let mut s = Script::with(&[(OP_MODE, MODE_PROCESS)]);
            s.facts.extend(facts.iter().copied());
            s
        };
        let mut s = process(&[(OP_ST_FINISH, 1)]);
        assert!(send(&mut s, &data).unwrap());
        let mut s = process(&[]);
        assert!(!send(&mut s, &data).unwrap());
    }

    /// A 4-byte send to session 1.
    fn four_bytes() -> SendData {
        SendData {
            info: swoole_rs_corebase::DataHead {
                fd: 1,
                kind: SEND_DATA,
                len: 4,
                ..Default::default()
            },
            data: core::ptr::null(),
        }
    }

    #[test]
    fn base_mode_sends_here_without_the_factory() {
        use crate::ops::{OP_SC_SEND, OP_SC_VERIFY, OP_SC_WRITABLE};
        use crate::send::{DIRECT_SEND, FOUND, OUT_EMPTY};
        let data = four_bytes();
        let mut s = Script::with(&[
            (OP_SC_VERIFY, FOUND | OUT_EMPTY | DIRECT_SEND | (4 << 32)),
            (OP_SC_SEND, 4),
            (OP_SC_WRITABLE, 1),
        ]);
        assert!(send(&mut s, &data).unwrap());
        assert_eq!((s.called(OP_ST_FINISH), s.called(OP_SC_SEND)), (0, 1));
        let mut s = Script::default();
        assert!(!send(&mut s, &data).unwrap(), "no session");
    }

    #[test]
    fn clear_timer_deletes_armed_timers() {
        let mut s = Script::default().queue(OP_ST_TIMER, &[1, 0, 1]);
        clear_timer(&mut s).unwrap();
        assert_eq!(s.args(OP_ST_TIMER_DEL), vec![0, 2]);
    }

    #[test]
    fn startup_error_falls_back_to_the_code() {
        let text = |message: &CStr, code| startup_error(message.to_owned(), code);
        assert_eq!(text(c"bind failed", 98), c"bind failed");
        assert_eq!(text(c"", 1004), c"1004");
        assert_eq!(text(c"", 0), c"");
    }

    #[test]
    fn state_round_trips() {
        let state = swr_v1_server_state_new();
        // SAFETY: a live state.
        assert_eq!(unsafe { &*state }.next_round(), 0);
        // SAFETY: freed once.
        unsafe { free_state(state) };
    }

    #[test]
    fn texts_and_the_lock_are_answered_without_native() {
        let state = ServerState::new();
        let root = c"/srv/www";
        let arg = root.as_ptr().expose_provenance() as u64;
        assert_eq!(unsafe { native_fact(&state, OP_ST_SET_ROOT, arg) }, Some(0));
        assert_eq!(state.text(TEXT_DOCUMENT_ROOT), b"/srv/www");
        assert_eq!(state.text(TEXT_PID_FILE), b"");
        assert!(state.text_slot(TEXT_COUNT).is_none());
        assert_eq!(unsafe { native_fact(&state, OP_SET_MAXFD, 0) }, None);

        let state = std::sync::Arc::new(state);
        assert_eq!(unsafe { native_fact(&state, OP_LOCK, 0) }, Some(0));
        // Two waiters: the unlock wakes one, whose unlock wakes the other.
        let waiters: Vec<_> = (0..2)
            .map(|_| {
                let state = std::sync::Arc::clone(&state);
                std::thread::spawn(move || {
                    state.lock();
                    state.unlock();
                })
            })
            .collect();
        // Both blocked in the wait before the unlock, so it must hand over twice.
        while state.locked.0.lock().unwrap().1 < 2 {
            std::thread::yield_now();
        }
        assert!(
            !waiters.iter().any(std::thread::JoinHandle::is_finished),
            "the lock is held"
        );
        assert_eq!(unsafe { native_fact(&state, OP_UNLOCK, 0) }, Some(0));
        for waiter in waiters {
            waiter.join().unwrap();
        }
        assert_eq!(
            *state.locked.0.lock().unwrap(),
            (false, 0),
            "free, no waiters"
        );
    }

    #[test]
    fn the_pid_file_is_written_then_unlinked() {
        // What swoole_init sets.
        swoole_rs_corebase::swr_v1_core_set_max_file_content(64 * 1024 * 1024);
        let state = ServerState::new();
        // No pid_file: neither op touches the file system.
        assert_eq!(unsafe { native_fact(&state, OP_LC_WRITE_PID, 0) }, Some(0));
        assert_eq!(unsafe { native_fact(&state, OP_LC_UNLINK_PID, 0) }, Some(0));
        let path = std::env::temp_dir().join(format!("swoole-rs-pid-{}", std::process::id()));
        let bytes = path.to_str().unwrap().as_bytes();
        unsafe { state.texts[TEXT_PID_FILE].set_shared(bytes) };
        assert_eq!(unsafe { native_fact(&state, OP_LC_WRITE_PID, 0) }, Some(0));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            std::process::id().to_string()
        );
        assert_eq!(unsafe { native_fact(&state, OP_LC_UNLINK_PID, 0) }, Some(0));
        assert!(!path.exists());
    }
}
