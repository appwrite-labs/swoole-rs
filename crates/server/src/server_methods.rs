//! The public `Server` methods of `src/server/master.cc`, and the `BaseFactory`
//! methods that were `src/server/base.cc`, that are one operation over a fresh
//! context. Each method fills the context the way its C++ forward did and runs over
//! it in Rust.

use crate::Mode;
use crate::records::Lookup;
use core::ffi::{c_int, c_long};
use std::ffi::CString;
use swoole_rs_corebase::CCast;

use crate::hybrid::hybrid;
use crate::view;
use swoole_rs_corebase::{DataHead, log};

/// `Server::start()`.
#[must_use]
pub fn start(serv: view::Server<'_>) -> c_int {
    crate::startup::start(serv)
}

/// `Server::start_check()`.
#[must_use]
pub fn start_check(serv: view::Server<'_>) -> c_int {
    crate::startup::start_check(serv)
}

/// `Server::store_listen_socket()`.
pub fn store_listen_socket(serv: view::Server<'_>) {
    crate::startup::store_listen_socket(serv);
}

/// `Server::master_signal_handler(signo)`: the signal over `SwooleG.server`.
///
/// # Safety
/// Runs as a signal handler while `SwooleG.server` is live.
pub unsafe extern "C-unwind" fn master_signal_handler(signo: c_int) {
    // SAFETY: as the caller guarantees.
    if let Some(serv) = unsafe { view::Server::current() } {
        crate::control::signal(serv, signo);
    }
}

pub use crate::control::shutdown;

/// `Server::create()`: `SW_ERR` once created, else the hook and then startup's create.
#[must_use]
pub fn create(serv: view::Server<'_>) -> c_int {
    if !serv.config().factory_.is_null() {
        return -1;
    }
    serv.call_hook(view::ServerHook::BeforeCreate);
    crate::startup::create(serv)
}

/// `Server::destroy()`: nothing before the server is created.
pub fn destroy(serv: view::Server<'_>) {
    if !serv.config().factory_.is_null() {
        crate::startup::destroy(serv);
    }
}

/// `delete serv`'s first step: [`destroy`] a server still running in its master,
/// which runs its shutdown callbacks. Nothing the second time.
pub fn destroy_running(serv: view::Server<'_>) {
    let counters = serv.counters();
    let master = std::process::id().cast_signed()
        == counters
            .master_pid
            .load(core::sync::atomic::Ordering::Relaxed);
    if master
        && counters
            .shutdown
            .load(core::sync::atomic::Ordering::Relaxed)
            == 0
    {
        destroy(serv);
    }
}

/// `Server::start_master_thread(reactor)`, on the master's reactor.
#[cfg(all(feature = "upstream-symbols", not(test)))]
#[must_use]
pub(crate) fn start_master_thread(
    serv: view::Server<'_>,
    reactor: crate::reactor_handle::Reactor,
) -> c_int {
    crate::startup::start_master_thread(serv, reactor)
}

/// `Server::init_pipe_sockets(mb)`, once the server has started.
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub(crate) fn init_pipe_sockets(serv: view::Server<'_>, mb: crate::objects::Bus) {
    if !serv.is_started() {
        // upstream's `assert(is_started())`.
        swoole_rs_corebase::fatal("pipe sockets set up before the server started");
    }
    let config = serv.config();
    let workers = config.worker_num.get();
    // Task workers have pipes only when they talk over unix sockets.
    let task_pipes = config.task_ipc() == Some(crate::TaskIpcMode::UnixSock);
    let total = if task_pipes {
        workers + config.task_worker_num.get()
    } else {
        workers
    };
    for id in 0..total {
        let id = u16::try_from(id).unwrap_or(u16::MAX);
        for master in [true, false] {
            if let Some(end) = serv.worker_pipe(id, master) {
                mb.init_pipe_socket(end.fd());
            }
        }
    }
}

/// `Server::connection_incoming(reactor, conn)`, over the port `conn` arrived on:
/// `reactor`, this thread's, takes one of `serv`'s live connections.
#[cfg_attr(
    not(all(feature = "upstream-symbols", not(test))),
    expect(dead_code, reason = "its callers are the hybrid's, outside tests")
)]
pub(crate) fn connection_incoming(
    serv: view::Server<'_>,
    reactor: crate::reactor_handle::Reactor,
    conn: crate::Connection<'_>,
) -> c_int {
    serv.port_by_server_fd(conn.server_fd()).map_or(-1, |port| {
        crate::connection::incoming(serv, reactor, port, conn)
    })
}

/// `Server::accept_connection(reactor, event)`: the master reactor's read handler
/// for listening sockets, over the server the reactor carries.
///
/// Registered only on a server's reactor (its `ptr` is the live `Server`) for its
/// listening sockets.
pub(crate) extern "C" fn accept_connection(
    reactor: swoole_rs_reactor::Running<'_>,
    event: &mut swoole_rs_reactor::Event,
) -> c_int {
    // SAFETY: the server the reactor carries, live while it runs.
    let serv = unsafe { view::Server::from_raw(reactor.object()) };
    crate::connection::accept(
        serv,
        crate::reactor_handle::Reactor::running(reactor),
        event,
    )
}

/// `Server::get_startup_error_message()`.
#[must_use]
pub fn get_startup_error_message() -> CString {
    let message = swoole_rs_error::last_error_message();
    crate::state::startup_error(message, log::swr_v1_core_last_error())
}

pub use crate::connection::{add as add_connection, set_max_connection};

pub use crate::dispatch::{feedback, sendwait};

/// `Server::store_pipe_fd(worker->pipe_object)`, when `worker` has a pipe: both ends
/// map to the pipe, and the highest fd grows to cover them.
pub(crate) fn store_pipe_fd(
    serv: view::Server<'_>,
    worker: swoole_rs_process::worker_record::Worker<'_>,
) {
    let pipe = worker.pipe_object();
    if pipe.is_null() {
        return;
    }
    let gs = serv.counters();
    for master in [true, false] {
        let fd = view::worker_pipe(worker, master).map_or(-1, |end| end.fd());
        if let Some(conn) = serv.state().tables_ref().connection_at(fd) {
            conn.set_object(pipe);
        }
        if fd > gs.max_fd() {
            gs.max_fd.store(fd, core::sync::atomic::Ordering::Relaxed);
        }
    }
}

pub use crate::dispatch::schedule_worker;

/// `Server::get_idle_worker_num()`.
#[must_use]
pub fn get_idle_worker_num(serv: view::Server<'_>) -> u32 {
    crate::dispatch::idle_workers(serv, false)
}

/// `Server::get_idle_task_worker_num()`.
#[must_use]
pub fn get_idle_task_worker_num(serv: view::Server<'_>) -> c_int {
    crate::dispatch::idle_workers(serv, true).cast_signed()
}

/// `Server::get_tasking_num()`.
#[must_use]
pub fn get_tasking_num(serv: view::Server<'_>) -> c_int {
    crate::dispatch::tasking(serv)
}

pub use crate::dispatch::{add_port, add_systemd_socket};

pub use crate::send::{send, send_to_connection};

/// `Server::set_document_root(path)`, before the server starts.
#[expect(
    clippy::must_use_candidate,
    reason = "`swoole_server_set` ignores the result, as upstream's does"
)]
pub fn set_document_root(serv: view::Server<'_>, path: &[u8]) -> bool {
    serv.state().set_document_root(path)
}

/// Arms the master timer on this thread, the master's: each second it runs
/// `Server::timer_callback` over `serv`. The timer's node, or null.
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub(crate) fn master_timer(serv: view::Server<'_>) -> *mut core::ffi::c_void {
    let serv = serv.raw();
    swoole_rs_timer::add_fn(1000, true, move || {
        // SAFETY: the server that armed its master timer, on its own thread; it
        // outlives its timers, which `clear_timer` deletes before it goes.
        let serv = unsafe { view::Server::from_raw(serv) };
        crate::control::master_timer(serv);
    })
}

/// `Server::has_kernel_nobufs_error(session_id)`: whether a send to the session's
/// socket, or in process mode to its reactor pipe, hit `ENOBUFS` since the last
/// call (which clears it). Called after every `Server::send`, so it reads the
/// records directly rather than through the op runner.
#[must_use]
pub fn has_kernel_nobufs_error(serv: view::Server<'_>, session_id: c_long) -> bool {
    let state = serv.state();
    // `get_connection(int)` of the session id, as master.cc did.
    let fd = session_id.c_cast::<c_int>();
    let Some(conn) = state.connection(c_long::from(fd), Lookup::ByFd) else {
        return false;
    };
    let Some(socket) = conn.socket() else {
        return false;
    };
    let config = state.config_ref();
    if config.mode_ != Mode::Process {
        return socket.take_kernel_nobufs();
    }
    // `serv->get_reactor_pipe_socket(session_id, conn->reactor_id)`.
    let pipes = i64::from(state.tables_ref().reactor_pipe_num);
    let index = session_id % pipes;
    let id = i64::from(conn.reactor_id()) + index * i64::from(config.reactor_num.get());
    let id = u16::try_from(id).unwrap_or(u16::MAX);
    serv.worker_pipe(id, false)
        .is_some_and(|pipe| pipe.take_kernel_nobufs())
}

pub use crate::dispatch::sendfile;

/// `serv->reactor_thread_barrier.wait()`.
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub(crate) fn reactor_barrier(serv: view::Server<'_>) {
    swoole_rs_locks::barrier_wait(crate::server_object::barrier(serv, false));
}

/// `Server::get_package_max_length(conn)`, over the fd of one of `serv`'s connections.
#[must_use]
pub fn package_max_length(serv: view::Server<'_>, fd: c_int) -> u32 {
    // The connection's live port, as in C++, which doesn't check it either.
    serv.port_of_fd(fd)
        .map_or(0, |port| port.protocol.get().package_max_length)
}

/// `Server::CLOSE_RESET`, `Server::CLOSE_ACTIVELY`.
const CLOSE_RESET: c_int = 1 << 1;
const CLOSE_ACTIVELY: c_int = 1 << 2;

/// `Server::close(session_id, reset)`: the factory ends the session, actively.
#[expect(
    clippy::must_use_candidate,
    reason = "callers ignore the result where upstream's do"
)]
pub fn close(serv: view::Server<'_>, session_id: c_long, reset: bool) -> bool {
    let flags = CLOSE_ACTIVELY | if reset { CLOSE_RESET } else { 0 };
    hybrid().factory_end(serv, session_id, flags)
}

/// `Server::notify(conn, event)`: the factory hears `event` for `conn`.
#[expect(
    clippy::must_use_candidate,
    reason = "callers ignore the result where upstream's do"
)]
pub fn notify(serv: view::Server<'_>, conn: crate::Connection<'_>, event: c_int) -> bool {
    let mut head = DataHead {
        fd: conn.fd().into(),
        reactor_id: conn.reactor_id().into(),
        kind: event.c_cast::<u8>(),
        server_fd: conn.server_fd().c_cast::<u16>(),
        ..DataHead::default()
    };
    hybrid().factory_notify(serv, &mut head)
}

/// `SW_SERVER_EVENT_PIPE_MESSAGE` (server.cc asserts it).
const EVENT_PIPE_MESSAGE: u8 = 10;

/// `Server::send_pipe_message(worker_id, msg)`: `msg` to that worker, through the master.
pub fn send_pipe_message(
    serv: view::Server<'_>,
    worker_id: u32,
    msg: &mut swoole_rs_process::task_file::TaskBuffer,
) -> bool {
    msg.set_kind(EVENT_PIPE_MESSAGE);
    hybrid().pipe_to_worker(serv, worker_id, msg)
}

/// `Server::add_command(name, accepted_process_types, handler)`: whether the
/// command joined. The server keeps the handler for its life.
pub fn swr_v1_server_add_command(
    serv: view::Server<'_>,
    name: &[u8],
    accepted_types: c_int,
    handler: crate::CommandHandler,
) -> bool {
    let add = crate::commands::AddCommand {
        name,
        accepted_types,
        handler,
    };
    crate::commands::add_command(serv, &add)
}

/// `Server::command(process_id, process_type, name, message, callback)`: whether
/// the request went. Its reply reaches `callback` once, perhaps before this returns.
pub fn swr_v1_server_command(
    serv: view::Server<'_>,
    process_id: u32,
    process_type: c_int,
    name: &[u8],
    message: &[u8],
    callback: crate::CommandCallback,
) -> bool {
    let send = crate::commands::SendCommand {
        serv,
        process_id,
        process_type,
        name,
        message,
        callback,
    };
    crate::commands::command(&send)
}

/// `Server::call_command_callback(request_id, message)`: the reply to `request_id`
/// reaches its callback, or the server logs an invalid command result.
pub fn swr_v1_server_command_result(serv: view::Server<'_>, request_id: i64, message: &[u8]) {
    crate::commands::command_result(serv.state(), request_id, message);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::ConnectionRecord;
    use core::ffi::c_void;

    /// A server whose state and config are null: the methods under test read neither.
    static FAKE: [usize; 2] = [0; 2];

    fn fake_serv() -> *mut c_void {
        (&raw const FAKE).cast_mut().cast()
    }

    fn server() -> view::Server<'static> {
        unsafe { view::Server::from_raw(fake_serv()) }
    }

    #[test]
    fn close_notify_and_pipe_messages() {
        use crate::hybrid::tests::{Call, calls};
        calls();
        let serv = server();
        assert!(close(serv, 21, true), "close result");
        close(serv, 21, false);
        let active = CLOSE_ACTIVELY;
        assert_eq!(
            calls(),
            [Call::End(21, active | CLOSE_RESET), Call::End(21, active)],
            "close ends the session actively, reset or not"
        );

        // SAFETY: a zeroed record is a valid connection.
        let mut conn: ConnectionRecord = unsafe { core::mem::zeroed() };
        conn.fd = 22;
        conn.reactor_id = 3;
        conn.server_fd = 0x1_0005;
        assert!(notify(serv, (&mut conn).into(), 0x104), "notify result");
        assert_eq!(
            calls(),
            [Call::Notify(22, 3, 4, 5)],
            "notify head keeps the low bits"
        );

        let mut message = swoole_rs_process::task_file::TaskBuffer::default();
        assert!(message.pack(b"abc"), "packs inline");
        assert!(send_pipe_message(serv, 2, &mut message), "pipe result");
        assert_eq!(
            calls(),
            [Call::Pipe(2, EVENT_PIPE_MESSAGE, 43)],
            "the packed message goes to the worker as a pipe message, with its size"
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no shared mappings")]
    fn kernel_nobufs_takes_the_sockets_flag_or_in_process_mode_the_reactor_pipes() {
        use crate::records::ConnectionRecord;
        use crate::records::tests::naming;
        use swoole_rs_process::worker_record::WorkerRecord;
        use swoole_rs_reactor::SocketRecord;

        // SAFETY: plain records, for which zero is valid.
        let (mut sock, mut pipe): (SocketRecord, SocketRecord) =
            unsafe { (core::mem::zeroed(), core::mem::zeroed()) };
        sock.kernel_nobufs = true;
        pipe.kernel_nobufs = true;
        // SAFETY: as above.
        let mut connections: [ConnectionRecord; 4] = unsafe { core::mem::zeroed() };
        connections[3].socket = (&raw mut sock).cast();
        connections[3].reactor_id = 1;
        // SAFETY: as above.
        let mut workers: [WorkerRecord; 4] = unsafe { core::mem::zeroed() };
        // Session 3 on reactor 1 of 2, over 2 pipes: worker 1 + (3 % 2) * 2.
        workers[3].pipe_worker = (&raw mut pipe).cast();
        let state = crate::swr_v1_server_state_new();
        // SAFETY: a fresh state; the records above outlive every read below.
        let fields = unsafe {
            let config = (*state).config();
            // The whole `serv->gs`, as `ServerObject` maps it: the worker lookup reads its
            // zeroed event pool, past the counters.
            (*config).gs =
                swoole_rs_memory::members::Shm::calloc(1, size_of::<crate::shared::ServerShared>());
            (*config).workers = naming(&mut workers);
            (*config).worker_num.set(4);
            (*config).reactor_num.set(2);
            let tables = &mut *(*state).tables();
            tables.connection_list = naming(&mut connections);
            tables.max_connection = 4;
            tables.reactor_pipe_num = 2;
            [state.cast::<c_void>(), config.cast::<c_void>()]
        };
        // SAFETY: laid out as a Server's head, over the live state and config.
        let serv = unsafe { view::Server::from_raw((&raw const fields).cast_mut().cast()) };
        let set_mode = |mode: Mode| {
            // SAFETY: the state's config, which no reference holds across this write.
            unsafe { (*(*state).config()).mode_ = mode };
        };

        set_mode(Mode::Base);
        assert!(!has_kernel_nobufs_error(serv, 2), "no socket");
        assert!(!has_kernel_nobufs_error(serv, 4), "past the table");
        assert!(has_kernel_nobufs_error(serv, 3), "the socket's flag");
        assert!(!has_kernel_nobufs_error(serv, 3), "taken");
        assert!(pipe.kernel_nobufs, "base mode leaves the pipe");

        sock.kernel_nobufs = true;
        set_mode(Mode::Process);
        assert!(has_kernel_nobufs_error(serv, 3), "the reactor pipe's flag");
        assert!(!has_kernel_nobufs_error(serv, 3), "taken");
        assert!(sock.kernel_nobufs, "process mode leaves the socket");

        // SAFETY: allocated above, not used again.
        unsafe {
            (*(*state).config()).gs.free();
            crate::swr_v1_server_state_free(state);
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn pipe_fds_map_to_the_pipe_and_raise_the_highest_fd() {
        use core::sync::atomic::Ordering::Relaxed;
        use swoole_rs_process::worker_record::{Worker, WorkerRecord};
        use swoole_rs_reactor::SocketRecord;
        let live = crate::fixture::LiveServer::new();
        live.counters().max_fd.store(7, Relaxed);
        // SAFETY: plain records, for which zero is valid.
        let (mut master, mut end, mut record): (SocketRecord, SocketRecord, WorkerRecord) =
            unsafe { core::mem::zeroed() };
        master.fd = 5;
        end.fd = 9;
        // SAFETY: a zeroed worker record, live for the test.
        let worker = unsafe { Worker::new(&raw mut record) };
        store_pipe_fd(live.server(), worker);
        assert_eq!(
            live.counters().max_fd.load(Relaxed),
            7,
            "no pipe, no change"
        );

        let mut pipe = 0_u8;
        let pipe: *mut c_void = (&raw mut pipe).cast();
        worker.set_pipe_master((&raw mut master).cast());
        worker.set_pipe_worker((&raw mut end).cast());
        worker.set_pipe_object(pipe);
        store_pipe_fd(live.server(), worker);
        let tables = live.state().tables_ref();
        for fd in [5, 9] {
            assert_eq!(
                tables.connection_at(fd).map(crate::Connection::object),
                Some(pipe)
            );
        }
        assert_eq!(live.counters().max_fd.load(Relaxed), 9, "the highest end");
    }
}
