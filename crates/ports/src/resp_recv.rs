//! `swoole::redis::recv_packet`, the redis port's read: crates/clients'
//! [`recv_packet`] loop over the connection's own records.
use swoole_rs_clients::resp_recv::{Dispatched, ORIGIN, Received, RespConnection, RespRequest};
use swoole_rs_corebase::buffer::string::StringBuffer;
use swoole_rs_corebase::reactor::{Caught, Socket, SocketFields};
use swoole_rs_corebase::{DataHead, log};
use swoole_rs_server::SendData;

/// The connection's socket, receive buffer and request state, and where a whole
/// command goes. The read owns the buffer throughout dispatch.
struct Connection<'a, D> {
    socket: Socket<'a>,
    buffer: &'a mut StringBuffer,
    maximum: usize,
    request: RespRequest,
    dispatch: D,
}

impl<D: FnMut(SendData<'_>) -> bool> RespConnection for Connection<'_, D> {
    fn recv(&mut self) -> Received {
        let socket = self.socket;
        let remaining = self.maximum.saturating_sub(self.buffer.len());
        if remaining == 0 {
            return Received::Closed;
        }
        if let Some(received) = self.buffer.fill_within(remaining, |spare| {
            swoole_rs_corebase::tls::upstream::recv_spare(socket, spare, 0)
        }) {
            return Received::Bytes(received);
        }
        let error = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        match socket.catch_error(error) {
            Caught::Error => {
                log::sys_warning(
                    ORIGIN,
                    format_args!("recv from socket#{} failed", self.socket.fd()),
                );
                Received::Done
            }
            Caught::Close => Received::Closed,
            Caught::Wait | Caught::Invalid | Caught::ReduceSize => Received::Done,
        }
    }

    fn view(&mut self) -> (&[u8], usize, &mut RespRequest) {
        (
            self.buffer.view(),
            self.buffer.size().min(self.maximum),
            &mut self.request,
        )
    }

    fn set_offset(&mut self, offset: i64) {
        self.buffer.set_offset(offset);
    }

    fn extend(&mut self, size: usize) {
        self.buffer.extend(size);
    }

    fn dispatch(&mut self) -> Dispatched {
        if !(self.dispatch)(SendData::new(DataHead::default(), self.buffer.view())) {
            return Dispatched::Failed;
        }
        if self.socket.read(SocketFields::has_left_reactor) {
            Dispatched::Removed
        } else {
            Dispatched::Kept
        }
    }

    fn clear(&mut self) {
        self.buffer.clear();
    }
}

/// `int redis::recv_packet(Protocol *, Connection *, String *)` over the connection's
/// `socket`, up to the protocol's `max_length`, each whole command to
/// `dispatch` (the port's `onPackage`, which `init_protocol` set to
/// `Server::dispatch_task`'s): whether the connection stays open.
pub(crate) fn recv_packet<'a>(
    max_length: u32,
    socket: Socket<'a>,
    buffer: &'a mut StringBuffer,
    dispatch: impl FnMut(SendData<'_>) -> bool,
) -> bool {
    let mut connection = Connection {
        socket,
        buffer,
        maximum: max_length as usize,
        request: RespRequest::default(),
        dispatch,
    };
    let page = swoole_rs_corebase::swr_v1_core_pagesize() as usize;
    swoole_rs_clients::resp_recv::recv_packet(&mut connection, max_length, page)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;
    use swoole_rs_clients::protocol::Protocol;
    use swoole_rs_corebase::reactor::SocketRecord;

    #[test]
    fn package_limit_bounds_each_socket_read() {
        let command = b"*1\r\n$4\r\nPING\r\n";
        let (mut peer, reader) = UnixStream::pair().unwrap();
        peer.write_all(&command.repeat(2)).unwrap();
        peer.shutdown(std::net::Shutdown::Write).unwrap();

        let mut socket = SocketRecord::new();
        socket.fd = reader.as_raw_fd();
        socket.socket_type = 5;
        let socket = Socket::new(&mut socket);
        let mut buffer = StringBuffer::EMPTY;
        buffer.alloc(4096, None);
        let mut packages = 0;
        for _ in 0..2 {
            assert!(recv_packet(
                u32::try_from(command.len()).unwrap(),
                socket,
                &mut buffer,
                |packet| {
                    assert_eq!(packet.payload(), command);
                    packages += 1;
                    true
                },
            ));
        }
        buffer.dispose();
        assert_eq!(packages, 2);
    }

    /// `core-tests/src/protocol/redis.cpp`, `TEST(redis, recv_packet_with_partially_filled_buffer)`.
    #[test]
    fn recv_packet_with_partially_filled_buffer() {
        let (mut peer, reader) = UnixStream::pair().unwrap();
        assert_eq!(peer.write(b"*").unwrap(), 1);
        peer.shutdown(std::net::Shutdown::Write).unwrap();

        let mut socket = SocketRecord::new();
        socket.fd = reader.as_raw_fd();
        socket.socket_type = 5; // SW_SOCK_UNIX_STREAM
        let handle = Socket::new(&mut socket);
        let mut protocol = Protocol::EMPTY;
        protocol.package_max_length = 65536 * 2;
        let mut buffer = StringBuffer::EMPTY;
        buffer.alloc(65536, None);
        let buffer_size = buffer.size();

        let max_length = protocol.package_max_length;
        let dispatch = |_: SendData<'_>| unreachable!("a partial command");
        assert!(!recv_packet(max_length, handle, &mut buffer, dispatch));
        assert_eq!(buffer.len(), 1);
        assert_eq!(buffer.size(), buffer_size);

        buffer.dispose();
    }
}
