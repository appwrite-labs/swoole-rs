//! The master's message bus: which bus a worker uses, base mode's pass and forward
//! over it, and its buffer.
#[cfg(all(feature = "upstream-symbols", not(test)))]
use crate::{Mode, ServerState};
use core::ffi::c_int;
use swoole_rs_corebase::net_types::SocketAddress;

/// `DgramPacket` up to its address.
#[repr(C)]
pub(crate) struct DgramPacket {
    socket_type: c_int,
    pub(crate) socket_addr: SocketAddress,
}

/// `serv->get_worker_message_bus()`: this thread's in thread mode, else the
/// server's.
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub(crate) fn worker_message_bus(state: &ServerState) -> crate::objects::Bus<'_> {
    let thread_mode = state.config_ref().mode_ == Mode::Thread;
    let bus = if thread_mode {
        swoole_rs_corebase::thread_message_bus()
    } else {
        state.tables_ref().message_bus
    };
    // SAFETY: the thread's or the server's live `MessageBus`.
    unsafe { crate::objects::Bus::of(bus) }
}

/// Base mode's message bus takes `send`, then `worker_accept_event` of it.
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub(crate) fn base_pass(serv: crate::view::Server<'_>, send: &crate::base_factory::SendData) {
    let bus = worker_message_bus(serv.state());
    bus.rust().pass(&send.info, send.data);
    crate::event_worker::native::accept_event(serv, &bus.head());
}

/// The message bus writes `send` to the pipe of the worker that owns its session.
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub(crate) fn forward(serv: crate::view::Server<'_>, send: &crate::base_factory::SendData) -> bool {
    let state = serv.state();
    let Some(session) = state.session(send.info.fd) else {
        return false;
    };
    let id = session.reactor_bits() & 0xff;
    let Some(pipe) = serv.worker_pipe(u16::try_from(id).unwrap_or(u16::MAX), true) else {
        return false;
    };
    let bus = worker_message_bus(state);
    let socket = if state.config_ref().mode_ == Mode::Thread {
        bus.pipe_socket(pipe)
    } else {
        Some(pipe)
    };
    let Some(socket) = socket else {
        return false;
    };
    let mut info = send.info;
    // SAFETY: a live pipe socket and the send it writes.
    unsafe {
        bus.rust()
            .send(socket.as_ptr(), &mut info, send.data.cast())
    }
}

/// `Server::create_pipe_buffers()`: the bus's buffer, `ipc_max_size` bytes.
#[cfg(all(feature = "upstream-symbols", not(test)))]
pub(crate) fn create_pipe_buffers(serv: crate::view::Server<'_>) -> c_int {
    let state = serv.state();
    let size = state.config_ref().ipc_max_size();
    if state.message_bus().alloc_buffer(size as usize) {
        0
    } else {
        -1
    }
}
