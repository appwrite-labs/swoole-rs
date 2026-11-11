//! `Server::send_to_connection`: the master's (or base-mode worker's) delivery of a
//! send, a close or a receive pause/resume to a connection it owns. Native code holds
//! the connection, socket and output buffer; each effect is one op, and `OP_SC_VERIFY`
//! returns every fact a send reads before its first effect.
//!
//! `--fix-halfclose` (M15) builds have the `halfclose` feature, which reports
//! `HALFCLOSE` (upstream's `SW_HALFCLOSE_DRAIN`): pause and resume then go through the reactor's
//! read-event helpers, a resume skips closing connections, and a queued close stops
//! reading.
use crate::base_factory::SendData;
use crate::ops::{
    LOG_SC_CLOSED_BY_CLIENT, LOG_SC_CONN_OVERFLOW, LOG_SC_NO_SESSION_DATA, LOG_SC_NO_SESSION_EVENT,
    LOG_SC_SOCKET_OVERFLOW, OP_ERRNO, OP_MODE, OP_PORT_IDLE_TIME, OP_SC_ADD, OP_SC_ADD_READ,
    OP_SC_ADD_WRITE, OP_SC_ADVANCE, OP_SC_APPEND, OP_SC_CLOSE, OP_SC_DEL, OP_SC_ENSURE_BUFFER,
    OP_SC_FLAGS, OP_SC_HAS_SEND_TIMER, OP_SC_HIGH_WATERMARK, OP_SC_IDLE_TIMER, OP_SC_NOTIFY_FULL,
    OP_SC_OUT_FULL, OP_SC_OUT_HIGH, OP_SC_OVERFLOW, OP_SC_QUEUE_CLOSE, OP_SC_REMOVE_READ,
    OP_SC_SEND, OP_SC_SENDFILE, OP_SC_SET, OP_SC_TOUCH_SEND, OP_SC_VERIFY, OP_SC_WRITABLE,
    OP_SC_YIELD_ERROR,
};
use crate::records::VERIFIED_TLS;
use crate::{Host, MODE_PROCESS, SW_ERR, SW_OK, ServerState};
use std::ffi::c_void;
use swoole_rs_corebase::microtime;
use swoole_rs_reactor::{Socket, socket_record_send, steady_ms};

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
/// `OP_SC_VERIFY`'s bits beside those: the connection was found; `send_yield`; built
/// with the `halfclose` feature; the output buffer is empty; the socket sends directly.
pub(crate) const FOUND: u64 = 1 << 13;
pub(crate) const SEND_YIELD: u64 = 1 << 14;
pub(crate) const HALFCLOSE: u64 = 1 << 15;
pub(crate) const OUT_EMPTY: u64 = 1 << 16;
pub(crate) const DIRECT_SEND: u64 = 1 << 17;

/// `send_to_connection`'s direct send of a base-mode `SendData`, made here when it
/// needs nothing native: the verified connection has no overflow and its socket sends
/// directly with no output buffer yet. `None` leaves the whole send to native code;
/// otherwise `Socket::send`'s result after EINTR retries, and a complete send has set
/// the connection's `last_send_time`.
///
/// # Safety
/// `send` lends `info.len` readable bytes at `data`.
pub(crate) unsafe fn send_now(state: Option<&ServerState>, send: &SendData) -> Option<i64> {
    let state = state?;
    let conn = state.connection(send.info.fd, VERIFIED_TLS)?;
    // SAFETY: a live connection's socket record.
    let socket = unsafe { Socket::from_ptr(conn.socket.cast()) }?;
    if conn.overflow != 0
        || socket.read(|record| {
            record.direct_send == 0
                || !record.ssl().is_null()
                || !record.output.out_buffer.is_null()
        })
    {
        return None;
    }
    let length = usize::try_from(send.info.len).ok()?;
    let sent = loop {
        // SAFETY: `data` holds `length` bytes.
        let sent =
            unsafe { socket_record_send(socket, send.data.cast(), length, 0, no_tls, steady_ms) };
        if sent >= 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            break sent;
        }
    };
    if sent.cast_unsigned() == length {
        conn.last_send_time = microtime();
    }
    Some(i64::try_from(sent).unwrap_or(-1))
}

/// `send_now`'s TLS call, never made: it sends only on sockets without an SSL.
unsafe extern "C" fn no_tls(_: *mut c_void, _: *mut c_void, _: usize) -> isize {
    -1
}

/// The context's send of type `kind` and `length` bytes; `SW_OK` or `SW_ERR`, and 0
/// (upstream's `false`) where a send fails after the connection was found. `first` is
/// what `send_now` already sent, short of all of it.
pub(crate) fn send_to_connection<H: Host>(
    h: &mut H,
    kind: u64,
    first: Option<i64>,
) -> crate::Effect<i64> {
    let flags = h.get(OP_SC_VERIFY)?;
    if flags & FOUND == 0 {
        h.get(if kind == SEND_DATA {
            LOG_SC_NO_SESSION_DATA
        } else {
            LOG_SC_NO_SESSION_EVENT
        })?;
        return Ok(SW_ERR);
    }
    let send_yield = flags & SEND_YIELD != 0;
    if flags & OVERFLOW != 0 && h.get(OP_MODE)? != MODE_PROCESS {
        if send_yield {
            h.get(OP_SC_YIELD_ERROR)?;
        } else {
            h.get(LOG_SC_SOCKET_OVERFLOW)?;
        }
        return Ok(SW_ERR);
    }
    if kind == CLOSE && flags & (CLOSE_RESET | CLOSE_FORCE | PEER_CLOSED) != 0 {
        h.get(OP_SC_CLOSE)?;
        return Ok(SW_OK);
    }
    let halfclose = flags & HALFCLOSE != 0;
    if kind == PAUSE_RECV {
        if flags & REMOVED != 0 || flags & EVENT_READ == 0 {
            return Ok(SW_OK);
        }
        return Ok(if halfclose {
            h.signed(OP_SC_REMOVE_READ)?
        } else if flags & EVENT_WRITE != 0 {
            h.op(OP_SC_SET, WRITE)?.cast_signed()
        } else {
            h.signed(OP_SC_DEL)?
        });
    }
    if kind == RESUME_RECV {
        let skip = if halfclose {
            flags & (CLOSED | PEER_CLOSED | CLOSE_QUEUED | CLOSE_NOTIFIED | EVENT_READ) != 0
        } else {
            flags & REMOVED == 0 || flags & EVENT_READ != 0
        };
        if skip {
            return Ok(SW_OK);
        }
        return Ok(if halfclose {
            h.signed(OP_SC_ADD_READ)?
        } else if flags & EVENT_WRITE != 0 {
            h.op(OP_SC_SET, READ | WRITE)?.cast_signed()
        } else {
            h.op(OP_SC_ADD, READ)?.cast_signed()
        });
    }
    if flags & OUT_EMPTY != 0 {
        if kind == CLOSE {
            h.get(OP_SC_CLOSE)?;
            return Ok(SW_OK);
        }
        if kind != SEND_FILE && flags & DIRECT_SEND != 0 && direct_send(h, flags >> 32, first)? {
            return Ok(SW_OK);
        }
        h.get(OP_SC_ENSURE_BUFFER)?;
    }
    if kind == CLOSE {
        h.get(OP_SC_QUEUE_CLOSE)?;
        if halfclose {
            h.get(OP_SC_REMOVE_READ)?;
        }
    } else if kind == SEND_FILE {
        if h.signed(OP_SC_SENDFILE)? < 0 {
            return Ok(0);
        }
    } else {
        if flags & PEER_CLOSED != 0 {
            h.get(LOG_SC_CLOSED_BY_CLIENT)?;
            return Ok(0);
        }
        if h.is(OP_SC_OUT_FULL)? {
            if send_yield {
                h.get(OP_SC_YIELD_ERROR)?;
            } else {
                h.get(LOG_SC_CONN_OVERFLOW)?;
            }
            h.get(OP_SC_OVERFLOW)?;
            if flags & ON_BUFFER_EMPTY != 0 && flags & ON_BUFFER_FULL == 0 {
                h.get(OP_SC_HIGH_WATERMARK)?;
            }
        }
        h.get(OP_SC_APPEND)?;
        if flags & ON_BUFFER_FULL != 0
            && h.get(OP_SC_FLAGS)? & HIGH_WATERMARK == 0
            && h.is(OP_SC_OUT_HIGH)?
        {
            h.get(OP_SC_NOTIFY_FULL)?;
        }
    }
    if h.float(OP_PORT_IDLE_TIME)? > 0.0 && !h.is(OP_SC_HAS_SEND_TIMER)? {
        h.get(OP_SC_IDLE_TIMER)?;
    }
    if !h.is(OP_SC_WRITABLE)? {
        h.get(OP_SC_ADD_WRITE)?;
    }
    Ok(SW_OK)
}

/// Writes the `length` bytes straight to the socket: true when all of them went, else
/// the rest is buffered. After `send_now`'s attempt only its progress is recorded.
fn direct_send<H: Host>(h: &mut H, length: u64, first: Option<i64>) -> crate::Effect<bool> {
    if let Some(sent) = first {
        if sent > 0 {
            h.op(OP_SC_ADVANCE, sent.cast_unsigned())?;
        }
        return Ok(false);
    }
    loop {
        let sent = h.signed(OP_SC_SEND)?;
        if sent >= 0 && sent.cast_unsigned() == length {
            h.get(OP_SC_TOUCH_SEND)?;
            return Ok(true);
        }
        if sent > 0 {
            h.op(OP_SC_ADVANCE, sent.cast_unsigned())?;
            return Ok(false);
        }
        if h.signed(OP_ERRNO)? != i64::from(libc::EINTR) {
            return Ok(false);
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
            (OP_SC_VERIFY, FOUND | flags),
            (OP_SC_FLAGS, flags & !(u64::MAX << 13)),
            (OP_MODE, MODE_PROCESS),
        ]);
        s.facts.extend(extra.iter().copied());
        s
    }

    #[test]
    fn missing_session_logs_by_kind() {
        let mut s = Script::default();
        assert_eq!(send_to_connection(&mut s, SEND_DATA, None).unwrap(), SW_ERR);
        assert_eq!(s.called(LOG_SC_NO_SESSION_DATA), 1);
        let mut s = Script::default();
        assert_eq!(send_to_connection(&mut s, CLOSE, None).unwrap(), SW_ERR);
        assert_eq!(s.called(LOG_SC_NO_SESSION_EVENT), 1);
    }

    #[test]
    fn overflow_outside_process_mode_refuses() {
        let mut s = found(OVERFLOW | SEND_YIELD, &[(OP_MODE, MODE_BASE)]);
        assert_eq!(send_to_connection(&mut s, SEND_DATA, None).unwrap(), SW_ERR);
        assert_eq!(s.called(OP_SC_YIELD_ERROR), 1);
        let mut s = found(OVERFLOW, &[(OP_MODE, MODE_BASE)]);
        assert_eq!(send_to_connection(&mut s, SEND_DATA, None).unwrap(), SW_ERR);
        assert_eq!(s.called(LOG_SC_SOCKET_OVERFLOW), 1);
    }

    #[test]
    fn forced_close_closes_now() {
        for flag in [CLOSE_RESET, CLOSE_FORCE, PEER_CLOSED] {
            let mut s = found(flag, &[]);
            assert_eq!(send_to_connection(&mut s, CLOSE, None).unwrap(), SW_OK);
            assert_eq!(s.called(OP_SC_CLOSE), 1);
        }
    }

    #[test]
    fn pause_and_resume() {
        let mut s = found(EVENT_READ | EVENT_WRITE, &[(OP_SC_SET, 0)]);
        assert_eq!(send_to_connection(&mut s, PAUSE_RECV, None).unwrap(), SW_OK);
        assert_eq!(s.args(OP_SC_SET), vec![WRITE]);
        let mut s = found(EVENT_READ, &[]);
        send_to_connection(&mut s, PAUSE_RECV, None).unwrap();
        assert_eq!(s.called(OP_SC_DEL), 1);
        let mut s = found(REMOVED | EVENT_READ, &[]);
        send_to_connection(&mut s, PAUSE_RECV, None).unwrap();
        assert_eq!(s.called(OP_SC_DEL), 0, "already removed");

        let mut s = found(REMOVED | EVENT_WRITE, &[]);
        send_to_connection(&mut s, RESUME_RECV, None).unwrap();
        assert_eq!(s.args(OP_SC_SET), vec![READ | WRITE]);
        let mut s = found(REMOVED, &[]);
        send_to_connection(&mut s, RESUME_RECV, None).unwrap();
        assert_eq!(s.args(OP_SC_ADD), vec![READ]);
        let mut s = found(0, &[]);
        send_to_connection(&mut s, RESUME_RECV, None).unwrap();
        assert_eq!(s.called(OP_SC_ADD), 0, "still in the reactor");
    }

    #[test]
    fn halfclose_pause_and_resume() {
        let mut s = found(EVENT_READ | EVENT_WRITE | HALFCLOSE, &[]);
        send_to_connection(&mut s, PAUSE_RECV, None).unwrap();
        assert_eq!(s.called(OP_SC_REMOVE_READ), 1);
        assert_eq!(s.called(OP_SC_SET), 0);
        let mut s = found(HALFCLOSE, &[]);
        send_to_connection(&mut s, RESUME_RECV, None).unwrap();
        assert_eq!(s.called(OP_SC_ADD_READ), 1, "not removed, but not reading");
        for closing in [CLOSED, PEER_CLOSED, CLOSE_QUEUED, CLOSE_NOTIFIED] {
            let mut s = found(REMOVED | closing | HALFCLOSE, &[]);
            send_to_connection(&mut s, RESUME_RECV, None).unwrap();
            assert_eq!(s.called(OP_SC_ADD_READ), 0);
        }
    }

    #[test]
    fn close_with_empty_output_closes_else_queues() {
        let mut s = found(OUT_EMPTY, &[]);
        assert_eq!(send_to_connection(&mut s, CLOSE, None).unwrap(), SW_OK);
        assert_eq!(s.called(OP_SC_CLOSE), 1);
        let mut s = found(HALFCLOSE, &[]);
        assert_eq!(send_to_connection(&mut s, CLOSE, None).unwrap(), SW_OK);
        assert_eq!(s.called(OP_SC_QUEUE_CLOSE), 1);
        assert_eq!(s.called(OP_SC_REMOVE_READ), 1, "halfclose stops reading");
        assert_eq!(s.called(OP_SC_ADD_WRITE), 1);
    }

    #[test]
    fn direct_send_retries_interrupts_and_buffers_the_rest() {
        let mut s =
            found(OUT_EMPTY | DIRECT_SEND | 10 << 32, &[]).queue(OP_SC_SEND, &[u64::MAX, 10]);
        s.facts.insert(OP_ERRNO, libc::EINTR.cast_unsigned().into());
        assert_eq!(send_to_connection(&mut s, SEND_DATA, None).unwrap(), SW_OK);
        assert_eq!(s.called(OP_SC_SEND), 2);
        assert_eq!(s.called(OP_SC_TOUCH_SEND), 1);
        assert_eq!(s.called(OP_SC_APPEND), 0);

        let mut s = found(OUT_EMPTY | DIRECT_SEND | 10 << 32, &[]).queue(OP_SC_SEND, &[4]);
        send_to_connection(&mut s, SEND_DATA, None).unwrap();
        assert_eq!(s.args(OP_SC_ADVANCE), vec![4]);
        assert_eq!(s.called(OP_SC_ENSURE_BUFFER), 1);
        assert_eq!(s.called(OP_SC_APPEND), 1);

        let mut s = found(OUT_EMPTY | DIRECT_SEND, &[]);
        send_to_connection(&mut s, SEND_FILE, None).unwrap();
        assert_eq!(s.called(OP_SC_SEND), 0, "files are never sent directly");
        assert_eq!(s.called(OP_SC_SENDFILE), 1);
    }

    #[test]
    fn a_send_made_first_is_not_repeated() {
        let mut s = found(OUT_EMPTY | DIRECT_SEND | 10 << 32, &[]);
        send_to_connection(&mut s, SEND_DATA, Some(4)).unwrap();
        assert_eq!(s.called(OP_SC_SEND), 0);
        assert_eq!(s.args(OP_SC_ADVANCE), vec![4]);
        assert_eq!(s.called(OP_SC_APPEND), 1);

        let mut s = found(OUT_EMPTY | DIRECT_SEND | 10 << 32, &[]);
        send_to_connection(&mut s, SEND_DATA, Some(-1)).unwrap();
        assert_eq!(s.called(OP_SC_SEND), 0, "would block: the rest is buffered");
        assert_eq!(s.called(OP_SC_ADVANCE), 0);
        assert_eq!(s.called(OP_SC_APPEND), 1);
    }

    #[test]
    fn buffered_send_watermarks() {
        let mut s = found(PEER_CLOSED, &[]);
        assert_eq!(send_to_connection(&mut s, SEND_DATA, None).unwrap(), 0);
        assert_eq!(s.called(LOG_SC_CLOSED_BY_CLIENT), 1);

        let mut s = found(ON_BUFFER_EMPTY, &[(OP_SC_OUT_FULL, 1)]);
        assert_eq!(send_to_connection(&mut s, SEND_DATA, None).unwrap(), SW_OK);
        assert_eq!(s.called(LOG_SC_CONN_OVERFLOW), 1);
        assert_eq!(s.called(OP_SC_OVERFLOW), 1);
        assert_eq!(s.called(OP_SC_HIGH_WATERMARK), 1);

        let mut s = found(ON_BUFFER_FULL, &[(OP_SC_OUT_HIGH, 1)]);
        send_to_connection(&mut s, SEND_DATA, None).unwrap();
        assert_eq!(s.called(OP_SC_NOTIFY_FULL), 1);
        let mut s = found(ON_BUFFER_FULL, &[(OP_SC_OUT_HIGH, 1)])
            .queue(OP_SC_FLAGS, &[ON_BUFFER_FULL | HIGH_WATERMARK]);
        send_to_connection(&mut s, SEND_DATA, None).unwrap();
        assert_eq!(
            s.called(OP_SC_NOTIFY_FULL),
            0,
            "already above the watermark"
        );

        let mut s = found(0, &[(OP_SC_SENDFILE, u64::MAX)]);
        assert_eq!(send_to_connection(&mut s, SEND_FILE, None).unwrap(), 0);
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
            ("FOUND", FOUND),
            ("SEND_YIELD", SEND_YIELD),
            ("HALFCLOSE", HALFCLOSE),
            ("OUT_EMPTY", OUT_EMPTY),
            ("DIRECT_SEND", DIRECT_SEND),
        ] {
            let line = format!("#define SWR_SC_{name} (1u << {})", value.trailing_zeros());
            assert!(header.contains(&line), "{line}");
        }
        assert!(header.contains(&format!("#define SWR_SC_READ {READ}u")));
        assert!(header.contains(&format!("#define SWR_SC_WRITE {WRITE}u")));
    }
}
