//! Choosing a worker, sends from workers, and listen ports.
use crate::base_factory::SendData;
use crate::callbacks::{CB_DISPATCH_FUNC, CallbackSlot};
use crate::ops::{
    LOG_FDS_START, LOG_FILE_EMPTY, LOG_HOST_TOO_LONG, LOG_INVALID_PORT, LOG_INVALID_SESSION,
    LOG_LISTEN_PID, LOG_NOT_REGULAR, LOG_PORT_CREATED, LOG_SEND_IN_MASTER, LOG_SENDFILE_NAME,
    LOG_SENDWAIT, LOG_STAT, LOG_TOO_MANY_PORTS, OP_DGRAM_PORT, OP_ENV_FDS_START, OP_ENV_LISTEN_PID,
    OP_FEEDBACK_SEND, OP_FEEDBACK_THREAD, OP_HOST_MAXSIZE, OP_HOST_SIZE, OP_IS_CREATED,
    OP_IS_MASTER, OP_MODE, OP_NEW_PORT_TYPE, OP_PORT_COUNT, OP_PORT_IMPORT, OP_PORT_NEW,
    OP_PORT_NUMBER, OP_PORT_PUSH, OP_PORT_SOCKET, OP_PORT_TYPE, OP_SEND_SYNC, OP_SENDFILE_FINISH,
    OP_SENDFILE_NAME, OP_SENDFILE_NAME_LEN, OP_SENDFILE_NAME_MAX, OP_SENDFILE_OFFSET,
    OP_STREAM_PORT, OP_SYSTEMD_FDS, OP_VERIFY_CONN,
};
use crate::reactor_entry::RecvData;
use crate::records::Lookup;
use crate::view::Server;
use crate::{Host, MODE_PROCESS};
use core::ffi::{c_char, c_int, c_long};
use core::sync::atomic::Ordering::{Relaxed, SeqCst};
use swoole_rs_corebase::CCast;
use swoole_rs_utilities::misc::swoole_hash_php;

/// `Server::DispatchMode`, `DISPATCH_RESULT_USERFUNC_FALLBACK`; native asserts each.
const DISPATCH_ROUND: u8 = 1;
const DISPATCH_FDMOD: u8 = 2;
const DISPATCH_IPMOD: u8 = 4;
const DISPATCH_UIDMOD: u8 = 5;
const DISPATCH_CO_CONN_LB: u8 = 8;
const DISPATCH_CO_REQ_LB: u8 = 9;
const DISPATCH_CONCURRENT_LB: u8 = 10;
const USERFUNC_FALLBACK: c_int = -3;

/// `SocketType`, `SW_SOCK_SSL`, `SW_MAX_LISTEN_PORT`, `SW_SYSTEMD_FDS_START`.
const SOCK_UDP: u64 = 2;
const SOCK_UDP6: u64 = 4;
const SOCK_UNIX_STREAM: u64 = 5;
const SOCK_UNIX_DGRAM: u64 = 6;
const SOCK_SSL: u64 = 1 << 9;
const MAX_LISTEN_PORT: u64 = 60000;
const SYSTEMD_FDS_START: i64 = 3;

/// `Server::schedule_worker(fd, data)`: the user's dispatch function, else the dispatch
/// mode. `send` is the data the worker takes, if any.
pub fn schedule_worker(serv: Server<'_>, fd: c_int, send: Option<&SendData>) -> c_int {
    let state = serv.state();
    let key = c_long::from(fd);
    let dispatch_func = state.callbacks.slot(CB_DISPATCH_FUNC);
    if dispatch_func.is_some_and(CallbackSlot::is_set) {
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
    let key = match config.dispatch_mode.get() {
        DISPATCH_ROUND => state.next_round(),
        DISPATCH_FDMOD => fd_key,
        DISPATCH_IPMOD => ip_key(state, key, send),
        DISPATCH_UIDMOD => match conn().map_or(0, crate::Connection::uid) {
            0 => fd_key,
            uid => uid,
        },
        DISPATCH_CO_CONN_LB => {
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
        DISPATCH_CO_REQ_LB => return lowest(serv, Load::Coroutines),
        DISPATCH_CONCURRENT_LB => return lowest(serv, Load::Concurrency),
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

/// `Server::feedback`: a connection event from a worker to the master.
pub(crate) fn feedback<H: Host>(h: &mut H, event: u64) -> crate::Effect<bool> {
    Ok(if h.get(OP_MODE)? == MODE_PROCESS {
        h.op(OP_FEEDBACK_THREAD, event)? != 0
    } else {
        h.op(OP_FEEDBACK_SEND, event)? != 0
    })
}

/// `Server::sendwait`.
pub(crate) fn sendwait<H: Host>(h: &mut H, session: u64) -> crate::Effect<bool> {
    if h.op(OP_VERIFY_CONN, session)? == 0 {
        h.op(LOG_SENDWAIT, session)?;
        return Ok(false);
    }
    h.is(OP_SEND_SYNC)
}

/// `Server::sendfile`: validate the session and file, then hand the task to the factory.
pub(crate) fn sendfile<H: Host>(h: &mut H, session: i64) -> crate::Effect<bool> {
    if session <= 0 {
        h.get(LOG_INVALID_SESSION)?;
        return Ok(false);
    }
    if h.is(OP_IS_MASTER)? {
        h.get(LOG_SEND_IN_MASTER)?;
        return Ok(false);
    }
    if h.get(OP_SENDFILE_NAME_LEN)? > h.get(OP_SENDFILE_NAME_MAX)? {
        h.get(LOG_SENDFILE_NAME)?;
        return Ok(false);
    }
    let address = usize::try_from(h.get(OP_SENDFILE_NAME)?).unwrap_or(0);
    let name = core::ptr::with_exposed_provenance::<c_char>(address);
    let mut st: libc::stat = swoole_rs_corebase::sys::zeroed();
    // SAFETY: `name` is the NUL-terminated copy native code just made; `st` is valid.
    if unsafe { libc::stat(name, &raw mut st) } < 0 {
        h.get(LOG_STAT)?;
        return Ok(false);
    }
    if st.st_mode & libc::S_IFMT != libc::S_IFREG {
        h.get(LOG_NOT_REGULAR)?;
        return Ok(false);
    }
    if st.st_size <= h.signed(OP_SENDFILE_OFFSET)? {
        h.get(LOG_FILE_EMPTY)?;
        return Ok(false);
    }
    h.is(OP_SENDFILE_FINISH)
}

/// `Server::add_port`: validate, create the listen socket, and register the port.
pub(crate) fn add_port<H: Host>(h: &mut H) -> crate::Effect<i64> {
    if h.is(OP_IS_CREATED)? {
        h.get(LOG_PORT_CREATED)?;
        return Ok(0);
    }
    if h.get(OP_PORT_COUNT)? >= MAX_LISTEN_PORT {
        h.get(LOG_TOO_MANY_PORTS)?;
        return Ok(0);
    }
    let kind = h.get(OP_PORT_TYPE)?;
    let local = matches!(kind, SOCK_UNIX_STREAM | SOCK_UNIX_DGRAM);
    if !local && !(0..=65535).contains(&h.signed(OP_PORT_NUMBER)?) {
        h.get(LOG_INVALID_PORT)?;
        return Ok(0);
    }
    if h.get(OP_HOST_SIZE)? > h.get(OP_HOST_MAXSIZE)? {
        h.get(LOG_HOST_TOO_LONG)?;
        return Ok(0);
    }
    h.op(OP_PORT_NEW, u64::from(kind & SOCK_SSL != 0))?;
    if !h.is(OP_PORT_SOCKET)? {
        return Ok(0);
    }
    check_port_type(h)?;
    h.get(OP_PORT_PUSH)?;
    Ok(1)
}

/// `Server::check_port_type` of the new port.
fn check_port_type<H: Host>(h: &mut H) -> crate::Effect<()> {
    let kind = h.get(OP_NEW_PORT_TYPE)?;
    if matches!(kind, SOCK_UDP | SOCK_UDP6 | SOCK_UNIX_DGRAM) {
        h.op(OP_DGRAM_PORT, kind)?;
    } else {
        h.get(OP_STREAM_PORT)?;
    }
    Ok(())
}

/// `Server::add_systemd_socket`: import the sockets systemd passed; the number imported.
pub(crate) fn systemd_sockets<H: Host>(h: &mut H) -> crate::Effect<i64> {
    // Upstream compares an unset `LISTEN_PID` with an indeterminate value; treat it as foreign.
    if h.signed(OP_ENV_LISTEN_PID)? == i64::MIN {
        h.get(LOG_LISTEN_PID)?;
        return Ok(0);
    }
    let count = h.signed(OP_SYSTEMD_FDS)?;
    if count <= 0 {
        return Ok(0);
    }
    let start = match h.signed(OP_ENV_FDS_START)? {
        i64::MIN => SYSTEMD_FDS_START,
        start if start < 0 => {
            h.get(LOG_FDS_START)?;
            return Ok(0);
        }
        start => start,
    };
    let mut imported = 0;
    for fd in start..start + count {
        if h.op(OP_PORT_IMPORT, fd.cast_unsigned())? == 0 {
            continue;
        }
        check_port_type(h)?;
        h.get(OP_PORT_PUSH)?;
        imported += 1;
    }
    Ok(imported)
}

/// The dispatch and port operations of `crate::run`.
pub(crate) fn run<H: Host>(h: &mut H, operation: u32, arg: u64) -> crate::Effect<i64> {
    use crate::ops::{RUN_ADD_PORT, RUN_FEEDBACK, RUN_SENDFILE, RUN_SENDWAIT, RUN_SYSTEMD_SOCKETS};
    Ok(match operation {
        RUN_FEEDBACK => i64::from(feedback(h, arg)?),
        RUN_SENDWAIT => i64::from(sendwait(h, arg)?),
        RUN_SENDFILE => i64::from(sendfile(h, arg.cast_signed())?),
        RUN_ADD_PORT => add_port(h)?,
        RUN_SYSTEMD_SOCKETS => systemd_sockets(h)?,
        _ => crate::base_factory::run(h, operation, arg)?.unwrap_or(crate::SW_ERR),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MODE_BASE;
    use crate::script::Script;

    use crate::fixture::LiveServer;
    use swoole_rs_process::worker_record::{WorkerRecord, WorkerStatus};

    /// The live server with `n` event workers in `dispatch` mode: their records, and
    /// the server.
    struct Workers {
        live: LiveServer,
        records: Vec<WorkerRecord>,
    }

    impl Workers {
        fn new(n: u32, dispatch: u8) -> Self {
            let live = LiveServer::new();
            let mut records: Vec<WorkerRecord> = (0..n).map(|_| *WorkerRecord::boxed()).collect();
            // SAFETY: the fixture's configuration, which nothing else reads during the write.
            let config = unsafe { &mut *live.state().config() };
            config.worker_num.set(n);
            config.dispatch_mode.set(dispatch);
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
        assert_eq!(Workers::new(4, DISPATCH_FDMOD).schedule(10), 2, "fd mod");
        let round = Workers::new(4, DISPATCH_ROUND);
        assert_eq!(
            [round.schedule(10), round.schedule(10)],
            [0, 1],
            "round robin"
        );
        let uid = Workers::new(4, DISPATCH_UIDMOD);
        assert_eq!(uid.schedule(10), 2, "no uid: fd");
        assert_eq!(uid.schedule(UNLISTED), 0, "no connection: fd");
        connect(&uid, 10, 5, -1);
        assert_eq!(uid.schedule(10), 1, "uid");

        let mut co = Workers::new(4, DISPATCH_CO_CONN_LB);
        assert_eq!(co.schedule(UNLISTED), 0, "no connection: fd");
        co.records[0].coroutine_num = 5;
        connect(&co, 10, 0, -1);
        assert_eq!(co.schedule(10), 1, "pinned to the lowest load");
        let conn = co.live.state().tables_ref().connection_at(10).unwrap();
        assert_eq!(conn.worker_id(), 1, "and stays there");
        co.records[1].coroutine_num = 9;
        assert_eq!(co.schedule(10), 1, "pinned");

        let mut req = Workers::new(4, DISPATCH_CO_REQ_LB);
        for (record, load) in req.records.iter_mut().zip([5, 3, 3, 9]) {
            record.coroutine_num = load;
        }
        assert_eq!(req.schedule(10), 1, "the first lowest load");
        let concurrent = Workers::new(3, DISPATCH_CONCURRENT_LB);
        concurrent.records[0].concurrency.store(2, Relaxed);
        assert_eq!(concurrent.schedule(10), 1, "the lowest concurrency");

        let idle = Workers::new(3, 3);
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
        let w = Workers::new(4, DISPATCH_FDMOD);
        let callbacks = &w.live.state().callbacks;
        callbacks.set(crate::callbacks::Callback::DispatchFunc(to_six));
        assert_eq!(w.schedule(10), 6, "user function");
        callbacks.set(crate::callbacks::Callback::DispatchFunc(fall_back));
        assert_eq!(w.schedule(10), 2, "user function falls back");
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn a_datagram_hashes_its_sender() {
        let w = Workers::new(4, DISPATCH_IPMOD);
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
    fn worker_side_sends() {
        let mut s = Script::with(&[(OP_MODE, MODE_PROCESS), (OP_FEEDBACK_THREAD, 1)]);
        assert!(
            feedback(&mut s, 5).unwrap(),
            "process mode goes through the reactor thread"
        );
        let mut s = Script::with(&[(OP_MODE, MODE_BASE)]);
        assert!(!feedback(&mut s, 5).unwrap(), "base mode sends directly");

        let mut s = Script::with(&[]);
        assert!(!sendwait(&mut s, 3).unwrap(), "missing session");
        assert_eq!(s.args(LOG_SENDWAIT), vec![3], "traced");
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn idle_and_tasking_counts() {
        let w = Workers::new(3, DISPATCH_FDMOD);
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
    fn sendfile_checks() {
        let file = |name: &core::ffi::CStr, offset: u64| {
            let address = u64::try_from(name.as_ptr().expose_provenance()).unwrap();
            Script::with(&[
                (OP_SENDFILE_NAME_MAX, 100),
                (OP_SENDFILE_NAME, address),
                (OP_SENDFILE_OFFSET, offset),
                (OP_SENDFILE_FINISH, 1),
            ])
        };
        let mut s = Script::with(&[]);
        assert!(!sendfile(&mut s, 0).unwrap(), "invalid session");
        let mut s = Script::with(&[(OP_IS_MASTER, 1)]);
        assert!(!sendfile(&mut s, 1).unwrap(), "master");
        let mut s = Script::with(&[(OP_SENDFILE_NAME_MAX, 4), (OP_SENDFILE_NAME_LEN, 5)]);
        assert!(!sendfile(&mut s, 1).unwrap(), "name too long");

        assert!(
            sendfile(&mut file(c"Cargo.toml", 0), 1).unwrap(),
            "regular file"
        );
        let mut s = file(c"Cargo.toml", 1 << 40);
        assert!(!sendfile(&mut s, 1).unwrap(), "offset past the end");
        assert_eq!(s.called(LOG_FILE_EMPTY), 1, "warned");
        let mut s = file(c"src", 0);
        assert!(!sendfile(&mut s, 1).unwrap(), "directory");
        assert_eq!(s.called(LOG_NOT_REGULAR), 1, "warned");
        let mut s = file(c"no-such-file", 0);
        assert!(!sendfile(&mut s, 1).unwrap(), "missing");
        assert_eq!(s.called(LOG_STAT), 1, "warned");
    }

    #[test]
    fn ports() {
        let mut s = Script::with(&[
            (OP_PORT_TYPE, 1 | SOCK_SSL),
            (OP_PORT_NUMBER, 9501),
            (OP_HOST_SIZE, 8),
            (OP_HOST_MAXSIZE, 108),
            (OP_PORT_SOCKET, 1),
            (OP_NEW_PORT_TYPE, SOCK_UDP),
        ]);
        assert_eq!(add_port(&mut s).unwrap(), 1, "added");
        assert_eq!(s.args(OP_PORT_NEW), vec![1], "TLS");
        assert_eq!(
            s.args(OP_DGRAM_PORT),
            vec![SOCK_UDP],
            "datagram socket recorded"
        );

        let mut s = Script::with(&[(OP_PORT_TYPE, 1), (OP_PORT_NUMBER, 70000)]);
        assert_eq!(add_port(&mut s).unwrap(), 0, "bad port");
        assert_eq!(s.called(LOG_INVALID_PORT), 1, "warned");
        let mut s = Script::with(&[
            (OP_PORT_TYPE, SOCK_UNIX_STREAM),
            (OP_PORT_NUMBER, 70000),
            (OP_HOST_SIZE, 200),
            (OP_HOST_MAXSIZE, 108),
        ]);
        assert_eq!(add_port(&mut s).unwrap(), 0, "path too long");
        assert_eq!(
            s.called(LOG_HOST_TOO_LONG),
            1,
            "unix sockets skip the port check"
        );

        let mut s = Script::with(&[
            (OP_ENV_LISTEN_PID, 1),
            (OP_SYSTEMD_FDS, 3),
            (OP_ENV_FDS_START, i64::MIN.cast_unsigned()),
        ])
        .queue(OP_PORT_IMPORT, &[1, 0, 1]);
        assert_eq!(systemd_sockets(&mut s).unwrap(), 2, "two imported");
        assert_eq!(s.args(OP_PORT_IMPORT), vec![3, 4, 5], "from fd 3");
        let mut s = Script::with(&[(OP_ENV_LISTEN_PID, i64::MIN.cast_unsigned())]);
        assert_eq!(systemd_sockets(&mut s).unwrap(), 0, "no LISTEN_PID");
    }
}
