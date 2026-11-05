//! The server's Rust-owned state (`Server::state_`: the round-robin counter and the
//! command id counter) and the operations over the rest of `Server` that
//! `master.cc` kept: worker scheduling, commands, the DTLS accept, the document
//! root, pipe sockets, worker counters, aborts, connection iteration, sends, timers
//! and the startup error message.
use crate::base_factory::{SEND_DATA, SendData, sends_here};
use crate::ops::{
    FIELD_TASK_IPC_MODE, FIELD_TASK_WORKER_NUM, FIELD_WORKER_NUM, LOG_ST_DTLS_BIND,
    LOG_ST_DTLS_CONNECT, LOG_ST_ROOT_MISSING, LOG_ST_ROOT_TOO_LONG, OP_FIELD, OP_IS_STARTED,
    OP_MAXFD, OP_MINFD, OP_MODE, OP_RT_HAS_PIPE_COMMAND, OP_SET_MAXFD, OP_SET_WARNING,
    OP_ST_ABORT_COUNT, OP_ST_CMD_ADD, OP_ST_CMD_EXISTS, OP_ST_COUNT_SEND, OP_ST_DTLS_ADD,
    OP_ST_DTLS_ASSERT, OP_ST_DTLS_BIND, OP_ST_DTLS_CLEANUP, OP_ST_DTLS_CONNECT, OP_ST_DTLS_PEER,
    OP_ST_DTLS_SESSION, OP_ST_DTLS_SOCKET, OP_ST_DTLS_TYPE, OP_ST_DTLS_V6ONLY, OP_ST_FINISH,
    OP_ST_GS_CONCURRENCY, OP_ST_GS_CONCURRENCY_SUB, OP_ST_GS_CONCURRENCY_ZERO, OP_ST_INIT_PIPE,
    OP_ST_PIPE_BIND, OP_ST_PIPE_COMMAND_NEW, OP_ST_PIPE_FD, OP_ST_REACTOR_CLOSE, OP_ST_ROOT_PATH,
    OP_ST_SET_ROOT, OP_ST_SOCK_FREE, OP_ST_SOCK_OBJECT, OP_ST_TIMER, OP_ST_TIMER_DEL,
    OP_ST_VALID_CONN, OP_ST_VISIT, OP_ST_W_CONCURRENCY, OP_ST_W_CONCURRENCY_CAS,
    OP_ST_W_RESET_COUNTS, OP_WORKER_IDLE, TRACE_ST_SCHEDULE,
};
#[cfg(test)]
use crate::ops::{OP_WORKER_CONCURRENCY, OP_WORKER_COROUTINES};
use crate::send::send_to_connection;
use crate::{Host, MODE_BASE, MODE_PROCESS, SW_OK};
use core::ffi::{c_char, c_int};
use core::sync::atomic::{AtomicI32, AtomicPtr, AtomicU32, Ordering};
use std::cell::RefCell;
use std::ffi::{CStr, CString};

/// Owned by `Server::state_`, from the constructor to the destructor. Reactor threads
/// share it, so each counter is atomic.
pub struct ServerState {
    /// `worker_round_id`: the next worker `DISPATCH_ROUND` and idle dispatch try.
    round: AtomicU32,
    /// `command_current_id`: the id the next command gets, from 1.
    next_command: AtomicI32,
    config: AtomicPtr<crate::config::ServerConfig>,
    tables: AtomicPtr<crate::records::ServerTables>,
    pub(crate) static_config:
        std::sync::OnceLock<std::sync::Mutex<crate::static_config::StaticConfig>>,
    pub(crate) commands: std::sync::Mutex<crate::commands::Commands>,
}

impl Default for ServerState {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerState {
    pub(crate) const fn new() -> Self {
        Self {
            round: AtomicU32::new(0),
            next_command: AtomicI32::new(1),
            config: AtomicPtr::new(core::ptr::null_mut()),
            tables: AtomicPtr::new(core::ptr::null_mut()),
            static_config: std::sync::OnceLock::new(),
            commands: std::sync::Mutex::new(crate::commands::Commands::new()),
        }
    }

    pub(crate) fn config(&self) -> *mut crate::config::ServerConfig {
        self.config.load(Ordering::Relaxed)
    }

    pub(crate) fn tables(&self) -> *mut crate::records::ServerTables {
        self.tables.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    pub(crate) fn with_tables(tables: crate::records::ServerTables) -> Self {
        let state = Self::new();
        state
            .tables
            .store(Box::into_raw(Box::new(tables)), Ordering::Relaxed);
        state
    }

    /// The round before this call; it wraps as `sw_atomic_fetch_add` does.
    pub(crate) fn next_round(&self) -> u32 {
        self.round.fetch_add(1, Ordering::SeqCst)
    }
}

/// # Safety
/// The result is freed once with `swr_v1_server_state_free`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_server_state_new() -> *mut ServerState {
    let state = ServerState::new();
    let config = crate::config::ServerConfig {
        worker_registry: swoole_rs_corebase::worker_registry::swr_v1_process_worker_registry_new()
            .cast(),
        ..crate::config::ServerConfig::default()
    };
    state
        .config
        .store(Box::into_raw(Box::new(config)), Ordering::Relaxed);
    state.tables.store(
        Box::into_raw(Box::new(crate::records::ServerTables::default())),
        Ordering::Relaxed,
    );
    Box::into_raw(Box::new(state))
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

impl Drop for ServerState {
    fn drop(&mut self) {
        let tables = *self.tables.get_mut();
        if !tables.is_null() {
            // SAFETY: this state owns its unique table-view box; the shared arena is a loan.
            drop(unsafe { Box::from_raw(tables) });
        }
        let config = *self.config.get_mut();
        if !config.is_null() {
            // SAFETY: this state owns the unique configuration allocation made in state_new;
            // native reference views have ended before the base owner frees the state.
            drop(unsafe { Box::from_raw(config) });
        }
    }
}

thread_local! {
    static ERROR_TEXT: RefCell<CString> = RefCell::new(CString::default());
}

/// `Server::get_startup_error_message`: the last error message, or the last error
/// code as text when there is no message. The text lives until the thread's next call.
///
/// # Safety
/// `message` is a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_server_startup_error(
    message: *const c_char,
    code: c_int,
) -> *const c_char {
    // SAFETY: the caller passes a terminated string.
    if !unsafe { CStr::from_ptr(message) }.is_empty() || code <= 0 {
        return message;
    }
    ERROR_TEXT.with(|text| {
        let mut text = text.borrow_mut();
        *text = CString::new(code.to_string()).unwrap_or_default();
        text.as_ptr()
    })
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
    let round = h.state()?.round.load(Ordering::SeqCst);
    h.op(TRACE_ST_SCHEDULE, u64::from(key) | (u64::from(round) << 32))?;
    Ok(i64::from(key))
}

/// `Server::add_command`: before start, a new name, and (outside base mode) a command
/// pipe; the command gets the next id.
fn add_command<H: Host>(h: &mut H) -> crate::Effect<bool> {
    if h.is(OP_IS_STARTED)? || h.is(OP_ST_CMD_EXISTS)? {
        return Ok(false);
    }
    if h.get(OP_MODE)? != MODE_BASE
        && !h.is(OP_RT_HAS_PIPE_COMMAND)?
        && !h.is(OP_ST_PIPE_COMMAND_NEW)?
    {
        return Ok(false);
    }
    let id = h.state()?.next_command.fetch_add(1, Ordering::SeqCst);
    h.op(OP_ST_CMD_ADD, u64::from(id.cast_unsigned()))?;
    Ok(true)
}

/// `SocketType`.
const SOCK_UDP: u64 = 2;
const SOCK_UDP6: u64 = 4;

/// `Server::accept_dtls_connection`: a socket bound to the port and connected to the
/// peer, registered as a connection with a DTLS session; on failure everything made
/// is undone.
fn accept_dtls<H: Host>(h: &mut H) -> crate::Effect<bool> {
    if !h.is(OP_ST_DTLS_SOCKET)? {
        return Ok(false);
    }
    if !matches!(h.get(OP_ST_DTLS_TYPE)?, SOCK_UDP | SOCK_UDP6) {
        h.get(OP_ST_DTLS_ASSERT)?;
    }
    let mut added = false;
    let ok = 'setup: {
        if !h.is(OP_ST_DTLS_BIND)? {
            h.get(LOG_ST_DTLS_BIND)?;
            break 'setup false;
        }
        h.get(OP_ST_DTLS_V6ONLY)?;
        if !h.is(OP_ST_DTLS_CONNECT)? {
            h.get(LOG_ST_DTLS_CONNECT)?;
            break 'setup false;
        }
        h.get(OP_ST_DTLS_PEER)?;
        if !h.is(OP_ST_DTLS_ADD)? {
            break 'setup false;
        }
        added = true;
        h.is(OP_ST_DTLS_SESSION)?
    };
    if !ok {
        h.op(OP_ST_DTLS_CLEANUP, u64::from(added))?;
    }
    Ok(ok)
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

/// `Server::reset_worker_counter`: the worker's concurrency leaves the server's (never
/// below 0) and its counters restart.
fn reset_worker_counter<H: Host>(h: &mut H) -> crate::Effect<()> {
    let value = h.get(OP_ST_W_CONCURRENCY)?;
    if value > 0 && h.op(OP_ST_W_CONCURRENCY_CAS, value)? == value {
        h.op(OP_ST_GS_CONCURRENCY_SUB, value)?;
        let now = u32::try_from(h.get(OP_ST_GS_CONCURRENCY)? & 0xffff_ffff).unwrap_or(0);
        if now.cast_signed() < 0 {
            h.get(OP_ST_GS_CONCURRENCY_ZERO)?;
        }
    }
    h.get(OP_ST_W_RESET_COUNTS)?;
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

/// `Server::foreach_connection`: each valid connection from the lowest to the highest fd.
fn foreach_connection<H: Host>(h: &mut H) -> crate::Effect<()> {
    let (min, max) = (h.signed(OP_MINFD)?, h.signed(OP_MAXFD)?);
    for fd in min..=max {
        if h.op(OP_ST_VALID_CONN, fd.cast_unsigned())? != 0 {
            h.op(OP_ST_VISIT, fd.cast_unsigned())?;
        }
    }
    Ok(())
}

/// `Server::send`: through the factory, then the server's, port's and worker's counts.
/// A base or thread mode worker that owns the session sends it here, as
/// `BaseFactory::finish` would, without a second trip through native code.
fn send<H: Host>(h: &mut H, data: *const SendData) -> crate::Effect<bool> {
    // SAFETY: native code lends the live `SendData` it built for the call.
    let here = h.get(OP_MODE)? != MODE_PROCESS && sends_here(h, unsafe { &*data })?;
    let sent = if here {
        send_to_connection(h, u64::from(SEND_DATA))? == SW_OK
    } else {
        h.is(OP_ST_FINISH)?
    };
    if !sent {
        return Ok(false);
    }
    h.get(OP_ST_COUNT_SEND)?;
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

/// The operations of `swr_v1_server_run` this module runs.
pub(crate) fn run<H: Host>(h: &mut H, operation: u32, arg: u64) -> crate::Effect<Option<i64>> {
    use crate::ops::{
        RUN_ST_ABORT, RUN_ST_ADD_COMMAND, RUN_ST_CLEAR_TIMER, RUN_ST_DOCUMENT_ROOT,
        RUN_ST_DTLS_ACCEPT, RUN_ST_FOREACH, RUN_ST_INIT_PIPE_SOCKETS, RUN_ST_RESET_COUNTER,
        RUN_ST_SEND, RUN_ST_STORE_PIPE_FD,
    };
    Ok(Some(match operation {
        RUN_ST_ADD_COMMAND => i64::from(add_command(h)?),
        RUN_ST_DTLS_ACCEPT => i64::from(accept_dtls(h)?),
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
        RUN_ST_RESET_COUNTER => {
            reset_worker_counter(h)?;
            0
        }
        RUN_ST_ABORT => {
            abort_connection(h)?;
            0
        }
        RUN_ST_FOREACH => {
            foreach_connection(h)?;
            0
        }
        RUN_ST_SEND => i64::from(send(
            h,
            core::ptr::with_exposed_provenance(usize::try_from(arg).unwrap_or(0)),
        )?),
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
        assert_eq!(busy.args(TRACE_ST_SCHEDULE), vec![2 | (6 << 32)]);
    }

    #[test]
    fn commands_need_a_pipe_outside_base_mode() {
        let mut base = Script::with(&[(OP_MODE, MODE_BASE)]);
        assert!(add_command(&mut base).unwrap());
        assert!(add_command(&mut base).unwrap());
        assert_eq!(base.args(OP_ST_CMD_ADD), vec![1, 2], "ids count from 1");
        assert_eq!(base.called(OP_ST_PIPE_COMMAND_NEW), 0);

        let mut process = Script::with(&[(OP_MODE, MODE_PROCESS)]);
        assert!(!add_command(&mut process).unwrap(), "the pipe failed");
        assert_eq!(process.called(OP_ST_CMD_ADD), 0);
        let mut process = Script::with(&[(OP_MODE, MODE_PROCESS), (OP_RT_HAS_PIPE_COMMAND, 1)]);
        assert!(add_command(&mut process).unwrap());
        assert_eq!(process.called(OP_ST_PIPE_COMMAND_NEW), 0, "made once");

        for refused in [OP_IS_STARTED, OP_ST_CMD_EXISTS] {
            let mut s = Script::with(&[(OP_MODE, MODE_BASE), (refused, 1)]);
            assert!(!add_command(&mut s).unwrap());
            assert_eq!(s.called(OP_ST_CMD_ADD), 0);
        }
    }

    #[test]
    fn dtls_accept_undoes_what_it_made() {
        let ok = [
            (OP_ST_DTLS_SOCKET, 1),
            (OP_ST_DTLS_TYPE, SOCK_UDP),
            (OP_ST_DTLS_BIND, 1),
            (OP_ST_DTLS_CONNECT, 1),
            (OP_ST_DTLS_ADD, 1),
            (OP_ST_DTLS_SESSION, 1),
        ];
        let mut s = Script::with(&ok);
        assert!(accept_dtls(&mut s).unwrap());
        assert_eq!(s.called(OP_ST_DTLS_CLEANUP), 0);
        assert_eq!(s.called(OP_ST_DTLS_ASSERT), 0);

        let mut s = Script::with(&ok);
        s.facts.insert(OP_ST_DTLS_BIND, 0);
        assert!(!accept_dtls(&mut s).unwrap());
        assert_eq!(s.called(LOG_ST_DTLS_BIND), 1);
        assert_eq!(s.called(OP_ST_DTLS_CONNECT), 0);
        assert_eq!(s.args(OP_ST_DTLS_CLEANUP), vec![0]);

        let mut s = Script::with(&ok);
        s.facts.insert(OP_ST_DTLS_SESSION, 0);
        assert!(!accept_dtls(&mut s).unwrap());
        assert_eq!(
            s.args(OP_ST_DTLS_CLEANUP),
            vec![1],
            "the connection is cleared"
        );

        let mut s = Script::with(&ok);
        s.facts.insert(OP_ST_DTLS_CONNECT, 0);
        assert!(!accept_dtls(&mut s).unwrap());
        assert_eq!(s.called(LOG_ST_DTLS_CONNECT), 1);
        assert_eq!(s.args(OP_ST_DTLS_CLEANUP), vec![0]);

        let mut s = Script::with(&ok);
        s.facts.insert(OP_ST_DTLS_TYPE, 1);
        accept_dtls(&mut s).unwrap();
        assert_eq!(s.called(OP_ST_DTLS_ASSERT), 1, "only UDP ports");

        let mut s = Script::default();
        assert!(!accept_dtls(&mut s).unwrap());
        assert_eq!(s.called(OP_ST_DTLS_CLEANUP), 0, "no socket to free");
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
    fn worker_counter_reset_clamps_the_server_concurrency() {
        let mut s = Script::with(&[
            (OP_ST_W_CONCURRENCY, 3),
            (OP_ST_W_CONCURRENCY_CAS, 3),
            (OP_ST_GS_CONCURRENCY, u64::from(u32::MAX)),
        ]);
        reset_worker_counter(&mut s).unwrap();
        assert_eq!(s.args(OP_ST_GS_CONCURRENCY_SUB), vec![3]);
        assert_eq!(s.called(OP_ST_GS_CONCURRENCY_ZERO), 1);
        assert_eq!(s.called(OP_ST_W_RESET_COUNTS), 1);

        let mut raced = Script::with(&[(OP_ST_W_CONCURRENCY, 3), (OP_ST_W_CONCURRENCY_CAS, 2)]);
        reset_worker_counter(&mut raced).unwrap();
        assert_eq!(raced.called(OP_ST_GS_CONCURRENCY_SUB), 0);
        assert_eq!(raced.called(OP_ST_W_RESET_COUNTS), 1);

        let mut idle = Script::default();
        reset_worker_counter(&mut idle).unwrap();
        assert_eq!(idle.called(OP_ST_W_CONCURRENCY_CAS), 0);
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

        let mut s =
            Script::with(&[(OP_MINFD, 3), (OP_MAXFD, 6)]).queue(OP_ST_VALID_CONN, &[1, 0, 1, 1]);
        foreach_connection(&mut s).unwrap();
        assert_eq!(s.args(OP_ST_VISIT), vec![3, 5, 6]);

        let data = four_bytes();
        let process = |facts: &[(u32, u64)]| {
            let mut s = Script::with(&[(OP_MODE, MODE_PROCESS)]);
            s.facts.extend(facts.iter().copied());
            s
        };
        let mut s = process(&[(OP_ST_FINISH, 1)]);
        assert!(send(&mut s, &raw const data).unwrap());
        assert_eq!(s.called(OP_ST_COUNT_SEND), 1);
        let mut s = process(&[]);
        assert!(!send(&mut s, &raw const data).unwrap());
        assert_eq!(s.called(OP_ST_COUNT_SEND), 0);
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
        assert!(send(&mut s, &raw const data).unwrap());
        assert_eq!(
            (
                s.called(OP_ST_FINISH),
                s.called(OP_SC_SEND),
                s.called(OP_ST_COUNT_SEND)
            ),
            (0, 1, 1)
        );
        let mut s = Script::default();
        assert!(!send(&mut s, &raw const data).unwrap(), "no session");
        assert_eq!(s.called(OP_ST_COUNT_SEND), 0);
    }

    #[test]
    fn clear_timer_deletes_armed_timers() {
        let mut s = Script::default().queue(OP_ST_TIMER, &[1, 0, 1]);
        clear_timer(&mut s).unwrap();
        assert_eq!(s.args(OP_ST_TIMER_DEL), vec![0, 2]);
    }

    #[test]
    fn startup_error_falls_back_to_the_code() {
        let text = |message: &CStr, code| {
            // SAFETY: a terminated message.
            let out = unsafe { swr_v1_server_startup_error(message.as_ptr(), code) };
            // SAFETY: the message or this thread's text.
            unsafe { CStr::from_ptr(out) }.to_owned()
        };
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
        unsafe { swr_v1_server_state_free(state) };
    }
}
