//! The master's message bus: which bus a worker uses, base mode's pass and forward
//! over it, and its buffer.
#[cfg(feature = "upstream-symbols")]
use crate::{Mode, ServerState, view::PipeSocket};

/// `f` over `serv->get_worker_message_bus()`: this thread's in thread mode, else the
/// server's.
#[cfg(feature = "upstream-symbols")]
#[inline]
pub(crate) fn with_worker_message_bus<R>(
    state: &ServerState,
    f: impl FnOnce(crate::objects::Bus<'_>) -> R,
) -> R {
    state.with_worker_bus(|record| f(crate::objects::Bus::over(record)))
}

/// Base mode's message bus takes `send`, then `worker_accept_event` of it.
#[cfg(feature = "upstream-symbols")]
pub(crate) fn base_pass(serv: crate::view::Server<'_>, send: &crate::base_factory::SendData) {
    use crate::event_worker::native::{Payload, accept_event};
    let head = with_worker_message_bus(serv.state(), |bus| bus.rust().pass(send.info()));
    let payload = if send.info().len > 0 {
        Payload::Passed(send.payload())
    } else {
        Payload::Bus
    };
    accept_event(serv, &head, payload);
}

/// The message bus writes `send` to the pipe of the worker that owns its session.
#[cfg(feature = "upstream-symbols")]
pub(crate) fn forward(serv: crate::view::Server<'_>, send: &crate::base_factory::SendData) -> bool {
    let state = serv.state();
    let Some(session) = state.session(send.info().fd) else {
        return false;
    };
    let id = session.reactor_bits() & 0xff;
    let worker = serv.worker_record(u16::try_from(id).unwrap_or(u16::MAX));
    let Some(pipe) = worker.and_then(|worker| PipeSocket::of_worker(worker, true)) else {
        return false;
    };
    with_worker_message_bus(state, |bus| {
        let pipe = if state.config_ref().mode_ == Mode::Thread {
            PipeSocket::of_bus(bus.rust(), pipe)
        } else {
            Some(pipe)
        };
        let Some(pipe) = pipe else {
            return false;
        };
        let mut send = *send;
        send.send(bus.rust(), pipe)
    })
}

/// `Server::create_pipe_buffers()`: the bus's buffer, `ipc_max_size` bytes.
#[cfg(feature = "upstream-symbols")]
pub(crate) fn create_pipe_buffers(serv: crate::view::Server<'_>) -> bool {
    let state = serv.state();
    let size = state.config_ref().ipc_max_size();
    state.message_bus().alloc_buffer(size as usize)
}
