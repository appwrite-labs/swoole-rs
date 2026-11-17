//! `swoole::redis::recv_packet`, the redis port's read: crates/clients'
//! [`recv_packet`] loop over the connection's own records.
use core::cell::Cell;
use core::ffi::{c_int, c_long};
use swoole_rs_clients::resp_recv::{Dispatched, ORIGIN, Received, RespConnection, RespRequest};
use swoole_rs_corebase::{DataHead, log};
use swoole_rs_reactor::{Caught, Socket};
use swoole_rs_server::reactor_entry::RecvData;

use crate::framing_recv::Lent;

thread_local! {
    /// Each fd's request state on this reactor thread (upstream's `conn->object`),
    /// with the session it belongs to. A session that finds another's starts afresh,
    /// as upstream's does once the close freed the last one.
    static REQUESTS: Cell<Vec<(c_long, RespRequest)>> = const { Cell::new(Vec::new()) };
}

/// `f` over this thread's request states, taken out of the cell for `f` only.
fn requests<R>(f: impl FnOnce(&mut Vec<(c_long, RespRequest)>) -> R) -> R {
    REQUESTS.with(|cell| {
        let mut requests = cell.take();
        let result = f(&mut requests);
        cell.set(requests);
        result
    })
}

/// The connection's socket, receive buffer and request state, and where a whole
/// command goes. The buffer stays lent: the dispatch may free it.
struct Connection<'a, D> {
    socket: Socket<'a>,
    buffer: Lent<'a>,
    request: RespRequest,
    fd: c_int,
    dispatch: D,
}

impl<D: FnMut(RecvData<'_>) -> bool> RespConnection for Connection<'_, D> {
    fn recv(&mut self) -> Received {
        let socket = self.socket;
        let buffer = self.buffer.get();
        if let Some(received) = swoole_rs_tls::upstream::recv_append(socket, buffer) {
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
        let buffer = &*self.buffer.get();
        (buffer.view(), buffer.size(), &mut self.request)
    }

    fn set_offset(&mut self, offset: i64) {
        self.buffer.get().set_offset(offset);
    }

    fn extend(&mut self, size: usize) {
        self.buffer.get().extend(size);
    }

    fn dispatch(&mut self) -> Dispatched {
        let buffer = self.buffer.get();
        if !(self.dispatch)(RecvData::new(DataHead::default(), buffer.view())) {
            return Dispatched::Failed;
        }
        if self.socket.read(|s| s.removed) {
            Dispatched::Removed
        } else {
            Dispatched::Kept
        }
    }

    fn clear(&mut self) {
        self.buffer.get().clear();
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
    buffer: Lent<'a>,
    dispatch: impl FnMut(RecvData<'_>) -> bool,
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
    use swoole_rs_clients::protocol::Protocol;
    use swoole_rs_corebase::buffer::string::CppString;
    use swoole_rs_reactor::SocketRecord;
    use swoole_rs_server::ConnectionRecord;

    /// `core-tests/src/protocol/redis.cpp`, `TEST(redis, recv_packet_with_partially_filled_buffer)`.
    #[test]
    fn recv_packet_with_partially_filled_buffer() {
        let mut pairs = [0; 2];
        assert_eq!(
            unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, pairs.as_mut_ptr()) },
            0
        );
        assert_eq!(
            unsafe { libc::send(pairs[0], b"*".as_ptr().cast(), 1, 0) },
            1
        );
        unsafe { libc::close(pairs[0]) };

        let mut socket = SocketRecord::new();
        socket.fd = pairs[1];
        socket.socket_type = 5; // SW_SOCK_UNIX_STREAM
        let mut conn: ConnectionRecord = unsafe { core::mem::zeroed() };
        conn.fd.set(pairs[1]);
        let handle = unsafe { Socket::from_ptr(&raw mut socket) }.unwrap();
        let mut protocol = Protocol::EMPTY;
        protocol.package_max_length = 65536 * 2;
        let mut buffer = CppString::EMPTY;
        buffer.alloc(65536, None);
        let buffer_size = buffer.size();

        let max_length = protocol.package_max_length;
        let dispatch = |_: RecvData<'_>| unreachable!("a partial command");
        assert!(!recv_packet(
            max_length,
            (&mut conn).into(),
            handle,
            Lent::new(&mut buffer),
            dispatch,
        ));
        assert_eq!(buffer.len(), 1);
        assert_eq!(buffer.size(), buffer_size);

        // The session's state stays for its next read.
        let slot = usize::try_from(pairs[1]).unwrap();
        assert!(requests(|all| all
            .get(slot)
            .is_some_and(|&(owner, _)| owner == 0)));
        buffer.dispose();
        unsafe { libc::close(pairs[1]) };
    }
}
