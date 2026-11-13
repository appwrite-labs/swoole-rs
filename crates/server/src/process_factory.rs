//! The process-mode factory (upstream's `src/server/process.cc`): a reactor thread's
//! dispatch to a worker, and a worker's sends and closes back through the reactor
//! threads. The pipe writes are the hybrid's [`Pipe`] sends; forking, setup and
//! teardown are in `native`.
#![cfg_attr(
    not(any(test, feature = "upstream-symbols")),
    expect(dead_code, reason = "only the hybrid runs process mode")
)]
use core::ffi::{c_int, c_long};
use core::sync::atomic::Ordering::Relaxed;
use swoole_rs_corebase::{DataHead, log};
use swoole_rs_process::methods::{swoole_get_worker_id, worker_type};
use swoole_rs_process::worker_record::WorkerType;

use crate::base_factory::{SendData, is_stream_event, low16};
use crate::callbacks::{CB_CLOSE, CallbackSlot, EventSlot};
use crate::dispatch::schedule_worker;
use crate::hybrid::hybrid;
use crate::records::Lookup;
use crate::view;

#[cfg(all(feature = "upstream-symbols", not(test)))]
pub(crate) mod native;

/// Where process mode writes a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pipe {
    /// Worker `id`'s pipe, through this reactor thread's bus.
    Worker(u16),
    /// The reactor thread pipe worker `id` serves, through this worker's bus.
    Reactor(u16),
    /// Worker `id`'s master pipe, for a close it must run.
    Close(u16),
}

/// `Server::DISPATCH_RESULT_*`.
const DISPATCH_DISCARD_PACKET: c_int = -1;
const DISPATCH_CLOSE_CONNECTION: c_int = -2;
/// `Server::DISPATCH_FDMOD`, `DISPATCH_IPMOD`, `DISPATCH_CO_CONN_LB`.
const DISPATCH_FDMOD: u8 = 2;
const DISPATCH_IPMOD: u8 = 4;
const DISPATCH_CO_CONN_LB: u8 = 8;
/// `Server::CLOSE_RESET`, `Server::CLOSE_ACTIVELY`.
const CLOSE_RESET: c_int = 1 << 1;
const CLOSE_ACTIVELY: c_int = 1 << 2;
/// `SW_SERVER_EVENT_*`.
const EVENT_RECV_DATA: u8 = 0;
const EVENT_SEND_DATA: u8 = 2;
const EVENT_SEND_FILE: u8 = 3;
const EVENT_CLOSE: u8 = 4;
/// `SW_LOG_*`.
const TRACE: c_int = 1;
const NOTICE: c_int = 3;
const WARNING: c_int = 4;
/// `SW_ERROR_*`.
const SESSION_CLOSING: c_int = 1003;
const SESSION_CLOSED: c_int = 1004;
const SESSION_NOT_EXIST: c_int = 1005;
const OUTPUT_BUFFER_OVERFLOW: c_int = 1009;
const OUTPUT_SEND_YIELD: c_int = 1010;
const DATA_LENGTH_TOO_LARGE: c_int = 1203;

/// A worker id, as `Server::get_worker(uint16_t)` narrows it.
fn worker_id(id: i64) -> u16 {
    u16::try_from(id & 0xffff).unwrap_or_default()
}

/// Whether the dispatch mode binds a connection to one worker.
fn hash_dispatch(config: &crate::ServerConfig) -> bool {
    matches!(
        config.dispatch_mode.get(),
        DISPATCH_FDMOD | DISPATCH_IPMOD | DISPATCH_CO_CONN_LB
    )
}

/// This process's worker id, as `swoole_get_worker_id()`'s `int`.
fn current_worker() -> i64 {
    i64::from(swoole_get_worker_id().cast_signed())
}

/// `ProcessFactory::dispatch`: a reactor thread hands `send` to its worker. A stream
/// event's fd becomes its session.
pub(crate) fn dispatch(serv: view::Server<'_>, send: &mut SendData) -> bool {
    // The task's fd is the connection's, not its session's: native's `int` of it.
    let fd = u32::try_from(send.info.fd & 0xffff_ffff)
        .unwrap_or_default()
        .cast_signed();
    let target = schedule_worker(serv, fd, Some(send));
    if target < 0 {
        if target != DISPATCH_DISCARD_PACKET && target != DISPATCH_CLOSE_CONNECTION {
            let m = format_args!("invalid target worker id[{target}]");
            log::warning(c"ProcessFactory::dispatch", m);
        }
        return false;
    }
    let kind = send.info.kind;
    if is_stream_event(kind) {
        let conn = serv.state().connection(fd.into(), Lookup::ByFd);
        let Some(conn) = conn.filter(|conn| conn.is_active()) else {
            let m = format_args!("dispatch[type={kind}] failed, connection#{fd} is not active");
            log::warning(c"ProcessFactory::dispatch", m);
            return false;
        };
        // The server closed it: drop the data, unless this is its forced close.
        if conn.closed().load(Relaxed) != 0 && !(kind == EVENT_CLOSE && conn.close_force()) {
            return false;
        }
        send.info.fd = conn.session_id();
        send.info.server_fd = low16(conn.server_fd());
    }
    let target = worker_id(target.into());
    if kind == EVENT_RECV_DATA
        && let Some(worker) = serv.worker_record(target)
    {
        worker.count_dispatch();
    }
    hybrid().process_send(serv, Pipe::Worker(target), send)
}

/// `ProcessFactory::finish`: a worker sends `send` to its session through the reactor
/// thread that owns the connection.
pub(crate) fn finish(serv: view::Server<'_>, send: &SendData) -> bool {
    let (state, config) = (serv.state(), serv.config());
    let (session, len, kind) = (send.info.fd, send.info.len, send.info.kind);
    let size = config.output_buffer_size.get();
    if len > size {
        log::error_log(
            WARNING,
            DATA_LENGTH_TOO_LARGE,
            c"ProcessFactory::finish",
            format_args!(
                "The length of data [{len}] exceeds the output buffer size[{size}], please use \
                 the sendfile, chunked transfer mode or adjust the output_buffer_size"
            ),
        );
        return false;
    }
    let close = kind == EVENT_CLOSE;
    let lookup = if close {
        Lookup::Verified
    } else {
        Lookup::VerifiedTls
    };
    let Some(conn) = state.connection(session, lookup) else {
        if !close {
            let m = format_args!("session#{session} does not exists");
            log::error_log(TRACE, SESSION_NOT_EXIST, c"ProcessFactory::finish", m);
        }
        return false;
    };
    if (conn.closed().load(Relaxed) != 0 || conn.peer_closed()) && !close {
        let len = len.cast_signed();
        let m = format_args!("send {len} bytes failed, because session#{session} is closed");
        log::error_log(TRACE, SESSION_CLOSED, c"ProcessFactory::finish", m);
        return false;
    }
    if conn.overflow() && (kind == EVENT_SEND_DATA || kind == EVENT_SEND_FILE) {
        // `process_is_supported_send_yield`: the worker that owns the connection waits.
        let owner = || i64::from(schedule_worker(serv, conn.fd(), None)) == current_worker();
        if config.send_yield.get() && hash_dispatch(config) && owner() {
            log::set_last_error(OUTPUT_SEND_YIELD);
        } else {
            let m = format_args!("send failed, session={session} output buffer overflow");
            log::error_log(
                WARNING,
                OUTPUT_BUFFER_OVERFLOW,
                c"ProcessFactory::finish",
                m,
            );
        }
        return false;
    }
    let mut out = SendData {
        info: send.info,
        data: send.data,
    };
    out.info.reactor_id = i16::from(conn.reactor_id());
    out.info.server_fd = low16(swoole_get_worker_id().cast_signed());
    // `serv->get_reactor_pipe_socket(fd, reactor_id)`.
    let pipes = i64::from(state.tables_ref().reactor_pipe_num);
    let pipe = i64::from(out.info.reactor_id)
        + session.checked_rem(pipes).unwrap_or(0) * i64::from(config.reactor_num.get());
    hybrid().process_send(serv, Pipe::Reactor(worker_id(pipe)), &mut out)
}

/// `ProcessFactory::end`: closes `session`, forwarding an active close to the worker
/// the connection is bound to.
pub(crate) fn end(serv: view::Server<'_>, session: c_long, flags: c_int) -> bool {
    let (state, config) = (serv.state(), serv.config());
    let Some(conn) = state.connection(session, Lookup::Verified) else {
        let m = format_args!("session#{session} does not exists");
        log::error_log(TRACE, SESSION_NOT_EXIST, c"ProcessFactory::end", m);
        return false;
    };
    // Reset the send buffer: close immediately.
    if flags & CLOSE_RESET != 0 {
        conn.set_close_reset(true);
    }
    // The server closes the connection itself.
    if flags & CLOSE_ACTIVELY != 0 {
        conn.set_close_actively(true);
    }
    // Only an active close may be in another worker's binding: forward it there.
    if conn.close_actively() {
        let hash = hash_dispatch(config);
        let worker = if hash {
            i64::from(schedule_worker(serv, conn.fd(), None))
        } else {
            i64::from(conn.fd())
                .checked_rem(config.worker_num.get().into())
                .unwrap_or(0)
        };
        let is_worker = worker_type() == Some(WorkerType::Event);
        if !(is_worker && (!hash || worker == current_worker())) {
            let mut close = SendData {
                info: DataHead {
                    kind: EVENT_CLOSE,
                    fd: session,
                    reactor_id: i16::from(conn.reactor_id()),
                    ..DataHead::default()
                },
                data: core::ptr::null(),
            };
            return hybrid().process_send(serv, Pipe::Close(worker_id(worker)), &mut close);
        }
    }
    if conn.closing() {
        let m = format_args!("session#{session} is closing");
        log::error_log(NOTICE, SESSION_CLOSING, c"ProcessFactory::end", m);
        return false;
    }
    if !(conn.close_force() || conn.close_reset()) && conn.closed().load(Relaxed) != 0 {
        let m = format_args!("session#{session} is closed");
        log::error_log(TRACE, SESSION_CLOSED, c"ProcessFactory::end", m);
        return false;
    }
    // `Server::if_do_close_callback`: once, by whoever sets `closed`.
    let callback = state
        .callbacks
        .slot(CB_CLOSE)
        .is_some_and(CallbackSlot::is_set);
    if callback
        && conn
            .closed()
            .compare_exchange(0, 1, Relaxed, Relaxed)
            .is_ok()
    {
        let call = |info: &mut DataHead| state.callbacks.event(EventSlot::Close, serv, info);
        _ = crate::base_factory::on_close(|| Some(conn), session, call);
    }
    conn.set_close_errno(0);
    let close = SendData {
        info: DataHead {
            fd: session,
            kind: EVENT_CLOSE,
            ..DataHead::default()
        },
        data: core::ptr::null(),
    };
    finish(serv, &close)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conn_io::tests::{accepted, session};
    use crate::hybrid::tests::{Call, calls};

    fn send(fd: c_long, kind: u8, len: u32) -> SendData {
        SendData {
            info: DataHead {
                fd,
                kind,
                len,
                ..DataHead::default()
            },
            data: core::ptr::null(),
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn dispatch_drops_closed_connections_but_not_their_forced_close() {
        let (live, _clients) = accepted();
        let conn = session(&live, 1);
        let fd = c_long::from(conn.fd());
        let mut data = send(fd, EVENT_RECV_DATA, 4);
        assert!(dispatch(live.server(), &mut data));
        // The fd became the session, and the worker its target.
        assert_eq!(data.info.fd, 1);
        let [Call::Process(Pipe::Worker(_), 1, EVENT_RECV_DATA)] = calls()[..] else {
            panic!("one send to a worker");
        };
        conn.closed().store(1, Relaxed);
        for (kind, force, sent) in [
            (EVENT_RECV_DATA, true, false),
            (EVENT_CLOSE, true, true),
            (EVENT_CLOSE, false, false),
        ] {
            conn.set_close_force(force);
            assert_eq!(dispatch(live.server(), &mut send(fd, kind, 0)), sent);
        }
        drop(calls());
        // A connection that isn't there.
        assert!(!dispatch(
            live.server(),
            &mut send(9999, EVENT_RECV_DATA, 0)
        ));
        assert_eq!(calls(), []);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn finish_checks_size_session_and_overflow() {
        let (live, _clients) = accepted();
        let serv = live.server();
        serv.config().output_buffer_size.set(8);
        assert!(!finish(serv, &send(1, EVENT_SEND_DATA, 9)));
        assert!(!finish(serv, &send(99, EVENT_SEND_DATA, 1)));
        assert_eq!(calls(), []);
        assert!(finish(serv, &send(1, EVENT_SEND_DATA, 1)));
        assert_eq!(
            calls(),
            [Call::Process(Pipe::Reactor(0), 1, EVENT_SEND_DATA)]
        );
        let conn = session(&live, 1);
        conn.set_overflow(true);
        assert!(!finish(serv, &send(1, EVENT_SEND_DATA, 1)));
        // A close still goes through an overflowing buffer.
        assert!(finish(serv, &send(1, EVENT_CLOSE, 0)));
        conn.set_peer_closed(true);
        assert!(!finish(serv, &send(1, EVENT_SEND_DATA, 1)));
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no epoll for the live reactor")]
    fn end_forwards_active_closes_owned_elsewhere() {
        let (live, _clients) = accepted();
        let serv = live.server();
        // The master is no event worker: an active close goes to the bound worker.
        assert!(end(serv, 1, CLOSE_ACTIVELY));
        let [Call::Process(Pipe::Close(_), 1, EVENT_CLOSE)] = calls()[..] else {
            panic!("one forwarded close");
        };
        // Otherwise the close goes to the reactor thread, once.
        assert!(end(serv, 2, 0));
        assert_eq!(calls(), [Call::Process(Pipe::Reactor(0), 2, EVENT_CLOSE)]);
        let conn = session(&live, 2);
        conn.closed().store(1, Relaxed);
        assert!(!end(serv, 2, 0));
        conn.set_closing(true);
        assert!(!end(serv, 2, 0));
        assert!(!end(serv, 99, 0));
        assert_eq!(calls(), []);
    }
}
