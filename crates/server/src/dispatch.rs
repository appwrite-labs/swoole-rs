//! Choosing a worker, sends from workers, and listen ports.
use crate::ops::{
    FIELD_DISPATCH_MODE, FIELD_TASK_WORKER_NUM, FIELD_WORKER_NUM, LOG_FDS_START, LOG_FILE_EMPTY,
    LOG_HOST_TOO_LONG, LOG_INVALID_PORT, LOG_INVALID_SESSION, LOG_LISTEN_PID, LOG_NOT_REGULAR,
    LOG_PORT_CREATED, LOG_SEND_IN_MASTER, LOG_SENDFILE_NAME, LOG_SENDWAIT, LOG_STAT,
    LOG_TOO_MANY_PORTS, OP_CONN_UID, OP_CONN_WORKER, OP_DGRAM_PORT, OP_DISPATCH_FUNC,
    OP_ENV_FDS_START, OP_ENV_LISTEN_PID, OP_FD_CONN, OP_FEEDBACK_SEND, OP_FEEDBACK_THREAD,
    OP_FIELD, OP_HAS_DISPATCH_FUNC, OP_HOST_MAXSIZE, OP_HOST_SIZE, OP_IP_KEY, OP_IS_CREATED,
    OP_IS_MASTER, OP_MODE, OP_NEW_PORT_TYPE, OP_PIN_WORKER, OP_PIPE_NOBUFS, OP_PORT_COUNT,
    OP_PORT_IMPORT, OP_PORT_NEW, OP_PORT_NUMBER, OP_PORT_PUSH, OP_PORT_SOCKET, OP_PORT_TYPE,
    OP_SEND_SYNC, OP_SENDFILE_FINISH, OP_SENDFILE_NAME, OP_SENDFILE_NAME_LEN, OP_SENDFILE_NAME_MAX,
    OP_SENDFILE_OFFSET, OP_SOCKET_CONN, OP_SOCKET_NOBUFS, OP_STREAM_PORT, OP_SYSTEMD_FDS,
    OP_TASKING, OP_TASKING_RESET, OP_VERIFY_CONN, OP_WORKER_CONCURRENCY, OP_WORKER_COROUTINES,
    OP_WORKER_IDLE,
};
use crate::{Host, MODE_PROCESS, state};
use core::ffi::c_char;

/// `Server::DispatchMode`, `DISPATCH_RESULT_USERFUNC_FALLBACK`; native asserts each.
const DISPATCH_ROUND: u64 = 1;
const DISPATCH_FDMOD: u64 = 2;
const DISPATCH_IPMOD: u64 = 4;
const DISPATCH_UIDMOD: u64 = 5;
const DISPATCH_CO_CONN_LB: u64 = 8;
const DISPATCH_CO_REQ_LB: u64 = 9;
const DISPATCH_CONCURRENT_LB: u64 = 10;
const USERFUNC_FALLBACK: i64 = -3;

/// `SocketType`, `SW_SOCK_SSL`, `SW_MAX_LISTEN_PORT`, `SW_SYSTEMD_FDS_START`.
const SOCK_UDP: u64 = 2;
const SOCK_UDP6: u64 = 4;
const SOCK_UNIX_STREAM: u64 = 5;
const SOCK_UNIX_DGRAM: u64 = 6;
const SOCK_SSL: u64 = 1 << 9;
const MAX_LISTEN_PORT: u64 = 60000;
const SYSTEMD_FDS_START: i64 = 3;

fn field<H: Host>(h: &mut H, id: u32) -> crate::Effect<u64> {
    h.op(OP_FIELD, u64::from(id))
}

/// `Server::schedule_worker`: the user's dispatch function, else the dispatch mode.
pub(crate) fn schedule_worker<H: Host>(h: &mut H, fd: i32) -> crate::Effect<i64> {
    if h.is(OP_HAS_DISPATCH_FUNC)? {
        let id = h.signed(OP_DISPATCH_FUNC)?;
        if id != USERFUNC_FALLBACK {
            return Ok(id);
        }
    }
    let fd_key = fd.cast_unsigned();
    let workers = u32::try_from(field(h, FIELD_WORKER_NUM)?)
        .unwrap_or(u32::MAX)
        .max(1);
    let key = match field(h, FIELD_DISPATCH_MODE)? {
        DISPATCH_ROUND => h.state()?.next_round(),
        DISPATCH_FDMOD => fd_key,
        DISPATCH_IPMOD => u32::try_from(h.get(OP_IP_KEY)? & 0xffff_ffff).unwrap_or(0),
        DISPATCH_UIDMOD => {
            let uid = if h.is(OP_FD_CONN)? {
                h.get(OP_CONN_UID)?
            } else {
                0
            };
            if uid == 0 {
                fd_key
            } else {
                u32::try_from(uid).unwrap_or(fd_key)
            }
        }
        DISPATCH_CO_CONN_LB => {
            if !h.is(OP_FD_CONN)? {
                return Ok(i64::from(fd_key % workers));
            }
            let worker = h.signed(OP_CONN_WORKER)?;
            return Ok(if worker < 0 {
                let lowest = state::lowest(h, OP_WORKER_COROUTINES)?;
                h.op(OP_PIN_WORKER, lowest.cast_unsigned())?;
                lowest
            } else {
                worker
            });
        }
        DISPATCH_CO_REQ_LB => return state::lowest(h, OP_WORKER_COROUTINES),
        DISPATCH_CONCURRENT_LB => return state::lowest(h, OP_WORKER_CONCURRENCY),
        // Deliver to an idle worker.
        _ => return state::idle_worker(h),
    };
    Ok(i64::from(key % workers))
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

/// `Server::has_kernel_nobufs_error`.
pub(crate) fn kernel_nobufs<H: Host>(h: &mut H, session: u64) -> crate::Effect<bool> {
    let conn = h.op(OP_SOCKET_CONN, session)?;
    if conn == 0 {
        return Ok(false);
    }
    Ok(if conn & 2 != 0 {
        h.is(OP_PIPE_NOBUFS)?
    } else {
        h.is(OP_SOCKET_NOBUFS)?
    })
}

/// Idle workers among ids `from..to`.
pub(crate) fn idle_workers<H: Host>(h: &mut H, task: bool) -> crate::Effect<i64> {
    let workers = field(h, FIELD_WORKER_NUM)?;
    let range = if task {
        workers..workers + field(h, FIELD_TASK_WORKER_NUM)?
    } else {
        0..workers
    };
    let mut idle = 0;
    for id in range {
        if h.op(OP_WORKER_IDLE, id)? != 0 {
            idle += 1;
        }
    }
    Ok(idle)
}

/// `Server::get_tasking_num`: a negative count is reset to 0.
pub(crate) fn tasking<H: Host>(h: &mut H) -> crate::Effect<i64> {
    let count = h.signed(OP_TASKING)?;
    if count < 0 {
        h.get(OP_TASKING_RESET)?;
        return Ok(0);
    }
    Ok(count)
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
    // SAFETY: a zeroed `stat` is a valid out value.
    let mut st: libc::stat = unsafe { core::mem::zeroed() };
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
    use crate::ops::{
        RUN_ADD_PORT, RUN_FEEDBACK, RUN_IDLE_TASK_WORKERS, RUN_IDLE_WORKERS, RUN_KERNEL_NOBUFS,
        RUN_SCHEDULE_WORKER, RUN_SENDFILE, RUN_SENDWAIT, RUN_SYSTEMD_SOCKETS, RUN_TASKING,
    };
    Ok(match operation {
        RUN_SCHEDULE_WORKER => schedule_worker(h, i32::try_from(arg.cast_signed()).unwrap_or(0))?,
        RUN_FEEDBACK => i64::from(feedback(h, arg)?),
        RUN_SENDWAIT => i64::from(sendwait(h, arg)?),
        RUN_KERNEL_NOBUFS => i64::from(kernel_nobufs(h, arg)?),
        RUN_IDLE_WORKERS => idle_workers(h, false)?,
        RUN_IDLE_TASK_WORKERS => idle_workers(h, true)?,
        RUN_TASKING => tasking(h)?,
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

    fn mode(dispatch: u64, extra: &[(u32, u64)]) -> Script {
        let mut s = Script::with(extra).queue(OP_FIELD, &[4, dispatch]);
        s.facts.entry(OP_FIELD).or_insert(4);
        s
    }

    #[test]
    fn schedule_by_mode() {
        assert_eq!(
            schedule_worker(&mut mode(DISPATCH_FDMOD, &[]), 10).unwrap(),
            2,
            "fd mod"
        );
        assert_eq!(
            schedule_worker(&mut mode(DISPATCH_ROUND, &[]), 10).unwrap(),
            0,
            "round robin"
        );
        assert_eq!(
            schedule_worker(&mut mode(DISPATCH_IPMOD, &[(OP_IP_KEY, 9)]), 10).unwrap(),
            1,
            "ip hash"
        );
        assert_eq!(
            schedule_worker(&mut mode(DISPATCH_UIDMOD, &[]), 10).unwrap(),
            2,
            "no connection: fd"
        );
        assert_eq!(
            schedule_worker(
                &mut mode(DISPATCH_UIDMOD, &[(OP_FD_CONN, 1), (OP_CONN_UID, 5)]),
                10
            )
            .unwrap(),
            1,
            "uid"
        );
        assert_eq!(
            schedule_worker(&mut mode(DISPATCH_CO_CONN_LB, &[]), 10).unwrap(),
            2,
            "no connection"
        );
        assert_eq!(
            schedule_worker(
                &mut mode(
                    DISPATCH_CO_CONN_LB,
                    &[
                        (OP_FD_CONN, 1),
                        (OP_CONN_WORKER, u64::MAX),
                        (OP_WORKER_COROUTINES, 1)
                    ]
                ),
                10
            )
            .unwrap(),
            0,
            "pinned to the lowest load"
        );
        assert_eq!(
            schedule_worker(&mut mode(DISPATCH_CO_REQ_LB, &[]), 10).unwrap(),
            0,
            "lowest load"
        );
        assert_eq!(
            schedule_worker(&mut mode(3, &[(OP_WORKER_IDLE, 1)]), 10).unwrap(),
            0,
            "idle worker"
        );
        let mut s = Script::with(&[(OP_HAS_DISPATCH_FUNC, 1), (OP_DISPATCH_FUNC, 6)]);
        assert_eq!(schedule_worker(&mut s, 10).unwrap(), 6, "user function");
        let mut s = mode(
            DISPATCH_FDMOD,
            &[
                (OP_HAS_DISPATCH_FUNC, 1),
                (OP_DISPATCH_FUNC, (-3i64).cast_unsigned()),
            ],
        );
        assert_eq!(
            schedule_worker(&mut s, 10).unwrap(),
            2,
            "user function falls back"
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

        let mut s = Script::with(&[(OP_SOCKET_CONN, 3), (OP_PIPE_NOBUFS, 1)]);
        assert!(kernel_nobufs(&mut s, 1).unwrap(), "pipe");
        let mut s = Script::with(&[(OP_SOCKET_CONN, 1), (OP_PIPE_NOBUFS, 1)]);
        assert!(!kernel_nobufs(&mut s, 1).unwrap(), "socket");

        let mut s = Script::with(&[(OP_FIELD, 3)]).queue(OP_WORKER_IDLE, &[1, 0, 1]);
        assert_eq!(
            idle_workers(&mut s, false).unwrap(),
            2,
            "idle event workers"
        );
        let mut s = Script::with(&[(OP_TASKING, u64::MAX)]);
        assert_eq!(tasking(&mut s).unwrap(), 0, "reset");
        assert_eq!(s.called(OP_TASKING_RESET), 1, "stored");
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
