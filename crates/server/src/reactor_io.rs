//! Reactor threads' worker-pipe write path and a reactor thread's setup (`src/server/reactor_thread.cc`). The context holds the
//! server, the reactor, its thread and, per operation, the event, socket and
//! connection.
use crate::ops::{
    FIELD_HEARTBEAT_INTERVAL, LOG_RT_CLOSED_BY_SERVER, LOG_RT_DISCARD, LOG_RT_SET_WRITE, OP_FIELD,
    OP_HAVE_DGRAM, OP_MODE, OP_PORT_COUNT, OP_PORT_DGRAM, OP_PORT_FD, OP_REACTOR_NUM,
    OP_RT_ADD_WORKER_PIPE, OP_RT_BUF_EMPTY, OP_RT_BUS_SETUP, OP_RT_CATCH_WAIT, OP_RT_DGRAM_BIND,
    OP_RT_DISCARD_TIMEOUT, OP_RT_FRONT, OP_RT_FRONT_CONN, OP_RT_FRONT_CONN_CLOSED,
    OP_RT_FRONT_STREAM, OP_RT_HAS_PIPE_COMMAND, OP_RT_HEARTBEAT_TIMER, OP_RT_INIT_PIPE_SOCKETS,
    OP_RT_INIT_REACTOR, OP_RT_PIPE_COMMAND, OP_RT_POP, OP_RT_REMOVE_WRITE, OP_RT_SEND_FRONT,
    OP_RT_SETUP, OP_RT_THREAD_WORKER, OP_RT_TOUCH_SEND, OP_WORKER_NUM, RUN_RT_INIT,
    RUN_RT_PIPE_WRITE,
};
use crate::{Host, MODE_THREAD, SW_ERR, SW_OK};

pub(crate) fn run<H: Host>(h: &mut H, operation: u32, arg: u64) -> crate::Effect<Option<i64>> {
    Ok(Some(match operation {
        RUN_RT_PIPE_WRITE => pipe_write(h)?,
        RUN_RT_INIT => init(h, arg)?,
        _ => return Ok(None),
    }))
}

/// `ReactorThread_onPipeWrite`: flush the queued messages to a worker, dropping
/// those for connections the server already closed.
fn pipe_write<H: Host>(h: &mut H) -> crate::Effect<i64> {
    while !h.is(OP_RT_BUF_EMPTY)? {
        h.get(OP_RT_FRONT)?;
        if h.is(OP_RT_FRONT_STREAM)? {
            let discard = if h.is(OP_RT_FRONT_CONN)? {
                h.get(OP_RT_TOUCH_SEND)?;
                let closed = h.is(OP_RT_FRONT_CONN_CLOSED)?;
                if closed {
                    h.get(LOG_RT_CLOSED_BY_SERVER)?;
                }
                closed
            } else if h.is(OP_RT_DISCARD_TIMEOUT)? {
                h.get(LOG_RT_DISCARD)?;
                true
            } else {
                false
            };
            if discard {
                h.get(OP_RT_POP)?;
                continue;
            }
        }
        if h.signed(OP_RT_SEND_FRONT)? < 0 {
            return Ok(if h.is(OP_RT_CATCH_WAIT)? {
                SW_OK
            } else {
                SW_ERR
            });
        }
        h.get(OP_RT_POP)?;
    }
    if h.signed(OP_RT_REMOVE_WRITE)? < 0 {
        h.get(LOG_RT_SET_WRITE)?;
    }
    Ok(SW_OK)
}

/// `ReactorThread::init`: a reactor thread's handlers, its UDP ports, worker
/// pipes and message bus.
fn init<H: Host>(h: &mut H, id: u64) -> crate::Effect<i64> {
    h.op(OP_RT_SETUP, id)?;
    // Listen on this thread's UDP ports.
    if h.is(OP_HAVE_DGRAM)? {
        let reactors = h.get(OP_REACTOR_NUM)?;
        for port in 0..h.get(OP_PORT_COUNT)? {
            if h.op(OP_PORT_DGRAM, port)? == 0 {
                continue;
            }
            let fd = h.op(OP_PORT_FD, port)?.cast_signed();
            if reactors == 0 || fd.rem_euclid(reactors.cast_signed()).cast_unsigned() != id {
                continue;
            }
            if h.op(OP_RT_DGRAM_BIND, port)?.cast_signed() < 0 {
                return Ok(SW_ERR);
            }
        }
    }
    h.get(OP_RT_INIT_REACTOR)?;
    h.get(OP_RT_INIT_PIPE_SOCKETS)?;
    if h.get(OP_MODE)? == MODE_THREAD {
        h.get(OP_RT_THREAD_WORKER)?;
        if h.op(OP_FIELD, u64::from(FIELD_HEARTBEAT_INTERVAL))? > 0 {
            h.get(OP_RT_HEARTBEAT_TIMER)?;
        }
    }
    if h.is(OP_RT_HAS_PIPE_COMMAND)? {
        h.get(OP_RT_PIPE_COMMAND)?;
    }
    if !h.is(OP_RT_BUS_SETUP)? {
        return Ok(SW_ERR);
    }
    let reactors = h.get(OP_REACTOR_NUM)?;
    for worker in 0..h.get(OP_WORKER_NUM)? {
        if reactors == 0 || worker % reactors != id {
            continue;
        }
        if h.op(OP_RT_ADD_WORKER_PIPE, worker)?.cast_signed() < 0 {
            return Ok(SW_ERR);
        }
    }
    Ok(SW_OK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::Script;

    fn go(h: &mut Script, operation: u32, arg: u64) -> i64 {
        run(h, operation, arg).unwrap().unwrap()
    }

    #[test]
    fn pipe_write_drops_closed_sessions_and_stops_when_the_pipe_is_full() {
        let mut h = Script::with(&[
            (OP_RT_FRONT_STREAM, 1),
            (OP_RT_FRONT_CONN, 1),
            (OP_RT_CATCH_WAIT, 1),
        ])
        .queue(OP_RT_BUF_EMPTY, &[0, 0, 1])
        .queue(OP_RT_FRONT_CONN_CLOSED, &[1, 0])
        .queue(OP_RT_SEND_FRONT, &[u64::MAX]);
        assert_eq!(go(&mut h, RUN_RT_PIPE_WRITE, 0), SW_OK);
        assert_eq!(h.called(LOG_RT_CLOSED_BY_SERVER), 1);
        assert_eq!(h.called(OP_RT_POP), 1);
        assert_eq!(h.called(OP_RT_REMOVE_WRITE), 0);
        let mut h = Script::with(&[(OP_RT_FRONT_STREAM, 1), (OP_RT_DISCARD_TIMEOUT, 1)])
            .queue(OP_RT_BUF_EMPTY, &[0, 1]);
        go(&mut h, RUN_RT_PIPE_WRITE, 0);
        assert_eq!(
            (h.called(LOG_RT_DISCARD), h.called(OP_RT_SEND_FRONT)),
            (1, 0)
        );
        assert_eq!(h.called(OP_RT_REMOVE_WRITE), 1);
    }

    #[test]
    fn init_binds_only_this_threads_ports_and_pipes() {
        let mut h = Script::with(&[
            (OP_HAVE_DGRAM, 1),
            (OP_REACTOR_NUM, 2),
            (OP_PORT_COUNT, 3),
            (OP_PORT_DGRAM, 1),
            (OP_WORKER_NUM, 4),
            (OP_RT_BUS_SETUP, 1),
        ])
        .queue(OP_PORT_FD, &[4, 5, 7]);
        assert_eq!(go(&mut h, RUN_RT_INIT, 1), SW_OK);
        assert_eq!(h.args(OP_RT_DGRAM_BIND), vec![1, 2]);
        assert_eq!(h.args(OP_RT_ADD_WORKER_PIPE), vec![1, 3]);
        let mut h = Script::default();
        assert_eq!(go(&mut h, RUN_RT_INIT, 0), SW_ERR);
    }
}
