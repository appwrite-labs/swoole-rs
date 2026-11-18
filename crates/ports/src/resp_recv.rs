//! `swoole::redis::recv_packet`, the redis port's read: crates/clients'
//! [`recv_packet`] loop over the connection's own records.
use core::cell::Cell;
use core::ffi::{c_int, c_long};
use swoole_rs_clients::resp_recv::{Dispatched, ORIGIN, Received, RespConnection, RespRequest};
use swoole_rs_corebase::buffer::string::CppString;
use swoole_rs_corebase::reactor::{Caught, Socket};
use swoole_rs_corebase::{CellExt, DataHead, log};
use swoole_rs_server::SendData;

thread_local! {
    /// Each fd's request state on this reactor thread (upstream's `conn->object`),
    /// with the session it belongs to. A session that finds another's starts afresh,
    /// as upstream's does once the close freed the last one.
    static REQUESTS: Cell<Vec<(c_long, RespRequest)>> = const { Cell::new(Vec::new()) };
}

/// `f` over this thread's request states, taken out of the cell for `f` only.
fn requests<R>(f: impl FnOnce(&mut Vec<(c_long, RespRequest)>) -> R) -> R {
    REQUESTS.with(|cell| cell.with_taken(f))
}

/// The connection's socket, receive buffer and request state, and where a whole
/// command goes. The read owns the buffer throughout dispatch.
struct Connection<'a, D> {
    socket: Socket<'a>,
    buffer: &'a mut CppString,
    request: RespRequest,
    fd: c_int,
    dispatch: D,
}

impl<D: FnMut(SendData<'_>) -> bool> RespConnection for Connection<'_, D> {
    fn recv(&mut self) -> Received {
        let socket = self.socket;
        if let Some(received) = swoole_rs_corebase::tls::upstream::recv_append(socket, self.buffer)
        {
            return Received::Bytes(received);
        }
        let error = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        match socket.catch_error(error) {
            Caught::Error => {
                log::sys_warning(ORIGIN, format_args!("recv from socket#{} failed", self.fd));
                Received::Done
            }
            Caught::Close => Received::Closed,
            Caught::Wait | Caught::Invalid | Caught::ReduceSize => Received::Done,
        }
    }

    fn view(&mut self) -> (&[u8], usize, &mut RespRequest) {
        (self.buffer.view(), self.buffer.size(), &mut self.request)
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
        if self.socket.read(|s| s.removed) {
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
/// record and `socket`, up to the protocol's `max_length`, each whole command to
/// `dispatch` (the port's `onPackage`, which `init_protocol` set to
/// `Server::dispatch_task`'s): whether the connection stays open. `conn` is a
/// session of this reactor thread's redis port.
pub(crate) fn recv_packet<'a>(
    max_length: u32,
    conn: swoole_rs_server::Connection<'a>,
    socket: Socket<'a>,
    buffer: &'a mut CppString,
    dispatch: impl FnMut(SendData<'_>) -> bool,
) -> bool {
    let (fd, session) = (conn.fd(), conn.session_id());
    let slot = usize::try_from(fd).ok();
    // A new session's is zeroed, which every field takes (`RespState::TotalLine`).
    let request = slot
        .and_then(|slot| requests(|all| all.get(slot).copied()))
        .filter(|&(owner, _)| owner == session)
        .map_or_default(|(_, request)| request);
    let mut connection = Connection {
        socket,
        buffer,
        request,
        fd,
        dispatch,
    };
    let page = swoole_rs_corebase::swr_v1_core_pagesize() as usize;
    let open = swoole_rs_clients::resp_recv::recv_packet(&mut connection, max_length, page);
    // Stored afterwards, not held across `onPackage`, which may read another session.
    if let Some(slot) = slot {
        requests(|all| {
            if all.len() <= slot {
                all.resize(slot + 1, (0, RespRequest::default()));
            }
            if let Some(stored) = all.get_mut(slot) {
                *stored = (session, connection.request);
            }
        });
    }
    open
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;
    use swoole_rs_clients::protocol::Protocol;
    use swoole_rs_corebase::reactor::SocketRecord;
    use swoole_rs_server::ConnectionRecord;

    /// `core-tests/src/protocol/redis.cpp`, `TEST(redis, recv_packet_with_partially_filled_buffer)`.
    #[test]
    fn recv_packet_with_partially_filled_buffer() {
        let (mut peer, reader) = UnixStream::pair().unwrap();
        assert_eq!(peer.write(b"*").unwrap(), 1);
        peer.shutdown(std::net::Shutdown::Write).unwrap();

        let mut socket = SocketRecord::new();
        socket.fd = reader.as_raw_fd();
        socket.socket_type = 5; // SW_SOCK_UNIX_STREAM
        let mut conn = ConnectionRecord::default();
        conn.fd.set(reader.as_raw_fd());
        let handle = Socket::new(&mut socket);
        let mut protocol = Protocol::EMPTY;
        protocol.package_max_length = 65536 * 2;
        let mut buffer = CppString::EMPTY;
        buffer.alloc(65536, None);
        let buffer_size = buffer.size();

        let max_length = protocol.package_max_length;
        let dispatch = |_: SendData<'_>| unreachable!("a partial command");
        assert!(!recv_packet(
            max_length,
            (&mut conn).into(),
            handle,
            &mut buffer,
            dispatch,
        ));
        assert_eq!(buffer.len(), 1);
        assert_eq!(buffer.size(), buffer_size);

        // The session's state stays for its next read.
        let slot = usize::try_from(reader.as_raw_fd()).unwrap();
        assert!(requests(|all| all
            .get(slot)
            .is_some_and(|&(owner, _)| owner == 0)));
        buffer.dispose();
    }
}
