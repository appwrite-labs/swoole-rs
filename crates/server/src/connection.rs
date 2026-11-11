//! Accepting connections and their idle and health checks.
use crate::ops::{
    LOG_ACCEPT_FAILED, LOG_MAX_CONNECTION_CREATED, LOG_MAX_CONNECTION_SESSIONS,
    LOG_MAX_CONNECTION_SOCKETS, LOG_NO_SESSION_SLOT, LOG_NODELAY, LOG_RCVBUF, LOG_SNDBUF,
    LOG_TOO_MANY_CONNECTIONS, OP_ABORT, OP_ACCEPT, OP_ACCEPT_TIMER, OP_ACCEPT_TIMER_CLEAR,
    OP_ADD_RECV_TIMER, OP_CHUNK_SIZE, OP_CLOSE_CONNECTION, OP_CONN_CLOSING, OP_CONN_INIT,
    OP_CONN_PROTECT, OP_CONN_RESET, OP_CONNECTION_LIST, OP_COUNT_ACCEPT, OP_DELAY_RECEIVE,
    OP_DIRECT_SEND, OP_DISABLE_NOTIFY, OP_ERRNO, OP_EVENT_FD, OP_FORCE_CLOSE, OP_HAS_ON_CONNECT,
    OP_LAST_RECEIVED, OP_LAST_SENT, OP_LOCK, OP_MAX_CONNECTION, OP_MAX_SOCKETS, OP_MAXFD, OP_MINFD,
    OP_NODELAY_ENABLED, OP_NOTIFY_CONNECT, OP_NOW_MS, OP_PORT_COUNT, OP_PORT_DGRAM,
    OP_PORT_EVENT_ADD, OP_PORT_EVENT_DEL, OP_PORT_IDLE_TIME, OP_PORT_NODELAY, OP_PORT_RCVBUF,
    OP_PORT_SNDBUF, OP_PORT_SSL, OP_REACTOR_ADD_READ, OP_SEND_INCOMING, OP_SESSION_BIND,
    OP_SESSION_CLAIM, OP_SESSION_ROUND, OP_SET_MAX_CONNECTION, OP_SET_MAXFD, OP_SET_MINFD,
    OP_SET_NODELAY, OP_SET_RCVBUF, OP_SET_SNDBUF, OP_SINGLE_THREAD, OP_SOCK_FD, OP_SOCK_SSL,
    OP_SPIN_LOCK, OP_SPIN_UNLOCK, OP_SSL_CLEAR, OP_SSL_CREATE, OP_UNLOCK,
};
use crate::{Host, SW_ERR, SW_OK};

/// `SW_ACCEPT_MAX_COUNT`, `SW_MAX_CONNECTION`, `SW_SESSION_LIST_SIZE`; native asserts each.
const ACCEPT_MAX_COUNT: u32 = 64;
const MAX_CONNECTION: u32 = 100_000;
const SESSION_LIST_SIZE: u32 = 1024 * 1024;

/// `Server::accept_connection`: up to `SW_ACCEPT_MAX_COUNT` sockets per readable event.
pub(crate) fn accept<H: Host>(h: &mut H) -> crate::Effect<i64> {
    for _ in 0..ACCEPT_MAX_COUNT {
        if !h.is(OP_ACCEPT)? {
            let errno = i32::try_from(h.get(OP_ERRNO)?).unwrap_or(0);
            if errno == libc::EAGAIN {
                return Ok(SW_OK);
            }
            if errno == libc::EINTR {
                continue;
            }
            if errno == libc::EMFILE || errno == libc::ENFILE {
                disable_accept(h)?;
            }
            h.get(LOG_ACCEPT_FAILED)?;
            return Ok(SW_OK);
        }
        if h.signed(OP_SOCK_FD)? >= i64::from(max_connection(h)?) {
            h.get(LOG_TOO_MANY_CONNECTIONS)?;
            h.get(OP_ABORT)?;
            disable_accept(h)?;
            return Ok(SW_OK);
        }
        if h.is(OP_PORT_SSL)? {
            if !h.is(OP_SSL_CREATE)? {
                h.get(OP_ABORT)?;
                return Ok(SW_OK);
            }
        } else {
            h.get(OP_SSL_CLEAR)?;
        }
        let server_fd = h.get(OP_EVENT_FD)?;
        if !add(h, server_fd)? {
            h.get(OP_ABORT)?;
            return Ok(SW_OK);
        }
        h.get(OP_CHUNK_SIZE)?;
        let started = if h.is(OP_SINGLE_THREAD)? {
            incoming(h)? >= 0
        } else {
            h.signed(OP_SEND_INCOMING)? >= 0
        };
        if !started {
            h.get(OP_ABORT)?;
            return Ok(SW_OK);
        }
    }
    Ok(SW_OK)
}

fn max_connection<H: Host>(h: &mut H) -> crate::Effect<u32> {
    Ok(u32::try_from(h.get(OP_MAX_CONNECTION)?).unwrap_or(u32::MAX))
}

/// `Server::add_connection` with the server fd native code stored in the context.
pub(crate) fn add_connection<H: Host>(h: &mut H) -> crate::Effect<bool> {
    let server_fd = h.get(OP_EVENT_FD)?;
    add(h, server_fd)
}

/// Takes the next free session slot after `session_round`, then sets up the
/// socket and connection. Inlined into `accept`, whose runner's constant
/// context folds each op's checks (#2964).
#[inline(always)]
fn add<H: Host>(h: &mut H, server_fd: u64) -> crate::Effect<bool> {
    h.get(OP_CONN_RESET)?;
    h.get(OP_SPIN_LOCK)?;
    let mut session_id = h.signed(OP_SESSION_ROUND)?;
    let mut found = false;
    for _ in 0..max_connection(h)? {
        session_id += 1;
        if h.op(OP_SESSION_CLAIM, session_id.cast_unsigned())? != 0 {
            found = true;
            break;
        }
    }
    h.get(OP_SPIN_UNLOCK)?;
    if !found {
        h.get(LOG_NO_SESSION_SLOT)?;
        return Ok(false);
    }
    h.op(OP_SESSION_BIND, session_id.cast_unsigned())?;

    if h.is(OP_PORT_NODELAY)? {
        if !h.is(OP_SET_NODELAY)? {
            h.get(LOG_NODELAY)?;
        }
        h.get(OP_NODELAY_ENABLED)?;
    }
    if h.signed(OP_PORT_RCVBUF)? > 0 && h.signed(OP_SET_RCVBUF)? != 0 {
        h.get(LOG_RCVBUF)?;
    }
    if h.signed(OP_PORT_SNDBUF)? > 0 && h.signed(OP_SET_SNDBUF)? != 0 {
        h.get(LOG_SNDBUF)?;
    }
    h.op(OP_CONN_INIT, server_fd)?;
    if !h.is(OP_PORT_SSL)? {
        h.get(OP_DIRECT_SEND)?;
    }

    h.get(OP_LOCK)?;
    let fd = h.signed(OP_SOCK_FD)?;
    if fd > h.signed(OP_MAXFD)? {
        h.op(OP_SET_MAXFD, fd.cast_unsigned())?;
    } else if fd < h.signed(OP_MINFD)? {
        h.op(OP_SET_MINFD, fd.cast_unsigned())?;
    }
    h.get(OP_UNLOCK)?;
    h.get(OP_COUNT_ACCEPT)?;
    Ok(true)
}

/// `Server::connection_incoming`: the idle timer, reading, and `onConnect`.
pub(crate) fn incoming<H: Host>(h: &mut H) -> crate::Effect<i64> {
    if h.float(OP_PORT_IDLE_TIME)? > 0.0 {
        h.get(OP_ADD_RECV_TIMER)?;
    }
    if h.is(OP_SOCK_SSL)? {
        return h.signed(OP_REACTOR_ADD_READ);
    }
    // Delayed receive waits for the resume command.
    if !h.is(OP_DELAY_RECEIVE)? && h.signed(OP_REACTOR_ADD_READ)? < 0 {
        return Ok(SW_ERR);
    }
    if h.is(OP_HAS_ON_CONNECT)? && !h.is(OP_NOTIFY_CONNECT)? {
        return Ok(SW_ERR);
    }
    Ok(SW_OK)
}

/// The idle timer: closes a connection idle in both directions for `max_idle_time`.
pub(crate) fn idle_timeout<H: Host>(h: &mut H) -> crate::Effect<()> {
    if h.is(OP_CONN_PROTECT)? {
        return Ok(());
    }
    let now = h.float(OP_NOW_MS)?;
    let idle = h.float(OP_PORT_IDLE_TIME)?;
    if now - h.float(OP_LAST_RECEIVED)? < idle && now - h.float(OP_LAST_SENT)? < idle {
        return Ok(());
    }
    if h.is(OP_DISABLE_NOTIFY)? || h.is(OP_CONN_CLOSING)? {
        h.get(OP_CLOSE_CONNECTION)?;
        return Ok(());
    }
    h.get(OP_FORCE_CLOSE)?;
    Ok(())
}

/// `Server::disable_accept`: stop accepting on stream ports until the retry timer.
pub(crate) fn disable_accept<H: Host>(h: &mut H) -> crate::Effect<()> {
    if !h.is(OP_ACCEPT_TIMER)? {
        return Ok(());
    }
    for_stream_ports(h, OP_PORT_EVENT_DEL)?;
    Ok(())
}

/// The accept retry timer: accept on stream ports again.
pub(crate) fn enable_accept<H: Host>(h: &mut H) -> crate::Effect<()> {
    for_stream_ports(h, OP_PORT_EVENT_ADD)?;
    h.get(OP_ACCEPT_TIMER_CLEAR)?;
    Ok(())
}

fn for_stream_ports<H: Host>(h: &mut H, op: u32) -> crate::Effect<()> {
    for index in 0..h.get(OP_PORT_COUNT)? {
        if h.op(OP_PORT_DGRAM, index)? == 0 {
            h.op(op, index)?;
        }
    }
    Ok(())
}

/// `Server::set_max_connection`: 0 means the default, bounded by the session list
/// and `SwooleG.max_sockets`.
pub(crate) fn set_max_connection<H: Host>(h: &mut H, requested: u32) -> crate::Effect<()> {
    if h.is(OP_CONNECTION_LIST)? {
        h.get(LOG_MAX_CONNECTION_CREATED)?;
        return Ok(());
    }
    let max_sockets = u32::try_from(h.get(OP_MAX_SOCKETS)?).unwrap_or(u32::MAX);
    let mut value = requested;
    if value == 0 {
        value = MAX_CONNECTION.min(max_sockets);
    } else if value > SESSION_LIST_SIZE {
        value = SESSION_LIST_SIZE;
        h.get(LOG_MAX_CONNECTION_SESSIONS)?;
    }
    if max_sockets > 0 && value > max_sockets {
        value = max_sockets;
        h.op(LOG_MAX_CONNECTION_SOCKETS, u64::from(max_sockets))?;
    }
    h.op(OP_SET_MAX_CONNECTION, u64::from(value))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::Script;

    fn accepting(extra: &[(u32, u64)]) -> Script {
        let mut facts = vec![
            (OP_MAX_CONNECTION, 1000),
            (OP_SOCK_FD, 10),
            (OP_SESSION_CLAIM, 1),
            (OP_MAXFD, 5),
            (OP_MINFD, 3),
            (OP_ACCEPT_TIMER, 1),
            (OP_PORT_COUNT, 2),
            (OP_EVENT_FD, 4),
        ];
        facts.extend_from_slice(extra);
        Script::with(&facts)
    }

    #[test]
    fn accepts_until_eagain_and_hands_each_to_a_reactor_thread() {
        let mut s = accepting(&[(OP_ERRNO, libc::EAGAIN as u64)]).queue(OP_ACCEPT, &[1, 1, 0]);
        assert_eq!(accept(&mut s).unwrap(), SW_OK, "EAGAIN ends the batch");
        assert_eq!(
            s.called(OP_SEND_INCOMING),
            2,
            "one incoming event per socket"
        );
        assert_eq!(s.called(OP_SSL_CLEAR), 2, "plain ports clear ssl");
        assert_eq!(
            s.args(OP_SET_MAXFD),
            vec![10, 10],
            "maxfd grows to the new fd"
        );
        assert_eq!(s.args(OP_CONN_INIT), vec![4, 4], "server fd from the event");
        assert_eq!(s.called(OP_ABORT), 0, "nothing aborted");
        assert_eq!(s.called(OP_DIRECT_SEND), 2, "plain sockets send directly");
    }

    #[test]
    fn eintr_retries_and_emfile_disables_accept() {
        let mut s = accepting(&[]).queue(OP_ERRNO, &[libc::EINTR as u64, libc::EMFILE as u64]);
        s.queued.insert(OP_ACCEPT, vec![0, 0]);
        assert_eq!(
            accept(&mut s).unwrap(),
            SW_OK,
            "accept errors are not fatal"
        );
        assert_eq!(s.called(OP_ACCEPT), 2, "EINTR retried once");
        assert_eq!(s.args(OP_PORT_DGRAM), vec![0, 1], "every port checked");
        assert_eq!(s.called(OP_PORT_EVENT_DEL), 2, "stream ports stop reading");
        assert_eq!(s.called(LOG_ACCEPT_FAILED), 1, "warned after disabling");
    }

    #[test]
    fn too_many_connections_aborts_and_disables() {
        let mut s = accepting(&[(OP_SOCK_FD, 1000)]).queue(OP_ACCEPT, &[1]);
        assert_eq!(accept(&mut s).unwrap(), SW_OK, "a full table is not fatal");
        assert_eq!(s.called(LOG_TOO_MANY_CONNECTIONS), 1, "warned");
        assert_eq!(s.called(OP_ABORT), 1, "socket aborted");
        assert_eq!(s.called(OP_CONN_RESET), 0, "no connection made");
    }

    #[test]
    fn ssl_failure_and_full_session_list_abort() {
        let mut s = accepting(&[(OP_PORT_SSL, 1)]).queue(OP_ACCEPT, &[1]);
        accept(&mut s).unwrap();
        assert_eq!(s.called(OP_ABORT), 1, "ssl_create failed");

        let mut s = accepting(&[
            (OP_SESSION_CLAIM, 0),
            (OP_MAX_CONNECTION, 3),
            (OP_SOCK_FD, 1),
        ])
        .queue(OP_ACCEPT, &[1]);
        accept(&mut s).unwrap();
        assert_eq!(
            s.args(OP_SESSION_CLAIM),
            vec![1, 2, 3],
            "max_connection slots tried"
        );
        assert_eq!(s.called(OP_SPIN_UNLOCK), 1, "lock released");
        assert_eq!(s.called(LOG_NO_SESSION_SLOT), 1, "warned");
        assert_eq!(s.called(OP_ABORT), 1, "socket aborted");
    }

    #[test]
    fn session_ids_continue_from_the_round() {
        let mut s = accepting(&[(OP_SESSION_ROUND, 41)]).queue(OP_SESSION_CLAIM, &[0, 1]);
        assert!(add_connection(&mut s).unwrap(), "second slot free");
        assert_eq!(s.args(OP_SESSION_BIND), vec![43], "bound to the claimed id");
    }

    #[test]
    fn socket_options_warn_on_failure() {
        let mut s = accepting(&[
            (OP_PORT_NODELAY, 1),
            (OP_PORT_RCVBUF, 4096),
            (OP_SET_RCVBUF, u64::MAX),
            (OP_PORT_SNDBUF, 4096),
            (OP_SOCK_FD, 1),
        ]);
        assert!(add_connection(&mut s).unwrap(), "added");
        assert_eq!(s.called(LOG_NODELAY), 1, "nodelay failed");
        assert_eq!(s.called(OP_NODELAY_ENABLED), 1, "flag set regardless");
        assert_eq!(s.called(LOG_RCVBUF), 1, "rcvbuf failed");
        assert_eq!(s.called(LOG_SNDBUF), 0, "sndbuf succeeded");
        assert_eq!(s.args(OP_SET_MINFD), vec![1], "minfd shrinks");
    }

    #[test]
    fn single_thread_runs_incoming_inline() {
        let mut s =
            accepting(&[(OP_SINGLE_THREAD, 1), (OP_HAS_ON_CONNECT, 1)]).queue(OP_ACCEPT, &[1]);
        accept(&mut s).unwrap();
        assert_eq!(s.called(OP_REACTOR_ADD_READ), 1, "reading");
        assert_eq!(s.called(OP_ABORT), 1, "onConnect notify failed");
    }

    #[test]
    fn incoming_orders_timer_ssl_and_delay() {
        let mut s = Script::with(&[(OP_PORT_IDLE_TIME, 1.5f64.to_bits()), (OP_SOCK_SSL, 1)]);
        assert_eq!(
            incoming(&mut s).unwrap(),
            0,
            "ssl adds read regardless of delay"
        );
        assert_eq!(s.called(OP_ADD_RECV_TIMER), 1, "idle timer armed");

        let mut s = Script::with(&[
            (OP_DELAY_RECEIVE, 1),
            (OP_HAS_ON_CONNECT, 1),
            (OP_NOTIFY_CONNECT, 1),
        ]);
        assert_eq!(incoming(&mut s).unwrap(), SW_OK, "delayed");
        assert_eq!(s.called(OP_REACTOR_ADD_READ), 0, "waits for resume");

        let mut s = Script::with(&[(OP_REACTOR_ADD_READ, u64::MAX)]);
        assert_eq!(incoming(&mut s).unwrap(), SW_ERR, "reactor add failed");
    }

    #[test]
    fn idle_timeout_closes_only_idle_connections() {
        let facts = |recv: f64, notify: u64| {
            Script::with(&[
                (OP_NOW_MS, 10_000f64.to_bits()),
                (OP_PORT_IDLE_TIME, 5000f64.to_bits()),
                (OP_LAST_RECEIVED, recv.to_bits()),
                (OP_LAST_SENT, 9000f64.to_bits()),
                (OP_DISABLE_NOTIFY, notify),
            ])
        };
        let mut s = facts(8000.0, 0);
        idle_timeout(&mut s).unwrap();
        assert_eq!(
            s.called(OP_FORCE_CLOSE) + s.called(OP_CLOSE_CONNECTION),
            0,
            "active"
        );

        let mut s = facts(1000.0, 0);
        idle_timeout(&mut s).unwrap();
        assert_eq!(
            s.called(OP_FORCE_CLOSE),
            1,
            "idle receive closes with notify"
        );

        let mut s = facts(1000.0, 1);
        idle_timeout(&mut s).unwrap();
        assert_eq!(
            s.called(OP_CLOSE_CONNECTION),
            1,
            "no notify closes directly"
        );

        let mut s = Script::with(&[(OP_CONN_PROTECT, 1)]);
        idle_timeout(&mut s).unwrap();
        assert_eq!(s.called(OP_NOW_MS), 0, "protected");
    }

    #[test]
    fn enable_accept_skips_dgram_ports() {
        let mut s = Script::with(&[(OP_PORT_COUNT, 2)]).queue(OP_PORT_DGRAM, &[1, 0]);
        enable_accept(&mut s).unwrap();
        assert_eq!(s.args(OP_PORT_EVENT_ADD), vec![1], "stream port only");
        assert_eq!(s.called(OP_ACCEPT_TIMER_CLEAR), 1, "timer cleared");

        let mut s = Script::with(&[(OP_PORT_COUNT, 2)]);
        disable_accept(&mut s).unwrap();
        assert_eq!(
            s.called(OP_PORT_EVENT_DEL),
            0,
            "timer failed, ports untouched"
        );
    }

    #[test]
    fn max_connection_bounds() {
        let run = |requested, sockets, created| {
            let mut s = Script::with(&[(OP_MAX_SOCKETS, sockets), (OP_CONNECTION_LIST, created)]);
            set_max_connection(&mut s, requested).unwrap();
            (
                s.args(OP_SET_MAX_CONNECTION),
                s.log
                    .iter()
                    .map(|(op, _)| *op)
                    .filter(|op| *op >= 120)
                    .collect::<Vec<_>>(),
            )
        };
        assert_eq!(
            run(0, 1024, 0),
            (vec![1024], vec![]),
            "default bounded by sockets"
        );
        assert_eq!(run(0, 0, 0), (vec![0], vec![]), "unknown sockets");
        assert_eq!(run(0, 200_000, 0), (vec![100_000], vec![]), "default");
        assert_eq!(
            run(2_000_000, 0, 0),
            (vec![1024 * 1024], vec![LOG_MAX_CONNECTION_SESSIONS]),
            "session list bound"
        );
        assert_eq!(
            run(5000, 1024, 0),
            (vec![1024], vec![LOG_MAX_CONNECTION_SOCKETS]),
            "socket bound"
        );
        assert_eq!(
            run(5000, 1024, 1),
            (vec![], vec![LOG_MAX_CONNECTION_CREATED]),
            "too late"
        );
    }
}
