//! A connection's read, write and close paths (`src/server/reactor_thread.cc`):
//! `ReactorThread_onRead`, `ReactorThread_onWrite`, `ReactorThread_onClose` and
//! `Server::close_connection`. Native code passes each entry's facts as its argument
//! (the fact bits below), so the hot read path makes no call besides the port's
//! own `onRead`.
//!
//! `--fix-halfclose` (M15) builds have the `halfclose` feature, which reports
//! `PREQUEUE` (upstream's `SW_HALFCLOSE_PREQUEUE`): a synchronous process worker keeps its ordered response after a
//! clean FIN, `close_notified` is the terminal read state, and a TLS close notify
//! still owed is queued behind the output (`native/halfclose/prequeue.md`).
use crate::ops::{
    LOG_CC_LINGER, OP_CC_COUNT, OP_CC_DTLS_FREE, OP_CC_FINISH, OP_CC_HTTP_DESTROY,
    OP_CC_HTTP_OBJECT, OP_CC_LINGER, OP_CC_QUEUE_CLOSE, OP_CC_RECV_FREE, OP_CC_REDIS_FREE,
    OP_CC_REDIS_OBJECT, OP_CC_SESSION_CLEAR, OP_CC_SSL_CLOSE, OP_CC_SSL_SHUTDOWN, OP_CC_SSL_WANT,
    OP_CC_TIMER_DEL, OP_CC_VALID, OP_CC_WATCH_WRITE, OP_CC_WEBSOCKET_FREE, OP_CLOSE, OP_LOCK,
    OP_MAXFD, OP_MINFD, OP_PC_NOTIFY, OP_RD_ABORT, OP_RD_DTLS, OP_RD_PORT_READ, OP_RD_UNREAD,
    OP_RD_WRITE_FACTS, OP_REACTOR_DEL, OP_RESUME_TIMER, OP_RT_REMOVE_WRITE, OP_SET_MAXFD,
    OP_SET_WAITING_TIME, OP_UNLOCK, OP_WR_BUFFER_EMPTY, OP_WR_CLEAR_OVERFLOW, OP_WR_CLOSE,
    OP_WR_CLOSE_ERRNO, OP_WR_FRONT, OP_WR_NOTIFY_CLOSE, OP_WR_QUEUED, OP_WR_READ_ONLY, OP_WR_SEND,
    OP_WR_SENDFILE, OP_WR_STATE, OP_WR_TIMER_DEL, OP_WR_TIMER_DELAY, OP_WR_WAIT,
};
use crate::reactor_thread::{SW_CONTINUE, SW_ERROR, SW_READY, SW_WAIT, verify_ssl};
use crate::{Host, SW_ERR, SW_OK};

// The fact bits.
/// The connection exists (and, for a read, belongs to a port; else is active).
pub(crate) const FOUND: u64 = 1 << 0;
pub(crate) const CLOSE_NOTIFIED: u64 = 1 << 1;
pub(crate) const CLOSE_QUEUED: u64 = 1 << 2;
/// The socket has TLS that wants to read.
pub(crate) const SSL_WANT_READ: u64 = 1 << 3;
/// The socket has TLS whose handshake is not finished.
pub(crate) const SSL_PENDING: u64 = 1 << 4;
pub(crate) const DTLS_PORT: u64 = 1 << 5;
pub(crate) const DTLS_SOCKET: u64 = 1 << 6;
/// Built with the `halfclose` feature.
pub(crate) const PREQUEUE: u64 = 1 << 7;
pub(crate) const PROCESS: u64 = 1 << 8;
pub(crate) const CLOSE_NOTIFY: u64 = 1 << 9;
pub(crate) const DISABLE_NOTIFY: u64 = 1 << 10;
pub(crate) const CLOSE_FORCE: u64 = 1 << 11;
pub(crate) const CLOSE_RESET: u64 = 1 << 12;
pub(crate) const PEER_CLOSED: u64 = 1 << 13;
pub(crate) const HASH_DISPATCH: u64 = 1 << 14;
pub(crate) const DISPATCH_FUNC: u64 = 1 << 15;
pub(crate) const COROUTINE: u64 = 1 << 16;
pub(crate) const CONN_SSL: u64 = 1 << 17;
pub(crate) const SOCKET_SSL: u64 = 1 << 18;
pub(crate) const CLOSE_ERRNO: u64 = 1 << 19;
pub(crate) const TIMER: u64 = 1 << 20;
pub(crate) const WEBSOCKET_BUFFER: u64 = 1 << 21;
pub(crate) const REMOVED: u64 = 1 << 22;
pub(crate) const BASE: u64 = 1 << 23;
pub(crate) const RECV_BUFFER: u64 = 1 << 24;
/// Built with `SW_USE_SOCKET_LINGER`.
const LINGER: u64 = 1 << 25;
/// The socket's TLS is ready and it is a stream.
pub(crate) const TLS_READY_STREAM: u64 = 1 << 26;
pub(crate) const SSL_CLOSED: u64 = 1 << 27;
/// `overflow` is set and the output is below the socket's buffer size.
pub(crate) const OVERFLOW_DRAINED: u64 = 1 << 28;
/// `onBufferEmpty` is set, the connection is over its high watermark and the output
/// is down to the port's low watermark.
pub(crate) const LOW_WATERMARK: u64 = 1 << 29;
pub(crate) const SEND_TIMER: u64 = 1 << 30;
pub(crate) const OUT_EMPTY: u64 = 1 << 31;
/// `OP_RD_PORT_READ`'s bits above its result: the connection is still active, then
/// (only while it is) the workers hold more than `max_queued_bytes` and the read
/// notified the close.
pub(crate) const ACTIVE: u64 = 1 << 32;
pub(crate) const QUEUE_OVER: u64 = 1 << 33;
pub(crate) const READ_NOTIFIED: u64 = 1 << 34;
/// `OP_WR_WAIT`'s bits.
pub(crate) const CLOSE_WAIT: u64 = 1 << 35;
pub(crate) const SEND_WAIT: u64 = 1 << 36;

/// The low 32 bits, where native code packs a C `int`.
fn low(bits: u64) -> u32 {
    u32::try_from(bits & u64::from(u32::MAX)).unwrap_or_default()
}

/// `BufferChunk::Type`, as `OP_WR_FRONT` returns it (plus one; 0 is an empty buffer).
const KIND_SENDFILE: u64 = 1;
const KIND_CLOSE: u64 = 2;

/// `ReactorThread_onRead`: finish a pending TLS (or DTLS) handshake, then let the
/// port read, and pause reading while the workers hold too many of its bytes.
pub(crate) fn read<H: Host>(h: &mut H, facts: u64) -> crate::Effect<i64> {
    // The server closed the connection (and the fd may be reused) before the event.
    if facts & FOUND == 0 {
        return Ok(SW_OK);
    }
    let prequeue = facts & PREQUEUE != 0;
    if prequeue {
        if facts & CLOSE_NOTIFIED != 0 {
            return Ok(SW_OK);
        }
        // Once a close is queued, a read only drives a TLS shutdown that wants one.
        if facts & CLOSE_QUEUED != 0 {
            return Ok(if facts & SSL_WANT_READ != 0 {
                let facts = h.get(OP_RD_WRITE_FACTS)?;
                write(h, facts)?
            } else {
                SW_OK
            });
        }
    }
    if facts & DTLS_PORT != 0 && h.is(OP_RD_DTLS)? {
        h.get(OP_RD_ABORT)?;
        return Ok(SW_OK);
    }
    let code = if facts & SSL_PENDING != 0 {
        verify_ssl(h)?
    } else {
        SW_CONTINUE
    };
    match code {
        SW_ERROR => {
            h.get(OP_RD_ABORT)?;
            return Ok(SW_OK);
        }
        SW_READY if facts & DTLS_SOCKET != 0 => return Ok(SW_OK),
        SW_WAIT => return Ok(SW_OK),
        SW_READY | SW_CONTINUE => {}
        _ => std::process::abort(),
    }
    let result = h.get(OP_RD_PORT_READ)?;
    let retval = i64::from(low(result).cast_signed());
    if result & ACTIVE == 0 || prequeue && result & READ_NOTIFIED != 0 {
        return Ok(retval);
    }
    if facts & PROCESS != 0 && result & QUEUE_OVER != 0 {
        h.op(OP_SET_WAITING_TIME, 1)?;
        if h.is(OP_RESUME_TIMER)? {
            h.get(OP_RD_UNREAD)?;
        }
    }
    Ok(retval)
}

/// `ReactorThread_onWrite`: notify a pending close, or flush the output buffer, then
/// report the drained buffer and stop watching for writes once it is empty.
pub(crate) fn write<H: Host>(h: &mut H, facts: u64) -> crate::Effect<i64> {
    if facts & FOUND == 0 {
        return Ok(SW_ERR);
    }
    if facts & CLOSE_NOTIFY != 0 {
        if facts & SSL_PENDING != 0 {
            return h.signed(OP_CLOSE);
        }
        h.get(OP_WR_NOTIFY_CLOSE)?;
        return Ok(SW_OK);
    } else if facts & DISABLE_NOTIFY != 0 && facts & CLOSE_FORCE != 0 {
        return h.signed(OP_CLOSE);
    }
    loop {
        let ret = match h.get(OP_WR_FRONT)? {
            0 => break,
            front if front - 1 == KIND_CLOSE => return h.signed(OP_WR_CLOSE),
            front if front - 1 == KIND_SENDFILE => h.signed(OP_WR_SENDFILE)?,
            _ => {
                let ret = h.signed(OP_WR_SEND)?;
                if ret == SW_OK {
                    h.get(OP_WR_QUEUED)?;
                }
                ret
            }
        };
        if ret < 0 {
            let wait = h.get(OP_WR_WAIT)?;
            if wait & CLOSE_WAIT != 0 {
                return h.signed(OP_WR_CLOSE_ERRNO);
            } else if wait & SEND_WAIT != 0 {
                break;
            }
        }
    }
    let mut state = h.get(OP_WR_STATE)?;
    if state & OVERFLOW_DRAINED != 0 {
        h.get(OP_WR_CLEAR_OVERFLOW)?;
    }
    if state & LOW_WATERMARK != 0 {
        h.get(OP_WR_BUFFER_EMPTY)?;
        // onBufferEmpty may have written more.
        state = h.get(OP_WR_STATE)?;
    }
    if state & SEND_TIMER != 0 {
        h.get(if state & OUT_EMPTY != 0 {
            OP_WR_TIMER_DEL
        } else {
            OP_WR_TIMER_DELAY
        })?;
    }
    if state & (PEER_CLOSED | REMOVED) == 0 && state & OUT_EMPTY != 0 {
        // A notified close no longer reads: it only drains.
        h.get(if facts & PREQUEUE != 0 {
            OP_RT_REMOVE_WRITE
        } else {
            OP_WR_READ_ONLY
        })?;
    }
    Ok(SW_OK)
}

/// `ReactorThread_onClose`: the peer closed. Close now when nobody is told or a
/// close is already queued, else mark the connection and notify the worker once.
pub(crate) fn peer_close<H: Host>(h: &mut H, facts: u64) -> crate::Effect<i64> {
    if facts & FOUND == 0 {
        return Ok(SW_ERR);
    }
    if facts & DISABLE_NOTIFY != 0 {
        return h.signed(OP_CLOSE);
    }
    if h.signed(OP_REACTOR_DEL)? != 0 {
        return Ok(SW_ERR);
    }
    if facts & CLOSE_QUEUED != 0 {
        return h.signed(OP_CLOSE);
    }
    if facts & CLOSE_NOTIFIED != 0 {
        return Ok(SW_OK);
    }
    Ok(h.op(OP_PC_NOTIFY, u64::from(peer_closed(facts)))?
        .cast_signed())
}

/// Whether the peer's FIN ends the connection. A synchronous process worker with
/// fixed-worker dispatch still owes its ordered response after a clean FIN; other
/// modes, TLS and fatal closes are terminal (always, without `PREQUEUE`).
fn peer_closed(facts: u64) -> bool {
    facts & PREQUEUE == 0
        || facts & PROCESS == 0
        || facts & HASH_DISPATCH == 0
        || facts & (DISPATCH_FUNC | COROUTINE | CONN_SSL | CLOSE_ERRNO | CLOSE_FORCE | CLOSE_RESET)
            != 0
}

/// `Server::close_connection`: release what the connection holds, count the close,
/// clear its session and move the max fd down past it, then close the socket.
/// `facts` are its fact bits, the socket's fd above bit 32.
pub(crate) fn close_connection<H: Host>(h: &mut H, facts: u64) -> crate::Effect<i64> {
    let fd = i64::from(low(facts >> 32).cast_signed());
    if facts & TIMER != 0 {
        h.get(OP_CC_TIMER_DEL)?;
    }
    let removed = facts & REMOVED != 0;
    // A TLS close notify that cannot be sent now goes behind the queued output.
    if facts & PREQUEUE != 0
        && facts & TLS_READY_STREAM != 0
        && facts & (PEER_CLOSED | CLOSE_FORCE | CLOSE_RESET | SSL_CLOSED) == 0
        && !h.is(OP_CC_SSL_SHUTDOWN)?
        && h.is(OP_CC_SSL_WANT)?
    {
        if facts & CLOSE_QUEUED == 0 {
            h.get(OP_CC_QUEUE_CLOSE)?;
        }
        if h.op(OP_CC_WATCH_WRITE, u64::from(removed))?.cast_signed() == SW_OK {
            return Ok(SW_OK);
        }
    }
    if facts & WEBSOCKET_BUFFER != 0 {
        h.get(OP_CC_WEBSOCKET_FREE)?;
    }
    if !removed && h.signed(OP_REACTOR_DEL)? < 0 {
        return Ok(SW_ERR);
    }
    h.op(OP_CC_COUNT, u64::from(facts & BASE != 0))?;
    if facts & SOCKET_SSL != 0 {
        h.get(OP_CC_SSL_CLOSE)?;
    }
    if facts & DTLS_SOCKET != 0 {
        h.get(OP_CC_DTLS_FREE)?;
    }
    if facts & RECV_BUFFER != 0 {
        h.get(OP_CC_RECV_FREE)?;
    }
    if h.is(OP_CC_HTTP_OBJECT)? {
        h.get(OP_CC_HTTP_DESTROY)?;
    }
    if h.is(OP_CC_REDIS_OBJECT)? {
        h.get(OP_CC_REDIS_FREE)?;
    }
    if facts & LINGER != 0 && facts & (CLOSE_FORCE | CLOSE_RESET) != 0 && h.is(OP_CC_LINGER)? {
        h.get(LOG_CC_LINGER)?;
    }
    h.get(OP_CC_SESSION_CLEAR)?;
    // The connection list's max fd, if it was this one.
    h.get(OP_LOCK)?;
    if fd == h.signed(OP_MAXFD)? {
        let mut find = fd - 1;
        while h.op(OP_CC_VALID, find.cast_unsigned())? == 0 && find > h.signed(OP_MINFD)? {
            find -= 1;
        }
        h.op(OP_SET_MAXFD, find.cast_unsigned())?;
    }
    h.get(OP_UNLOCK)?;
    h.signed(OP_CC_FINISH)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::{OP_SSL_ACCEPT, OP_SSL_READY};
    use crate::script::Script;

    const ON_READ: u64 = 42;

    fn read_with(facts: u64, script: Script) -> (i64, Script) {
        let mut h = script;
        let code = read(&mut h, facts).unwrap();
        (code, h)
    }

    #[test]
    fn read_lets_the_port_read() {
        let (code, h) = read_with(FOUND, Script::with(&[(OP_RD_PORT_READ, ACTIVE | ON_READ)]));
        assert_eq!(code, 42);
        assert_eq!(h.log, vec![(OP_RD_PORT_READ, 0)], "one native call");
        let (code, h) = read_with(0, Script::default());
        assert_eq!((code, h.log.len()), (SW_OK, 0), "a stale event");
        // The port's result is a C int.
        let (code, _) = read_with(
            FOUND,
            Script::with(&[(OP_RD_PORT_READ, u64::from(u32::MAX))]),
        );
        assert_eq!(code, SW_ERR);
    }

    #[test]
    fn read_pauses_a_process_connection_over_its_queue() {
        let over = ACTIVE | QUEUE_OVER;
        let (_, h) = read_with(
            FOUND | PROCESS,
            Script::with(&[(OP_RD_PORT_READ, over), (OP_RESUME_TIMER, 1)]),
        );
        assert_eq!(h.args(OP_SET_WAITING_TIME), vec![1]);
        assert_eq!(h.called(OP_RD_UNREAD), 1);
        let (_, h) = read_with(FOUND | PROCESS, Script::with(&[(OP_RD_PORT_READ, over)]));
        assert_eq!(h.called(OP_RD_UNREAD), 0, "no timer, keep reading");
        let (_, h) = read_with(FOUND, Script::with(&[(OP_RD_PORT_READ, over)]));
        assert_eq!(h.called(OP_SET_WAITING_TIME), 0, "base mode");
        let (_, h) = read_with(
            FOUND | PROCESS,
            Script::with(&[(OP_RD_PORT_READ, QUEUE_OVER)]),
        );
        assert_eq!(h.called(OP_SET_WAITING_TIME), 0, "closed by the read");
    }

    #[test]
    fn read_finishes_handshakes_first() {
        let (code, h) = read_with(FOUND | SSL_PENDING, Script::with(&[(OP_SSL_ACCEPT, 2)]));
        assert_eq!((code, h.called(OP_RD_PORT_READ)), (SW_OK, 0), "waits");
        let (code, h) = read_with(FOUND | SSL_PENDING, Script::with(&[(OP_SSL_ACCEPT, 4)]));
        assert_eq!((code, h.called(OP_RD_ABORT)), (SW_OK, 1), "fails");
        let (_, h) = read_with(
            FOUND | SSL_PENDING,
            Script::with(&[(OP_SSL_ACCEPT, 5), (OP_RD_PORT_READ, ACTIVE)]),
        );
        assert_eq!(
            (h.called(OP_SSL_READY), h.called(OP_RD_PORT_READ)),
            (1, 1),
            "ready reads"
        );
        let (_, h) = read_with(
            FOUND | SSL_PENDING | DTLS_SOCKET,
            Script::with(&[(OP_SSL_ACCEPT, 5)]),
        );
        assert_eq!(
            h.called(OP_RD_PORT_READ),
            0,
            "a ready DTLS session does not read"
        );
        let (_, h) = read_with(FOUND | DTLS_PORT, Script::with(&[(OP_RD_DTLS, 1)]));
        assert_eq!(
            (h.called(OP_RD_ABORT), h.called(OP_RD_PORT_READ)),
            (1, 0),
            "DTLS fails"
        );
    }

    // tests/modules/halfclose: a read event cached across a queued close.
    #[test]
    fn prequeue_reads_stop_at_a_queued_or_notified_close() {
        let read = Script::with(&[(OP_RD_PORT_READ, ACTIVE | ON_READ)]);
        let (code, h) = read_with(
            FOUND | CLOSE_QUEUED,
            Script::with(&[(OP_RD_PORT_READ, ACTIVE | ON_READ)]),
        );
        assert_eq!((code, h.called(OP_RD_PORT_READ)), (42, 1), "upstream reads");
        let (code, h) = read_with(FOUND | PREQUEUE | CLOSE_QUEUED, read);
        assert_eq!(
            (code, h.log.len()),
            (SW_OK, 0),
            "a queued plain close is not read"
        );
        let (code, h) = read_with(
            FOUND | PREQUEUE | CLOSE_QUEUED | SSL_WANT_READ,
            Script::with(&[(OP_RD_WRITE_FACTS, FOUND)]),
        );
        assert_eq!((code, h.called(OP_RD_PORT_READ)), (SW_OK, 0));
        assert_eq!(
            h.called(OP_WR_FRONT),
            1,
            "a TLS shutdown's read retries the write"
        );
        let (code, h) = read_with(
            FOUND | PREQUEUE | CLOSE_NOTIFIED | CLOSE_QUEUED | SSL_WANT_READ,
            Script::default(),
        );
        assert_eq!((code, h.log.len()), (SW_OK, 0), "notified is terminal");
        let (code, h) = read_with(
            FOUND | PREQUEUE | PROCESS,
            Script::with(&[(
                OP_RD_PORT_READ,
                ACTIVE | READ_NOTIFIED | QUEUE_OVER | ON_READ,
            )]),
        );
        assert_eq!(
            (code, h.called(OP_SET_WAITING_TIME)),
            (42, 0),
            "no pause after EOF"
        );
    }

    #[test]
    fn write_flushes_then_reads_again() {
        let mut h = Script::with(&[(OP_WR_SEND, 0), (OP_WR_STATE, OUT_EMPTY | SEND_TIMER)])
            .queue(OP_WR_FRONT, &[1, 1, 0]);
        assert_eq!(write(&mut h, FOUND).unwrap(), SW_OK);
        assert_eq!((h.called(OP_WR_SEND), h.called(OP_WR_QUEUED)), (2, 2));
        assert_eq!(h.called(OP_WR_TIMER_DEL), 1);
        assert_eq!(h.called(OP_WR_READ_ONLY), 1);
        let mut h = Script::with(&[(OP_WR_STATE, OUT_EMPTY)]);
        write(&mut h, FOUND | PREQUEUE).unwrap();
        assert_eq!(
            (h.called(OP_RT_REMOVE_WRITE), h.called(OP_WR_READ_ONLY)),
            (1, 0),
            "drains only"
        );
        let mut h = Script::with(&[(OP_WR_STATE, OUT_EMPTY | PEER_CLOSED)]);
        write(&mut h, FOUND).unwrap();
        assert_eq!(h.called(OP_WR_READ_ONLY), 0);
        let mut h = Script::with(&[(OP_WR_STATE, SEND_TIMER | OVERFLOW_DRAINED | LOW_WATERMARK)]);
        write(&mut h, FOUND).unwrap();
        assert_eq!(h.called(OP_WR_TIMER_DELAY), 1);
        assert_eq!(
            (h.called(OP_WR_CLEAR_OVERFLOW), h.called(OP_WR_BUFFER_EMPTY)),
            (1, 1)
        );
        assert_eq!(h.called(OP_WR_READ_ONLY), 0);
    }

    #[test]
    fn write_stops_at_a_close_or_a_full_socket() {
        let mut h = Script::with(&[(OP_WR_FRONT, 1 + KIND_CLOSE), (OP_WR_CLOSE, 7)]);
        assert_eq!(write(&mut h, FOUND).unwrap(), 7);
        let mut h = Script::with(&[
            (OP_WR_FRONT, 1 + KIND_SENDFILE),
            (OP_WR_SENDFILE, u64::MAX),
            (OP_WR_WAIT, CLOSE_WAIT),
            (OP_WR_CLOSE_ERRNO, 3),
        ]);
        assert_eq!(write(&mut h, FOUND).unwrap(), 3);
        let mut h = Script::with(&[
            (OP_WR_FRONT, 1),
            (OP_WR_SEND, u64::MAX),
            (OP_WR_WAIT, SEND_WAIT),
        ]);
        assert_eq!(write(&mut h, FOUND).unwrap(), SW_OK);
        assert_eq!((h.called(OP_WR_QUEUED), h.called(OP_WR_STATE)), (0, 1));
        let mut h = Script::default();
        assert_eq!(write(&mut h, 0).unwrap(), SW_ERR);
    }

    #[test]
    fn write_notifies_or_closes_a_pending_close() {
        let mut h = Script::default();
        assert_eq!(write(&mut h, FOUND | CLOSE_NOTIFY).unwrap(), SW_OK);
        assert_eq!(
            (h.called(OP_WR_NOTIFY_CLOSE), h.called(OP_WR_FRONT)),
            (1, 0)
        );
        let mut h = Script::default();
        write(&mut h, FOUND | CLOSE_NOTIFY | SSL_PENDING).unwrap();
        assert_eq!(h.called(OP_CLOSE), 1, "closed during the handshake");
        let mut h = Script::default();
        write(&mut h, FOUND | DISABLE_NOTIFY | CLOSE_FORCE).unwrap();
        assert_eq!((h.called(OP_CLOSE), h.called(OP_WR_FRONT)), (1, 0));
    }

    #[test]
    fn peer_close_notifies_once() {
        let mut h = Script::with(&[(OP_PC_NOTIFY, 1)]);
        assert_eq!(peer_close(&mut h, FOUND).unwrap(), 1);
        assert_eq!(h.args(OP_PC_NOTIFY), vec![1], "peer closed");
        let mut h = Script::default();
        assert_eq!(peer_close(&mut h, FOUND | CLOSE_NOTIFIED).unwrap(), SW_OK);
        assert_eq!(h.called(OP_PC_NOTIFY), 0);
        let mut h = Script::with(&[(OP_REACTOR_DEL, u64::MAX)]);
        assert_eq!(peer_close(&mut h, FOUND).unwrap(), SW_ERR);
        let mut h = Script::default();
        peer_close(&mut h, FOUND | CLOSE_QUEUED).unwrap();
        assert_eq!((h.called(OP_CLOSE), h.called(OP_REACTOR_DEL)), (1, 1));
        let mut h = Script::default();
        peer_close(&mut h, FOUND | DISABLE_NOTIFY).unwrap();
        assert_eq!((h.called(OP_CLOSE), h.called(OP_REACTOR_DEL)), (1, 0));
        assert_eq!(peer_close(&mut Script::default(), 0).unwrap(), SW_ERR);
    }

    #[test]
    fn prequeue_keeps_a_synchronous_process_response() {
        let owed = PREQUEUE | PROCESS | HASH_DISPATCH;
        assert!(!peer_closed(owed));
        assert!(peer_closed(owed & !PREQUEUE), "upstream");
        assert!(peer_closed(owed & !PROCESS));
        assert!(peer_closed(owed & !HASH_DISPATCH));
        for terminal in [
            DISPATCH_FUNC,
            COROUTINE,
            CONN_SSL,
            CLOSE_ERRNO,
            CLOSE_FORCE,
            CLOSE_RESET,
        ] {
            assert!(peer_closed(owed | terminal));
        }
    }

    #[test]
    fn close_releases_and_moves_the_max_fd() {
        let fd = 9u64 << 32;
        let mut h = Script::with(&[
            (OP_MAXFD, 9),
            (OP_MINFD, 3),
            (OP_CC_HTTP_OBJECT, 1),
            (OP_CC_FINISH, 0),
        ])
        .queue(OP_CC_VALID, &[0, 0, 1]);
        assert_eq!(
            close_connection(
                &mut h,
                fd | TIMER | WEBSOCKET_BUFFER | SOCKET_SSL | RECV_BUFFER | BASE
            )
            .unwrap(),
            SW_OK
        );
        for op in [
            OP_CC_TIMER_DEL,
            OP_CC_WEBSOCKET_FREE,
            OP_REACTOR_DEL,
            OP_CC_SSL_CLOSE,
            OP_CC_RECV_FREE,
            OP_CC_HTTP_DESTROY,
            OP_CC_SESSION_CLEAR,
        ] {
            assert_eq!(h.called(op), 1, "{op}");
        }
        assert_eq!(h.args(OP_CC_COUNT), vec![1], "base mode");
        assert_eq!(h.args(OP_CC_VALID), vec![8, 7, 6]);
        assert_eq!(h.args(OP_SET_MAXFD), vec![6]);
        assert_eq!(
            (h.called(OP_CC_REDIS_FREE), h.called(OP_CC_SSL_SHUTDOWN)),
            (0, 0)
        );

        let mut h = Script::with(&[(OP_MAXFD, 12), (OP_CC_LINGER, 1)]);
        close_connection(&mut h, fd | REMOVED | LINGER | CLOSE_RESET).unwrap();
        assert_eq!((h.called(OP_REACTOR_DEL), h.called(LOG_CC_LINGER)), (0, 1));
        assert_eq!(h.called(OP_SET_MAXFD), 0, "not the max fd");
        assert_eq!(h.args(OP_CC_COUNT), vec![0]);

        let mut h = Script::with(&[(OP_REACTOR_DEL, u64::MAX)]);
        assert_eq!(close_connection(&mut h, fd).unwrap(), SW_ERR);
        assert_eq!(h.called(OP_CC_FINISH), 0);
    }

    #[test]
    fn prequeue_close_queues_a_tls_close_notify() {
        let tls = PREQUEUE | TLS_READY_STREAM;
        let mut h = Script::with(&[(OP_CC_SSL_WANT, 1)]);
        assert_eq!(close_connection(&mut h, tls).unwrap(), SW_OK);
        assert_eq!(
            (h.called(OP_CC_QUEUE_CLOSE), h.args(OP_CC_WATCH_WRITE)),
            (1, vec![0])
        );
        assert_eq!(h.called(OP_CC_FINISH), 0, "drains first");
        let mut h = Script::with(&[(OP_CC_SSL_WANT, 1)]);
        close_connection(&mut h, tls | CLOSE_QUEUED | REMOVED).unwrap();
        assert_eq!(
            (h.called(OP_CC_QUEUE_CLOSE), h.args(OP_CC_WATCH_WRITE)),
            (0, vec![1])
        );
        let mut h = Script::with(&[(OP_CC_SSL_WANT, 1), (OP_CC_WATCH_WRITE, u64::MAX)]);
        close_connection(&mut h, tls).unwrap();
        assert_eq!(h.called(OP_CC_FINISH), 1, "could not watch: close now");
        let mut h = Script::with(&[(OP_CC_SSL_SHUTDOWN, 1)]);
        close_connection(&mut h, tls).unwrap();
        assert_eq!(
            (h.called(OP_CC_SSL_WANT), h.called(OP_CC_FINISH)),
            (0, 1),
            "sent"
        );
        for terminal in [PEER_CLOSED, CLOSE_FORCE, CLOSE_RESET, SSL_CLOSED, PREQUEUE] {
            let mut h = Script::with(&[(OP_CC_SSL_WANT, 1)]);
            close_connection(&mut h, tls ^ terminal).unwrap();
            assert_eq!(h.called(OP_CC_SSL_SHUTDOWN), 0);
        }
    }
}
