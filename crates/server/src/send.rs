//! `Server::send_to_connection`: the master's (or base-mode worker's) delivery of a
//! send, a close or a receive pause/resume to a connection it owns. Native code holds
//! the connection, socket and output buffer; each fact and effect is one op.
//!
//! `--fix-halfclose` (M15) builds define `SW_HALFCLOSE_DRAIN`, which native code
//! reports as `OP_SC_HALFCLOSE`: pause and resume then go through the reactor's
//! read-event helpers, a resume skips closing connections, and a queued close stops
//! reading.
use crate::ops::{
    LOG_SC_CLOSED_BY_CLIENT, LOG_SC_CONN_OVERFLOW, LOG_SC_NO_SESSION_DATA, LOG_SC_NO_SESSION_EVENT,
    LOG_SC_SOCKET_OVERFLOW, OP_ERRNO, OP_MODE, OP_PORT_IDLE_TIME, OP_SC_ADD, OP_SC_ADD_READ,
    OP_SC_ADD_WRITE, OP_SC_ADVANCE, OP_SC_APPEND, OP_SC_CLOSE, OP_SC_DEL, OP_SC_DIRECT_SEND,
    OP_SC_ENSURE_BUFFER, OP_SC_FLAGS, OP_SC_HALFCLOSE, OP_SC_HAS_SEND_TIMER, OP_SC_HIGH_WATERMARK,
    OP_SC_IDLE_TIMER, OP_SC_NOTIFY_FULL, OP_SC_OUT_EMPTY, OP_SC_OUT_FULL, OP_SC_OUT_HIGH,
    OP_SC_OVERFLOW, OP_SC_QUEUE_CLOSE, OP_SC_REMOVE_READ, OP_SC_SEND, OP_SC_SEND_LEFT,
    OP_SC_SEND_YIELD, OP_SC_SENDFILE, OP_SC_SET, OP_SC_SETUP, OP_SC_TOUCH_SEND, OP_SC_VERIFY,
    OP_SC_WRITABLE, OP_SC_YIELD_ERROR,
};
use crate::{Host, MODE_PROCESS, SW_ERR, SW_OK};

/// `ServerEventType`.
const SEND_DATA: u64 = 2;
const SEND_FILE: u64 = 3;
const CLOSE: u64 = 4;
const PAUSE_RECV: u64 = 11;
const RESUME_RECV: u64 = 12;

/// `SW_EVENT_READ`, `SW_EVENT_WRITE` as `OP_SC_SET`/`OP_SC_ADD` take them.
pub(crate) const READ: u64 = 1;
pub(crate) const WRITE: u64 = 2;

/// `OP_SC_FLAGS` bits: the connection's flags, then its socket's.
pub(crate) const OVERFLOW: u64 = 1 << 0;
pub(crate) const CLOSE_RESET: u64 = 1 << 1;
pub(crate) const CLOSE_FORCE: u64 = 1 << 2;
pub(crate) const PEER_CLOSED: u64 = 1 << 3;
pub(crate) const CLOSED: u64 = 1 << 4;
pub(crate) const CLOSE_QUEUED: u64 = 1 << 5;
pub(crate) const HIGH_WATERMARK: u64 = 1 << 6;
pub(crate) const REMOVED: u64 = 1 << 7;
pub(crate) const CLOSE_NOTIFIED: u64 = 1 << 8;
pub(crate) const EVENT_READ: u64 = 1 << 9;
pub(crate) const EVENT_WRITE: u64 = 1 << 10;
/// `onBufferFull`, `onBufferEmpty` are set.
pub(crate) const ON_BUFFER_FULL: u64 = 1 << 11;
pub(crate) const ON_BUFFER_EMPTY: u64 = 1 << 12;

/// The context's send of type `kind` and `length` bytes; `SW_OK` or `SW_ERR`, and 0
/// (upstream's `false`) where a send fails after the connection was found.
pub(crate) fn send_to_connection<H: Host>(h: &mut H, kind: u64) -> i64 {
    if !h.is(OP_SC_VERIFY) {
        h.get(if kind == SEND_DATA {
            LOG_SC_NO_SESSION_DATA
        } else {
            LOG_SC_NO_SESSION_EVENT
        });
        return SW_ERR;
    }
    h.get(OP_SC_SETUP);
    let flags = h.get(OP_SC_FLAGS);
    let send_yield = h.is(OP_SC_SEND_YIELD);
    if h.get(OP_MODE) != MODE_PROCESS && flags & OVERFLOW != 0 {
        if send_yield {
            h.get(OP_SC_YIELD_ERROR);
        } else {
            h.get(LOG_SC_SOCKET_OVERFLOW);
        }
        return SW_ERR;
    }
    if kind == CLOSE && flags & (CLOSE_RESET | CLOSE_FORCE | PEER_CLOSED) != 0 {
        h.get(OP_SC_CLOSE);
        return SW_OK;
    }
    let halfclose = h.is(OP_SC_HALFCLOSE);
    if kind == PAUSE_RECV {
        if flags & REMOVED != 0 || flags & EVENT_READ == 0 {
            return SW_OK;
        }
        return if halfclose {
            h.signed(OP_SC_REMOVE_READ)
        } else if flags & EVENT_WRITE != 0 {
            h.op(OP_SC_SET, WRITE).cast_signed()
        } else {
            h.signed(OP_SC_DEL)
        };
    }
    if kind == RESUME_RECV {
        let skip = if halfclose {
            flags & (CLOSED | PEER_CLOSED | CLOSE_QUEUED | CLOSE_NOTIFIED | EVENT_READ) != 0
        } else {
            flags & REMOVED == 0 || flags & EVENT_READ != 0
        };
        if skip {
            return SW_OK;
        }
        return if halfclose {
            h.signed(OP_SC_ADD_READ)
        } else if flags & EVENT_WRITE != 0 {
            h.op(OP_SC_SET, READ | WRITE).cast_signed()
        } else {
            h.op(OP_SC_ADD, READ).cast_signed()
        };
    }
    if h.is(OP_SC_OUT_EMPTY) {
        if kind == CLOSE {
            h.get(OP_SC_CLOSE);
            return SW_OK;
        }
        if kind != SEND_FILE && h.is(OP_SC_DIRECT_SEND) && direct_send(h) {
            return SW_OK;
        }
        h.get(OP_SC_ENSURE_BUFFER);
    }
    if kind == CLOSE {
        h.get(OP_SC_QUEUE_CLOSE);
        if halfclose {
            h.get(OP_SC_REMOVE_READ);
        }
    } else if kind == SEND_FILE {
        if h.signed(OP_SC_SENDFILE) < 0 {
            return 0;
        }
    } else {
        if flags & PEER_CLOSED != 0 {
            h.get(LOG_SC_CLOSED_BY_CLIENT);
            return 0;
        }
        if h.is(OP_SC_OUT_FULL) {
            if send_yield {
                h.get(OP_SC_YIELD_ERROR);
            } else {
                h.get(LOG_SC_CONN_OVERFLOW);
            }
            h.get(OP_SC_OVERFLOW);
            if flags & ON_BUFFER_EMPTY != 0 && flags & ON_BUFFER_FULL == 0 {
                h.get(OP_SC_HIGH_WATERMARK);
            }
        }
        h.get(OP_SC_APPEND);
        if flags & ON_BUFFER_FULL != 0
            && h.get(OP_SC_FLAGS) & HIGH_WATERMARK == 0
            && h.is(OP_SC_OUT_HIGH)
        {
            h.get(OP_SC_NOTIFY_FULL);
        }
    }
    if h.float(OP_PORT_IDLE_TIME) > 0.0 && !h.is(OP_SC_HAS_SEND_TIMER) {
        h.get(OP_SC_IDLE_TIMER);
    }
    if !h.is(OP_SC_WRITABLE) {
        h.get(OP_SC_ADD_WRITE);
    }
    SW_OK
}

/// Writes straight to the socket: true when all of it went, else the rest is buffered.
fn direct_send<H: Host>(h: &mut H) -> bool {
    loop {
        let length = h.get(OP_SC_SEND_LEFT);
        let sent = h.signed(OP_SC_SEND);
        if sent >= 0 && sent.cast_unsigned() == length {
            h.get(OP_SC_TOUCH_SEND);
            return true;
        }
        if sent > 0 {
            h.op(OP_SC_ADVANCE, sent.cast_unsigned());
            return false;
        }
        if h.signed(OP_ERRNO) != i64::from(libc::EINTR) {
            return false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MODE_BASE;
    use crate::script::Script;

    fn found(flags: u64, extra: &[(u32, u64)]) -> Script {
        let mut s = Script::with(&[
            (OP_SC_VERIFY, 1),
            (OP_SC_FLAGS, flags),
            (OP_MODE, MODE_PROCESS),
        ]);
        s.facts.extend(extra.iter().copied());
        s
    }

    #[test]
    fn missing_session_logs_by_kind() {
        let mut s = Script::default();
        assert_eq!(send_to_connection(&mut s, SEND_DATA), SW_ERR);
        assert_eq!(s.called(LOG_SC_NO_SESSION_DATA), 1);
        let mut s = Script::default();
        assert_eq!(send_to_connection(&mut s, CLOSE), SW_ERR);
        assert_eq!(s.called(LOG_SC_NO_SESSION_EVENT), 1);
    }

    #[test]
    fn overflow_outside_process_mode_refuses() {
        let mut s = found(OVERFLOW, &[(OP_MODE, MODE_BASE), (OP_SC_SEND_YIELD, 1)]);
        assert_eq!(send_to_connection(&mut s, SEND_DATA), SW_ERR);
        assert_eq!(s.called(OP_SC_YIELD_ERROR), 1);
        let mut s = found(OVERFLOW, &[(OP_MODE, MODE_BASE)]);
        assert_eq!(send_to_connection(&mut s, SEND_DATA), SW_ERR);
        assert_eq!(s.called(LOG_SC_SOCKET_OVERFLOW), 1);
    }

    #[test]
    fn forced_close_closes_now() {
        for flag in [CLOSE_RESET, CLOSE_FORCE, PEER_CLOSED] {
            let mut s = found(flag, &[]);
            assert_eq!(send_to_connection(&mut s, CLOSE), SW_OK);
            assert_eq!(s.called(OP_SC_CLOSE), 1);
            assert_eq!(s.called(OP_SC_OUT_EMPTY), 0);
        }
    }

    #[test]
    fn pause_and_resume() {
        let mut s = found(EVENT_READ | EVENT_WRITE, &[(OP_SC_SET, 0)]);
        assert_eq!(send_to_connection(&mut s, PAUSE_RECV), SW_OK);
        assert_eq!(s.args(OP_SC_SET), vec![WRITE]);
        let mut s = found(EVENT_READ, &[]);
        send_to_connection(&mut s, PAUSE_RECV);
        assert_eq!(s.called(OP_SC_DEL), 1);
        let mut s = found(REMOVED | EVENT_READ, &[]);
        send_to_connection(&mut s, PAUSE_RECV);
        assert_eq!(s.called(OP_SC_DEL), 0, "already removed");

        let mut s = found(REMOVED | EVENT_WRITE, &[]);
        send_to_connection(&mut s, RESUME_RECV);
        assert_eq!(s.args(OP_SC_SET), vec![READ | WRITE]);
        let mut s = found(REMOVED, &[]);
        send_to_connection(&mut s, RESUME_RECV);
        assert_eq!(s.args(OP_SC_ADD), vec![READ]);
        let mut s = found(0, &[]);
        send_to_connection(&mut s, RESUME_RECV);
        assert_eq!(s.called(OP_SC_ADD), 0, "still in the reactor");
    }

    #[test]
    fn halfclose_pause_and_resume() {
        let mut s = found(EVENT_READ | EVENT_WRITE, &[(OP_SC_HALFCLOSE, 1)]);
        send_to_connection(&mut s, PAUSE_RECV);
        assert_eq!(s.called(OP_SC_REMOVE_READ), 1);
        assert_eq!(s.called(OP_SC_SET), 0);
        let mut s = found(0, &[(OP_SC_HALFCLOSE, 1)]);
        send_to_connection(&mut s, RESUME_RECV);
        assert_eq!(s.called(OP_SC_ADD_READ), 1, "not removed, but not reading");
        for closing in [CLOSED, PEER_CLOSED, CLOSE_QUEUED, CLOSE_NOTIFIED] {
            let mut s = found(REMOVED | closing, &[(OP_SC_HALFCLOSE, 1)]);
            send_to_connection(&mut s, RESUME_RECV);
            assert_eq!(s.called(OP_SC_ADD_READ), 0);
        }
    }

    #[test]
    fn close_with_empty_output_closes_else_queues() {
        let mut s = found(0, &[(OP_SC_OUT_EMPTY, 1)]);
        assert_eq!(send_to_connection(&mut s, CLOSE), SW_OK);
        assert_eq!(s.called(OP_SC_CLOSE), 1);
        let mut s = found(0, &[(OP_SC_HALFCLOSE, 1)]);
        assert_eq!(send_to_connection(&mut s, CLOSE), SW_OK);
        assert_eq!(s.called(OP_SC_QUEUE_CLOSE), 1);
        assert_eq!(s.called(OP_SC_REMOVE_READ), 1, "halfclose stops reading");
        assert_eq!(s.called(OP_SC_ADD_WRITE), 1);
    }

    #[test]
    fn direct_send_retries_interrupts_and_buffers_the_rest() {
        let mut s = found(
            0,
            &[
                (OP_SC_OUT_EMPTY, 1),
                (OP_SC_DIRECT_SEND, 1),
                (OP_SC_SEND_LEFT, 10),
            ],
        )
        .queue(OP_SC_SEND, &[u64::MAX, 10]);
        s.facts.insert(OP_ERRNO, libc::EINTR.cast_unsigned().into());
        assert_eq!(send_to_connection(&mut s, SEND_DATA), SW_OK);
        assert_eq!(s.called(OP_SC_SEND), 2);
        assert_eq!(s.called(OP_SC_TOUCH_SEND), 1);
        assert_eq!(s.called(OP_SC_APPEND), 0);

        let mut s = found(
            0,
            &[
                (OP_SC_OUT_EMPTY, 1),
                (OP_SC_DIRECT_SEND, 1),
                (OP_SC_SEND_LEFT, 10),
            ],
        )
        .queue(OP_SC_SEND, &[4]);
        send_to_connection(&mut s, SEND_DATA);
        assert_eq!(s.args(OP_SC_ADVANCE), vec![4]);
        assert_eq!(s.called(OP_SC_ENSURE_BUFFER), 1);
        assert_eq!(s.called(OP_SC_APPEND), 1);

        let mut s = found(0, &[(OP_SC_OUT_EMPTY, 1)]);
        send_to_connection(&mut s, SEND_FILE);
        assert_eq!(s.called(OP_SC_SEND), 0, "files are never sent directly");
        assert_eq!(s.called(OP_SC_SENDFILE), 1);
    }

    #[test]
    fn buffered_send_watermarks() {
        let mut s = found(PEER_CLOSED, &[]);
        assert_eq!(send_to_connection(&mut s, SEND_DATA), 0);
        assert_eq!(s.called(LOG_SC_CLOSED_BY_CLIENT), 1);

        let mut s = found(ON_BUFFER_EMPTY, &[(OP_SC_OUT_FULL, 1)]);
        assert_eq!(send_to_connection(&mut s, SEND_DATA), SW_OK);
        assert_eq!(s.called(LOG_SC_CONN_OVERFLOW), 1);
        assert_eq!(s.called(OP_SC_OVERFLOW), 1);
        assert_eq!(s.called(OP_SC_HIGH_WATERMARK), 1);

        let mut s = found(ON_BUFFER_FULL, &[(OP_SC_OUT_HIGH, 1)]);
        send_to_connection(&mut s, SEND_DATA);
        assert_eq!(s.called(OP_SC_NOTIFY_FULL), 1);
        let mut s = found(ON_BUFFER_FULL, &[(OP_SC_OUT_HIGH, 1)])
            .queue(OP_SC_FLAGS, &[ON_BUFFER_FULL, HIGH_WATERMARK]);
        send_to_connection(&mut s, SEND_DATA);
        assert_eq!(
            s.called(OP_SC_NOTIFY_FULL),
            0,
            "already above the watermark"
        );

        let mut s = found(0, &[(OP_SC_SENDFILE, u64::MAX)]);
        assert_eq!(send_to_connection(&mut s, SEND_FILE), 0);
        assert_eq!(s.called(OP_SC_ADD_WRITE), 0);
    }

    #[test]
    fn flags_match_the_header() {
        let header = include_str!("../../../native/include/swoole_rs.h");
        for (name, value) in [
            ("OVERFLOW", OVERFLOW),
            ("CLOSE_RESET", CLOSE_RESET),
            ("CLOSE_FORCE", CLOSE_FORCE),
            ("PEER_CLOSED", PEER_CLOSED),
            ("CLOSED", CLOSED),
            ("CLOSE_QUEUED", CLOSE_QUEUED),
            ("HIGH_WATERMARK", HIGH_WATERMARK),
            ("REMOVED", REMOVED),
            ("CLOSE_NOTIFIED", CLOSE_NOTIFIED),
            ("EVENT_READ", EVENT_READ),
            ("EVENT_WRITE", EVENT_WRITE),
            ("ON_BUFFER_FULL", ON_BUFFER_FULL),
            ("ON_BUFFER_EMPTY", ON_BUFFER_EMPTY),
        ] {
            let line = format!("#define SWR_SC_{name} (1u << {})", value.trailing_zeros());
            assert!(header.contains(&line), "{line}");
        }
        assert!(header.contains(&format!("#define SWR_SC_READ {READ}u")));
        assert!(header.contains(&format!("#define SWR_SC_WRITE {WRITE}u")));
    }
}
