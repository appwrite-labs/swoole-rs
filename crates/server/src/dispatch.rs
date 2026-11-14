//! Choosing a worker, sends from workers, and listen ports.
use crate::base_factory::SendData;
use crate::callbacks::CallbackKind;
use crate::port_config::{PortConfig, methods};
use crate::reactor_entry::RecvData;
use crate::records::Lookup;
use crate::view::{Port, Server};
use crate::{DispatchMode, Mode};
use core::ffi::{CStr, c_char, c_int, c_long};
use core::sync::atomic::Ordering::{Relaxed, SeqCst};
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::ServerEvent;
use swoole_rs_reactor::socket_upstream::socket_set_fd_option;
use swoole_rs_utilities::misc::swoole_hash_php;

/// `DISPATCH_RESULT_USERFUNC_FALLBACK`.
const USERFUNC_FALLBACK: c_int = -3;

/// `SocketType`, `SW_SOCK_SSL`, `SW_MAX_LISTEN_PORT`, `SW_SYSTEMD_FDS_START`.
const SOCK_UDP: c_int = 2;
const SOCK_UDP6: c_int = 4;
const SOCK_UNIX_STREAM: c_int = 5;
const SOCK_UNIX_DGRAM: c_int = 6;
const SOCK_SSL: c_int = 1 << 9;
const MAX_LISTEN_PORT: usize = 60000;
const SYSTEMD_FDS_START: c_int = 3;

/// `Server::schedule_worker(fd, data)`: the user's dispatch function, else the dispatch
/// mode. `send` is the data the worker takes, if any.
pub fn schedule_worker(serv: Server<'_>, fd: c_int, send: Option<&SendData>) -> c_int {
    let state = serv.state();
    let key = c_long::from(fd);
    let dispatch_func = state.callbacks.slot(CallbackKind::DispatchFunc);
    if dispatch_func.is_set() {
        // SAFETY: null, or the table's live slot of the fd, read for the call.
        let conn = unsafe { state.connection_ptr(key, Lookup::ByFd).as_ref() };
        let recv = send.map(|send| core::ptr::from_ref(send).cast::<RecvData>());
        // SAFETY: a `SendData` is laid out as the `RecvData` the callback reads: its head
        // and data pointer.
        let recv = recv.map(|recv| unsafe { &*recv });
        let id = state.callbacks.dispatch(serv, conn, recv);
        if id != USERFUNC_FALLBACK {
            return id;
        }
    }
    let config = serv.config();
    let fd_key = fd.cast_unsigned();
    let workers = config.worker_num.get().max(1);
    let conn = || state.connection(key, Lookup::ByFd);
    let key = match config.dispatch() {
        Some(DispatchMode::Round) => state.next_round(),
        Some(DispatchMode::FdMod) => fd_key,
        Some(DispatchMode::IpMod) => ip_key(state, key, send),
        Some(DispatchMode::UidMod) => match conn().map_or(0, crate::Connection::uid) {
            0 => fd_key,
            uid => uid,
        },
        Some(DispatchMode::CoConnLb) => {
            let Some(conn) = conn() else {
                return (fd_key % workers).cast_signed();
            };
            let worker = conn.worker_id();
            if worker >= 0 {
                return worker;
            }
            let lowest = lowest(serv, Load::Coroutines);
            conn.set_worker_id(lowest);
            return lowest;
        }
        Some(DispatchMode::CoReqLb) => return lowest(serv, Load::Coroutines),
        Some(DispatchMode::ConcurrentLb) => return lowest(serv, Load::Concurrency),
        // Deliver to an idle worker.
        _ => return idle_worker(serv),
    };
    (key % workers).cast_signed()
}

/// `get_ip_key`-style hash of the sender: its IPv4 address, else `swoole_hash_php` of
/// its IPv6 one. The connection's peer, or with none, the datagram's sender.
fn ip_key(state: &crate::ServerState, fd: c_long, send: Option<&SendData>) -> u32 {
    let addr = if let Some(conn) = state.connection(fd, Lookup::ByFd) {
        conn.info()
    } else {
        // No connection: the data is a datagram.
        let Some(send) = send else { return 0 };
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "the payload is a message bus buffer, aligned for its `DgramPacket`"
        )]
        let packet = send.data.cast::<crate::master_ops::DgramPacket>();
        // SAFETY: the datagram's payload starts with its `DgramPacket`.
        unsafe { (*packet).socket_addr }
    };
    if swoole_rs_reactor::type_is(addr.kind, swoole_rs_reactor::CLASS_INET4) {
        u32::from_be(addr.addr.v4().sin_addr.s_addr)
    } else {
        let v6 = (&raw const addr.addr).cast::<c_char>();
        // SAFETY: the address's storage holds a whole `sockaddr_in6`.
        let hash = unsafe { swoole_hash_php(v6, size_of::<libc::sockaddr_in6>()) };
        hash.c_cast::<u32>()
    }
}

/// What [`lowest`] compares.
#[derive(Clone, Copy)]
enum Load {
    Coroutines,
    Concurrency,
}

/// `get_lowest_load_worker_id` and `get_lowest_concurrent_worker_id`: the first event
/// worker with the fewest.
fn lowest(serv: Server<'_>, load: Load) -> c_int {
    let workers = serv.config().worker_num.get();
    let of = |id: u32| {
        let worker = u16::try_from(id).ok().and_then(|id| serv.worker_record(id));
        worker.map_or(0, |worker| match load {
            Load::Coroutines => worker.coroutine_num(),
            Load::Concurrency => worker.concurrency_count() as usize,
        })
    };
    (0..workers.max(1))
        .min_by_key(|&id| of(id))
        .unwrap_or(0)
        .cast_signed()
}

/// `get_idle_worker_id`: up to `worker_num + 1` round-robin tries for an idle worker,
/// else the last one tried, with the scheduler warning raised.
fn idle_worker(serv: Server<'_>) -> c_int {
    let workers = serv.config().worker_num.get().max(1);
    let mut key = 0;
    for _ in 0..=workers {
        key = serv.state().next_round() % workers;
        if is_idle(serv, key) {
            return key.cast_signed();
        }
    }
    serv.config().scheduler_warning.store(true, Relaxed);
    key.cast_signed()
}

fn is_idle(serv: Server<'_>, id: u32) -> bool {
    let worker = u16::try_from(id).ok().and_then(|id| serv.worker_record(id));
    worker.is_some_and(swoole_rs_process::worker_record::Worker::is_idle)
}

/// `Server::get_idle_worker_num` (event workers) and `get_idle_task_worker_num`.
#[must_use]
pub(crate) fn idle_workers(serv: Server<'_>, task: bool) -> u32 {
    let config = serv.config();
    let workers = config.worker_num.get();
    let range = if task {
        workers..workers.saturating_add(config.task_worker_num.get())
    } else {
        0..workers
    };
    let idle = range.filter(|&id| is_idle(serv, id)).count();
    u32::try_from(idle).unwrap_or(u32::MAX)
}

/// `Server::get_tasking_num`: a negative count is reset to 0.
#[must_use]
pub(crate) fn tasking(serv: Server<'_>) -> c_int {
    let tasking = &serv.counters().tasking_num;
    let count = tasking.load(SeqCst).cast_signed();
    if count < 0 {
        tasking.store(0, SeqCst);
        return 0;
    }
    count
}

/// `Server::feedback(conn, event)`: a connection event from a worker to the master,
/// through a reactor thread in process mode, else sent here.
#[must_use]
pub fn feedback(serv: Server<'_>, conn: crate::Connection<'_>, event: ServerEvent) -> bool {
    let info = swoole_rs_corebase::DataHead {
        fd: conn.session_id(),
        reactor_id: conn.reactor_id().into(),
        kind: event.code(),
        ..Default::default()
    };
    if serv.config().mode_ == Mode::Process {
        return crate::hybrid::hybrid().to_reactor_thread(serv, &info) > 0;
    }
    let sent = crate::send::send_event_to_connection(serv, &info, &[]);
    i64::from(sent) == crate::SW_OK
}

/// `Server::sendwait(session_id, data, length)`: a blocking send of all of `data` to
/// the session's socket.
#[must_use]
pub fn sendwait(serv: Server<'_>, session_id: c_long, data: &[u8]) -> bool {
    let Some(conn) = serv.state().connection(session_id, Lookup::VerifiedTls) else {
        let length = data.len();
        let m =
            format_args!("send {length} byte failed, because session#{session_id} is not exists");
        swoole_rs_corebase::log::error_log(
            SW_LOG_TRACE,
            swoole_rs_error::ErrorCode::SW_ERROR_SESSION_NOT_EXIST as c_int,
            c"Server::sendwait",
            m,
        );
        return false;
    };
    let sent = conn.socket().map_or(-1, |socket| {
        swoole_rs_tls::upstream::send_sync(socket, data, 0)
    });
    usize::try_from(sent) == Ok(data.len())
}

/// `SW_LOG_TRACE`.
const SW_LOG_TRACE: c_int = swoole_rs_corebase::log::LogLevel::SW_LOG_TRACE as c_int;

const IPC_BUFFER_SIZE: usize = swoole_rs_corebase::constants::SW_IPC_BUFFER_SIZE;
/// `sizeof(SendfileTask)`: its `off_t offset` and `size_t length`, then the name.
const SENDFILE_TASK_SIZE: usize = 16;
/// `SW_LOG_WARNING` and `SW_LOG_ERROR`.
const SW_LOG_WARNING: c_int = swoole_rs_corebase::log::LogLevel::SW_LOG_WARNING as c_int;
const SW_LOG_ERROR: c_int = swoole_rs_corebase::log::LogLevel::SW_LOG_ERROR as c_int;

/// `Server::sendfile`'s buffer, aligned for the `SendfileTask` it starts with.
#[repr(C, align(8))]
struct SendfileBuffer([u8; IPC_BUFFER_SIZE]);

fn log_error(
    level: c_int,
    code: swoole_rs_error::ErrorCode,
    origin: &CStr,
    m: core::fmt::Arguments<'_>,
) {
    swoole_rs_corebase::log::error_log(level, code as c_int, origin, m);
}

fn sendfile_error(level: c_int, code: swoole_rs_error::ErrorCode, m: core::fmt::Arguments<'_>) {
    log_error(level, code, c"Server::sendfile", m);
}

/// `Server::sendfile(session_id, file, l_file, offset, length)`, `l_file` being the
/// name's length narrowed to `uint32_t`, as upstream's callers do: checks the session
/// and the file, then hands the `SendfileTask` to the factory. The task copies the name
/// up to its first NUL (`swoole_strlcpy`).
#[expect(
    clippy::must_use_candidate,
    reason = "callers ignore the result where upstream's do"
)]
pub fn sendfile(
    serv: Server<'_>,
    session_id: c_long,
    file: &[u8],
    offset: libc::off_t,
    length: usize,
) -> bool {
    use swoole_rs_error::ErrorCode as E;
    if session_id <= 0 {
        let m = format_args!("invalid fd[{session_id}]");
        sendfile_error(SW_LOG_WARNING, E::SW_ERROR_SESSION_INVALID_ID, m);
        return false;
    }
    let master = swoole_rs_process::worker_record::WorkerType::Master;
    if swoole_rs_process::methods::worker_type() == Some(master) {
        let m = format_args!("can't send data to the connections in master process");
        sendfile_error(SW_LOG_ERROR, E::SW_ERROR_SERVER_SEND_IN_MASTER, m);
        return false;
    }
    let l_file = file.len().c_cast::<u32>();
    let name = file.split(|&byte| byte == 0).next().unwrap_or_default();
    let max = IPC_BUFFER_SIZE - SENDFILE_TASK_SIZE - 1;
    if l_file as usize > max {
        // `%.8s`: at most 8 bytes of the name.
        let head = String::from_utf8_lossy(name.get(..8).unwrap_or(name));
        let m = format_args!(
            "sendfile name[{head}...] length {l_file} is exceed the max name len {max}"
        );
        sendfile_error(SW_LOG_WARNING, E::SW_ERROR_NAME_TOO_LONG, m);
        return false;
    }
    let mut buffer = SendfileBuffer([0; IPC_BUFFER_SIZE]);
    let (task, field) = buffer.0.split_at_mut(SENDFILE_TASK_SIZE);
    let (head, tail) = task.split_at_mut(8);
    head.copy_from_slice(&offset.to_ne_bytes());
    tail.copy_from_slice(&length.to_ne_bytes());
    // `swoole_strlcpy(req->filename, file, ...)`: the name, cut to leave room for its
    // NUL, which the zeroed buffer already holds. One copy: a loop over the bytes costs
    // the static handler about 800 instructions a file.
    let (copied, _) = name.split_at(name.len().min(field.len() - 1));
    let (target, _) = field.split_at_mut(copied.len());
    target.copy_from_slice(copied);
    let path = CStr::from_bytes_until_nul(field).unwrap_or_default();
    let Some(st) = swoole_rs_corebase::sys::stat(path) else {
        let m = format_args!("stat({}) failed", path.to_string_lossy());
        sendfile_error(SW_LOG_WARNING, E::SW_ERROR_SYSTEM_CALL_FAIL, m);
        return false;
    };
    if st.st_mode & libc::S_IFMT != libc::S_IFREG {
        let path = path.to_string_lossy();
        let m = format_args!("the path[{path}] given is not a regular file");
        sendfile_error(SW_LOG_WARNING, E::SW_ERROR_SERVER_IS_NOT_REGULAR_FILE, m);
        return false;
    }
    if st.st_size <= offset {
        let m = format_args!("file[offset={offset}] is empty");
        sendfile_error(SW_LOG_WARNING, E::SW_ERROR_SYSTEM_CALL_FAIL, m);
        return false;
    }
    let mut send = SendData {
        info: swoole_rs_corebase::DataHead {
            fd: session_id,
            kind: ServerEvent::SendFile.code(),
            len: SENDFILE_TASK_SIZE.c_cast::<u32>() + l_file + 1,
            ..Default::default()
        },
        data: buffer.0.as_ptr().cast(),
    };
    crate::hybrid::hybrid().factory_finish(serv, &mut send)
}

/// `SW_HOST_MAXSIZE`: `sizeof(sockaddr_un::sun_path)`.
fn host_maxsize() -> usize {
    // `sun_path` is the struct's last field, with no padding after its bytes.
    size_of::<libc::sockaddr_un>() - core::mem::offset_of!(libc::sockaddr_un, sun_path)
}

/// `Server::add_port(type, host, port)`: validates, creates the listen socket, and
/// keeps the port; `None` with the error logged or set.
#[must_use]
pub fn add_port<'a>(
    serv: Server<'a>,
    socket_type: c_int,
    host: &CStr,
    number: c_int,
) -> Option<Port<'a>> {
    use swoole_rs_error::ErrorCode as E;
    let fail = |code, m: core::fmt::Arguments<'_>| {
        log_error(SW_LOG_ERROR, code, c"Server::add_port", m);
        None
    };
    if !serv.state().config_ref().factory_.is_null() {
        let m = format_args!("must add port before server is created");
        return fail(E::SW_ERROR_WRONG_OPERATION, m);
    }
    if serv.ports().len() >= MAX_LISTEN_PORT {
        let m = format_args!("up to 60000 listening ports are allowed");
        return fail(E::SW_ERROR_SERVER_TOO_MANY_LISTEN_PORT, m);
    }
    let local = matches!(socket_type, SOCK_UNIX_STREAM | SOCK_UNIX_DGRAM);
    if !local && !(0..=65535).contains(&number) {
        let m = format_args!("invalid port [{number}]");
        return fail(E::SW_ERROR_SERVER_INVALID_LISTEN_PORT, m);
    }
    if host.count_bytes() + 1 > host_maxsize() {
        let host = host.to_string_lossy();
        let limit = host_maxsize() - 1;
        let m = format_args!("address '{host}' exceeds the limit of {limit} characters");
        return fail(E::SW_ERROR_NAME_TOO_LONG, m);
    }
    let mut port = PortConfig::new_port(serv.raw());
    port.r#type = socket_type;
    port.port.set(number);
    if port.host.set(host.to_bytes()) < 0 {
        // `throw std::bad_alloc()`, which nothing catches.
        swoole_rs_corebase::fatal("out of memory storing a port's host");
    }
    if socket_type & SOCK_SSL != 0 {
        port.r#type = socket_type & !SOCK_SSL;
        port.ssl = 1;
        (methods().ssl_context_init)(&mut port);
    }
    if (methods().create_socket)(&port) < 0 {
        let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        swoole_rs_corebase::log::set_last_error(errno);
        port.delete();
        return None;
    }
    Some(keep_port(serv, port))
}

/// `Server::check_port_type(port)` then `ports.push_back(port)`: the server keeps the port.
/// The port is the server's last of its type, which is how upstream's
/// `udp_socket_ipv4`, `udp_socket_ipv6` and `dgram_socket` name their listeners
/// ([`Server::dgram_socket`] finds them).
fn keep_port(serv: Server<'_>, port: Box<PortConfig>) -> Port<'_> {
    let config = serv.config();
    if matches!(port.r#type, SOCK_UDP | SOCK_UDP6 | SOCK_UNIX_DGRAM) {
        let fd = port
            .listening_socket()
            .map_or(-1, swoole_rs_reactor::Socket::fd);
        let size = port.socket_buffer_size.get().cast_unsigned();
        swoole_rs_reactor::swr_v1_socket_buffer_size(fd, true, true, size);
        config.have_dgram_sock.set(true);
        let ports = config.dgram_port_num.get();
        config.dgram_port_num.set(ports.wrapping_add(1));
    } else {
        config.have_stream_sock.set(true);
    }
    // SAFETY: ports are added before the server starts, while the master is its only
    // thread, so no other thread reads the list, and the caller's views of it ended
    // before the push.
    unsafe { serv.state().push_port(port) }
}

/// `Server::add_systemd_socket()`: imports the sockets systemd passed; the number
/// imported.
#[must_use]
pub fn add_systemd_socket(serv: Server<'_>) -> c_int {
    use swoole_rs_corebase::get_env;
    let count = || swoole_rs_corebase::get_systemd_listen_fds();
    let start = || get_env(c"LISTEN_FDS_START");
    import_sockets(serv, get_env(c"LISTEN_PID"), count, start)
}

/// [`add_systemd_socket`] over `LISTEN_PID`, the `LISTEN_FDS` count and
/// `LISTEN_FDS_START`; each is read only once the ones before it pass, as upstream's.
fn import_sockets(
    serv: Server<'_>,
    listen_pid: Option<c_int>,
    count: impl FnOnce() -> c_int,
    start: impl FnOnce() -> Option<c_int>,
) -> c_int {
    let origin = c"Server::add_systemd_socket";
    // Upstream compares an unset `LISTEN_PID` with an indeterminate value; treat it as foreign.
    if listen_pid.is_none() {
        swoole_rs_corebase::log::warning(origin, format_args!("invalid LISTEN_PID"));
        return 0;
    }
    let count = count();
    if count <= 0 {
        return 0;
    }
    let start = match start() {
        None => SYSTEMD_FDS_START,
        Some(start) if start < 0 => {
            swoole_rs_corebase::log::warning(origin, format_args!("invalid LISTEN_FDS_START"));
            return 0;
        }
        Some(start) => start,
    };
    let mut imported = 0;
    for fd in start..start.saturating_add(count) {
        let mut port = PortConfig::new_port(serv.raw());
        if !(methods().import)(&mut port, fd) {
            port.delete();
            continue;
        }
        // The record `import` made: O_NONBLOCK and O_CLOEXEC.
        if let Some(socket) = port.listening_socket() {
            socket.edit(|socket| socket_set_fd_option(socket, 1, 1));
        }
        keep_port(serv, port);
        imported += 1;
    }
    imported
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::fixture::LiveServer;
    use swoole_rs_process::worker_record::{WorkerRecord, WorkerStatus};

    /// The live server with `n` event workers in `dispatch` mode: their records, and
    /// the server.
    struct Workers {
        live: LiveServer,
        records: Vec<WorkerRecord>,
    }

    impl Workers {
        fn new(n: u32, dispatch: DispatchMode) -> Self {
            let live = LiveServer::new();
            let mut records: Vec<WorkerRecord> = (0..n).map(|_| *WorkerRecord::boxed()).collect();
            // SAFETY: the fixture's configuration, which nothing else reads during the write.
            let config = unsafe { &mut *live.state().config() };
            config.worker_num.set(n);
            config.dispatch_mode.set(dispatch as u8);
            config.workers = crate::records::tests::naming_at(records.as_mut_ptr());
            Self { live, records }
        }

        fn serv(&self) -> Server<'_> {
            self.live.server()
        }

        fn schedule(&self, fd: c_int) -> c_int {
            schedule_worker(self.serv(), fd, None)
        }

        fn set(&self, ids: &[u16], status: WorkerStatus) {
            for &id in ids {
                self.serv().worker_record(id).unwrap().set_status(status);
            }
        }

        fn warned(&self) -> bool {
            self.serv().config().scheduler_warning.swap(false, Relaxed)
        }
    }

    impl Drop for Workers {
        fn drop(&mut self) {
            // SAFETY: as in `new`; the records drop after the server forgets them.
            let config = unsafe { &mut *self.live.state().config() };
            config.workers = swoole_rs_memory::members::Shm::default();
        }
    }

    /// A connection on `fd` with `uid` and pinned `worker`.
    fn connect(w: &Workers, fd: c_int, uid: u32, worker: c_int) {
        let conn = w.live.state().tables_ref().connection_at(fd).unwrap();
        conn.set_fd(fd);
        conn.set_uid(uid);
        conn.set_worker_id(worker);
    }

    /// Past the connection table: no connection.
    const UNLISTED: c_int = 5000;

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn schedule_by_mode() {
        assert_eq!(
            Workers::new(4, DispatchMode::FdMod).schedule(10),
            2,
            "fd mod"
        );
        let round = Workers::new(4, DispatchMode::Round);
        assert_eq!(
            [round.schedule(10), round.schedule(10)],
            [0, 1],
            "round robin"
        );
        let uid = Workers::new(4, DispatchMode::UidMod);
        assert_eq!(uid.schedule(10), 2, "no uid: fd");
        assert_eq!(uid.schedule(UNLISTED), 0, "no connection: fd");
        connect(&uid, 10, 5, -1);
        assert_eq!(uid.schedule(10), 1, "uid");

        let mut co = Workers::new(4, DispatchMode::CoConnLb);
        assert_eq!(co.schedule(UNLISTED), 0, "no connection: fd");
        co.records[0].coroutine_num = 5;
        connect(&co, 10, 0, -1);
        assert_eq!(co.schedule(10), 1, "pinned to the lowest load");
        let conn = co.live.state().tables_ref().connection_at(10).unwrap();
        assert_eq!(conn.worker_id(), 1, "and stays there");
        co.records[1].coroutine_num = 9;
        assert_eq!(co.schedule(10), 1, "pinned");

        let mut req = Workers::new(4, DispatchMode::CoReqLb);
        for (record, load) in req.records.iter_mut().zip([5, 3, 3, 9]) {
            record.coroutine_num = load;
        }
        assert_eq!(req.schedule(10), 1, "the first lowest load");
        let concurrent = Workers::new(3, DispatchMode::ConcurrentLb);
        concurrent.records[0].concurrency.store(2, Relaxed);
        assert_eq!(concurrent.schedule(10), 1, "the lowest concurrency");

        let idle = Workers::new(3, DispatchMode::IdleWorker);
        idle.set(&[1], WorkerStatus::Idle);
        assert_eq!(idle.schedule(10), 1, "the first idle worker round robin");
        assert!(!idle.warned());
        idle.set(&[1], WorkerStatus::Busy);
        assert_eq!(idle.schedule(10), 2, "the last of worker_num + 1 tries");
        assert!(idle.warned(), "none idle: the scheduler warns");
        assert_eq!(idle.live.state().next_round(), 6, "four tries after two");
    }

    extern "C" fn to_six(
        _: crate::callbacks::Ctx,
        _: Server<'_>,
        _: Option<&crate::records::ConnectionRecord>,
        _: Option<&RecvData>,
    ) -> c_int {
        6
    }

    extern "C" fn fall_back(
        _: crate::callbacks::Ctx,
        _: Server<'_>,
        _: Option<&crate::records::ConnectionRecord>,
        _: Option<&RecvData>,
    ) -> c_int {
        USERFUNC_FALLBACK
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn a_dispatch_function_comes_first_unless_it_falls_back() {
        let w = Workers::new(4, DispatchMode::FdMod);
        let callbacks = &w.live.state().callbacks;
        callbacks.set(crate::callbacks::Callback::DispatchFunc(to_six));
        assert_eq!(w.schedule(10), 6, "user function");
        callbacks.set(crate::callbacks::Callback::DispatchFunc(fall_back));
        assert_eq!(w.schedule(10), 2, "user function falls back");
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn a_datagram_hashes_its_sender() {
        let w = Workers::new(4, DispatchMode::IpMod);
        // SAFETY: a plain record, for which zero is valid.
        let mut packet: crate::master_ops::DgramPacket = unsafe { core::mem::zeroed() };
        packet.socket_addr.kind = 2;
        packet.socket_addr.addr.inet_v4.sin_addr.s_addr = 0x0a00_0001_u32.to_be();
        let send = SendData {
            info: swoole_rs_corebase::DataHead::default(),
            data: (&raw const packet).cast(),
        };
        assert_eq!(ip_key(w.live.state(), -1, Some(&send)), 0x0a00_0001);
        assert_eq!(
            schedule_worker(w.serv(), -1, Some(&send)),
            1,
            "10.0.0.1 mod 4"
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn worker_side_sends() {
        use crate::hybrid::tests::{Call, calls};
        let live = LiveServer::new();
        let conn = live.state().tables_ref().connection_at(10).unwrap();
        conn.set_session_id(7);
        conn.set_reactor_id(1);
        calls();
        assert!(
            feedback(live.server(), conn, ServerEvent::Connect),
            "process mode goes through the reactor thread"
        );
        assert_eq!(calls(), [Call::ToReactorThread(7, 1, 5)]);
        assert!(!sendwait(live.server(), 3, b"data"), "missing session");
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn idle_and_tasking_counts() {
        let w = Workers::new(3, DispatchMode::FdMod);
        w.set(&[0, 2], WorkerStatus::Idle);
        assert_eq!(idle_workers(w.serv(), false), 2, "idle event workers");
        assert_eq!(idle_workers(w.serv(), true), 0, "no task workers");
        let tasking = &w.live.counters().tasking_num;
        tasking.store(3, SeqCst);
        assert_eq!(super::tasking(w.serv()), 3);
        tasking.store(u32::MAX, SeqCst);
        assert_eq!(super::tasking(w.serv()), 0, "reset");
        assert_eq!(tasking.load(SeqCst), 0, "stored");
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn sendfile_checks() {
        use crate::hybrid::tests::{Call, calls};
        let live = LiveServer::new();
        let serv = live.server();
        calls();
        assert!(!sendfile(serv, 0, b"Cargo.toml", 0, 1), "invalid session");
        let long = vec![b'a'; IPC_BUFFER_SIZE - SENDFILE_TASK_SIZE];
        assert!(!sendfile(serv, 1, &long, 0, 1), "name too long");
        assert!(!sendfile(serv, 1, b"no-such-file", 0, 1), "missing");
        assert!(!sendfile(serv, 1, b"src", 0, 1), "directory");
        assert!(
            !sendfile(serv, 1, b"Cargo.toml", 1 << 40, 1),
            "offset past the end"
        );
        assert_eq!(calls(), [], "none reached the factory");
        let given = b"Cargo.toml\0junk";
        assert!(sendfile(serv, 12, given, 3, 99), "regular file");
        // The head, `l_file` (the whole given length) and the NUL.
        let len = 16 + 15 + 1;
        assert_eq!(
            calls(),
            [Call::Finish(
                12,
                ServerEvent::SendFile.code(),
                len,
                3,
                99,
                *b"Cargo.to"
            )],
            "the task: its offset and length, then the name up to its NUL"
        );
    }

    /// Port methods whose `create_socket` fails for port 1, and whose `import` takes
    /// fd 3 as TCP, fd 4 as UDP and refuses the rest, keeping the fd as the port.
    fn fake_port_methods() {
        // SAFETY: the fakes touch only the settings they are given.
        unsafe {
            crate::port_config::set_methods(crate::port_config::PortMethods {
                ssl_context_init: |_| true,
                ssl_create: |_, _| unreachable!(),
                import: |config, fd| {
                    config.r#type = if fd == 4 { SOCK_UDP } else { 1 };
                    config.port.set(fd);
                    fd < 5
                },
                close: |_| unreachable!(),
                listen: |_| unreachable!(),
                create_socket: |config| if config.port.get() == 1 { -1 } else { 0 },
                close_socket: |_| unreachable!(),
                get_protocols: |_| unreachable!(),
                init_protocol: |_| unreachable!(),
                destroy_http_request: |_| unreachable!(),
                tls_free: |_| {},
                dtls_create: |_, _| unreachable!(),
                dtls_session: |_, _| unreachable!(),
                dtls_free: |_, _| unreachable!(),
            });
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn add_port_checks_then_keeps_the_port() {
        fake_port_methods();
        let live = LiveServer::new();
        let serv = live.server();
        let before = serv.ports().len();
        assert!(add_port(serv, 1, c"127.0.0.1", 70000).is_none(), "bad port");
        let long = std::ffi::CString::new(vec![b'a'; host_maxsize()]).unwrap();
        assert!(
            add_port(serv, SOCK_UNIX_STREAM, &long, 70000).is_none(),
            "unix sockets skip the port check, then the path is too long"
        );
        assert!(add_port(serv, 1, c"127.0.0.1", 1).is_none(), "no socket");
        assert_eq!(serv.ports().len(), before, "none kept");
        assert!(!serv.config().have_dgram_sock.get());

        let port = add_port(serv, SOCK_UDP | SOCK_SSL, c"127.0.0.1", 9501).unwrap();
        let config = port.config();
        assert_eq!(
            (config.r#type, config.ssl, config.port.get()),
            (SOCK_UDP, 1, 9501)
        );
        assert_eq!(serv.ports().len(), before + 1, "kept");
        assert_eq!(serv.ports()[before].raw(), port.raw());
        assert!(serv.config().have_dgram_sock.get());
        assert_eq!(serv.config().dgram_port_num.get(), 1);
        assert!(add_port(serv, 1, c"::", 9502).is_some(), "a stream port");
        assert!(serv.config().have_stream_sock.get());

        // SAFETY: the fixture's config, which nothing else borrows here.
        let config = unsafe { serv.config_mut() };
        config.factory_ = core::ptr::NonNull::dangling().as_ptr();
        assert!(add_port(serv, 1, c"::", 9503).is_none(), "created");
        config.factory_ = core::ptr::null_mut();
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn systemd_sockets_import_from_listen_fds_start() {
        fake_port_methods();
        let live = LiveServer::new();
        let serv = live.server();
        let before = serv.ports().len();
        assert_eq!(
            import_sockets(serv, None, || unreachable!(), || unreachable!()),
            0,
            "no LISTEN_PID"
        );
        assert_eq!(
            import_sockets(serv, Some(1), || -1, || unreachable!()),
            0,
            "no LISTEN_FDS"
        );
        assert_eq!(
            import_sockets(serv, Some(1), || 3, || Some(-1)),
            0,
            "bad start"
        );
        assert_eq!(serv.ports().len(), before);
        assert_eq!(
            import_sockets(serv, Some(1), || 3, || None),
            2,
            "from fd 3; 5 refused"
        );
        let kept: Vec<_> = serv.ports()[before..]
            .iter()
            .map(|port| (port.config().port.get(), port.config().r#type))
            .collect();
        assert_eq!(kept, [(3, 1), (4, SOCK_UDP)]);
        assert!(serv.config().have_stream_sock.get() && serv.config().have_dgram_sock.get());
    }
}
