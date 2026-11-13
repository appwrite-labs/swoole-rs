//! The server's Rust-owned state (`Server::state_`: the round-robin counter and the
//! command id counter) and the operations over the rest of `Server` that
//! `master.cc` kept: worker scheduling, commands, the document root, worker counters, aborts, connection iteration, sends, timers
//! and the startup error message.
use core::ffi::c_int;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, AtomicU32, Ordering};
use std::ffi::CString;
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
    /// order. Readers share the value; a write replaces it.
    texts: [std::sync::Mutex<std::sync::Arc<[u8]>>; TEXT_COUNT],
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
pub(crate) type ThreadEntry = fn(crate::view::Server<'_>, u64);

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
            texts: Default::default(),
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
        worker: swoole_rs_process::worker_record::Worker<'_>,
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
        // SAFETY: the two ends of the boxed pair the server's list now owns, which
        // the state keeps for as long as the server has its workers.
        unsafe { worker.set_pipes(pipe.cast(), master.cast(), end.cast()) };
        true
    }

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

    /// Text member `index` (`TEXT_*`), empty past the last.
    pub(crate) fn text(&self, index: usize) -> std::sync::Arc<[u8]> {
        self.texts
            .get(index)
            .map_or_else(Default::default, |text| std::sync::Arc::clone(&lock(text)))
    }

    /// Sets text member `index` (`TEXT_*`); none past the last.
    pub(crate) fn set_text(&self, index: usize, value: &[u8]) {
        if let Some(text) = self.texts.get(index) {
            *lock(text) = value.into();
        }
    }

    /// `Server::set_document_root(path)`: the path's real path, if it exists.
    pub(crate) fn set_document_root(&self, path: &[u8]) -> bool {
        /// `SW_LOG_WARNING`.
        const LOG_WARNING: c_int = 4;
        use std::os::unix::ffi::OsStrExt;
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
        self.set_text(TEXT_DOCUMENT_ROOT, real.as_os_str().as_bytes());
        true
    }

    /// `swoole_set_isolation(group_, user_, chroot_)`: a worker drops to the
    /// configured group and user, then chroots.
    #[cfg(all(feature = "upstream-symbols", not(test)))]
    pub(crate) fn set_isolation(&self) {
        let text = |index| CString::new(&*self.text(index)).unwrap_or_default();
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

    /// Starts a thread of `serv`, whose state this is, running `entry(serv, arg)`;
    /// `take_thread(arg)` hands it back. A thread that can't start aborts, as
    /// `std::thread`'s uncaught `system_error` does.
    pub(crate) fn start_thread(serv: crate::view::Server<'_>, entry: ThreadEntry, arg: u64) {
        let raw = serv.raw().expose_provenance();
        let thread = std::thread::Builder::new().spawn(move || {
            // SAFETY: the live server that started this thread, which outlives it: a
            // server joins the threads its state holds before it goes.
            let serv = unsafe {
                crate::view::Server::from_raw(core::ptr::with_exposed_provenance_mut(raw))
            };
            entry(serv, arg);
        });
        let Ok(thread) = thread else {
            swoole_rs_corebase::fatal("a server thread can't start");
        };
        serv.state()
            .threads
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

    /// `Server::ports.push_back(port)`: the server owns the port from now on, and the
    /// port's `object_id` is the new count.
    ///
    /// # Safety
    /// No other thread reads the list during the push, and no slice of it
    /// ([`crate::records::ServerTables::ports`]) is used after it.
    pub(crate) unsafe fn push_port(
        &self,
        mut port: Box<crate::port_config::PortConfig>,
    ) -> crate::view::Port<'_> {
        let mut ports = self
            .ports
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let count = u32::try_from(ports.len() + 1).unwrap_or(u32::MAX);
        port.object_id = u16::try_from(count).unwrap_or(u16::MAX);
        let port = Box::into_raw(port).cast::<core::ffi::c_void>();
        ports.push(AtomicPtr::new(port));
        // SAFETY: the state's table view; no reader runs during the push, per the caller.
        let tables = unsafe { &mut *self.tables() };
        // `AtomicPtr` has the layout of the pointer it holds.
        tables.ports = crate::records::PortList {
            items: ports.as_ptr().cast(),
            count,
        };
        drop(ports);
        // The port just published, the list's last.
        let kept = self.tables_ref().ports().last().copied();
        kept.unwrap_or_else(|| swoole_rs_corebase::fatal("a pushed port is not listed"))
    }

    /// `serv->running`.
    pub(crate) fn running(&self) -> bool {
        self.config_ref().running.load(Ordering::Acquire)
    }

    /// `serv->running = running`, seen by every thread's next [`Self::running`].
    pub(crate) fn set_running(&self, running: bool) {
        self.config_ref().running.store(running, Ordering::Release);
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
        let file = self.text(TEXT_PID_FILE);
        if !file.is_empty() {
            let pid = std::process::id().cast_signed().to_string();
            swoole_rs_ossupport::file_put_contents(&c_path(&file), pid.as_bytes());
        }
    }

    /// The pid file removed, when there is one.
    pub(crate) fn unlink_pid(&self) {
        let file = self.text(TEXT_PID_FILE);
        if !file.is_empty() {
            swoole_rs_corebase::sys::unlink(&c_path(&file));
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
pub(crate) use tests::{free_state, swr_v1_server_state_new};

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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A state with a new server's records, which [`free_state`] frees.
    pub(crate) fn swr_v1_server_state_new() -> *mut ServerState {
        let (config, tables) = fresh_records();
        Box::into_raw(boxed(config, tables))
    }

    /// # Safety
    /// `state` is null or came from [`swr_v1_server_state_new`] and is not used again.
    pub(crate) unsafe fn free_state(state: *mut ServerState) {
        if !state.is_null() {
            // SAFETY: the caller transfers the box back.
            drop(unsafe { Box::from_raw(state) });
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
            &*state.text(TEXT_DOCUMENT_ROOT),
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
            &*state.text(TEXT_DOCUMENT_ROOT),
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
        let state = swr_v1_server_state_new();
        // SAFETY: a live state.
        assert_eq!(unsafe { &*state }.next_round(), 0);
        // SAFETY: freed once.
        unsafe { free_state(state) };
    }

    #[test]
    fn texts_and_the_lock_are_answered_without_native() {
        let state = ServerState::new();
        state.set_text(TEXT_DOCUMENT_ROOT, b"/srv/www");
        assert_eq!(&*state.text(TEXT_DOCUMENT_ROOT), b"/srv/www");
        assert_eq!(&*state.text(TEXT_PID_FILE), b"");
        assert!(state.text(TEXT_COUNT).is_empty());

        let state = std::sync::Arc::new(state);
        state.lock();
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
        state.unlock();
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
        // No pid_file: neither touches the file system.
        state.write_pid();
        state.unlink_pid();
        let path = std::env::temp_dir().join(format!("swoole-rs-pid-{}", std::process::id()));
        let bytes = path.to_str().unwrap().as_bytes();
        state.set_text(TEXT_PID_FILE, bytes);
        state.write_pid();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            std::process::id().to_string()
        );
        state.unlink_pid();
        assert!(!path.exists());
    }
}
