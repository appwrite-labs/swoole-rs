//! `swoole::redis::recv_packet`, the redis port's read: protocols-ffi's
//! [`recv_packet`] loop over the connection's own records.
use core::ffi::c_int;
use swoole_rs_buffer::string::CppString;
use swoole_rs_corebase::{DataHead, log};
use swoole_rs_protocols_ffi::{Dispatched, ORIGIN, Received, RespConnection, RespRequest};
use swoole_rs_reactor::{Caught, Socket};
use swoole_rs_server::reactor_entry::RecvData;

use crate::framing_recv::Lent;

/// The connection's socket, receive buffer and request, and where a whole command
/// goes. The buffer and request stay lent: the dispatch may free both.
struct Connection<'a, D> {
    socket: Socket<'a>,
    buffer: Lent<'a>,
    request: *mut RespRequest,
    fd: c_int,
    dispatch: D,
}

impl<D> Connection<'_, D> {
    fn string(&mut self) -> &mut CppString {
        self.buffer.get()
    }
}

impl<D: FnMut(RecvData<'_>) -> bool> RespConnection for Connection<'_, D> {
    fn recv(&mut self) -> Received {
        let socket = self.socket;
        let buffer = self.string();
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
        // SAFETY: the connection's live request, apart from the buffer and not held
        // across `onPackage`.
        let request = unsafe { &mut *self.request };
        let buffer = &*self.string();
        (buffer.view(), buffer.size(), request)
    }

    fn set_offset(&mut self, offset: i64) {
        self.string().set_offset(offset);
    }

    fn extend(&mut self, size: usize) {
        self.string().extend(size);
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
        self.string().clear();
    }
}

/// `int redis::recv_packet(Protocol *, Connection *, String *)` over the connection's
/// record and `socket`, up to the protocol's `max_length`, each whole command to
/// `dispatch` (the port's `onPackage`, which `init_protocol` set to
/// `Server::dispatch_task`'s): whether the connection stays open.
///
/// # Safety
/// `conn` is a session of this reactor's redis port, whose request state, if any,
/// this reactor made.
pub(crate) unsafe fn recv_packet<'a>(
    max_length: u32,
    conn: swoole_rs_server::Connection<'a>,
    socket: Socket<'a>,
    buffer: Lent<'a>,
    dispatch: impl FnMut(RecvData<'_>) -> bool,
) -> bool {
    // Zeroed, which every field of a RespRequest takes (`RespState::TotalLine`).
    let request = match conn.resp_request() {
        Some(request) => request,
        None => conn.set_resp_request(Box::default()),
    };
    let mut connection = Connection {
        socket,
        buffer,
        request,
        fd: conn.fd(),
        dispatch,
    };
    let page = swoole_rs_corebase::swr_v1_core_pagesize() as usize;
    swoole_rs_protocols_ffi::recv_packet(&mut connection, max_length, page)
}

#[cfg(test)]
mod tests {
    use super::*;
    use swoole_rs_clients::protocol::Protocol;
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
        conn.socket.set(Some(handle));
        let mut protocol = Protocol::EMPTY;
        protocol.package_max_length = 65536 * 2;
        let mut buffer = CppString::EMPTY;
        buffer.alloc(65536, None);
        let buffer_size = buffer.size();

        let max_length = protocol.package_max_length;
        let dispatch = |_: RecvData<'_>| unreachable!("a partial command");
        assert!(!unsafe {
            recv_packet(
                max_length,
                (&mut conn).into(),
                handle,
                Lent::new(&mut buffer),
                dispatch,
            )
        });
        assert_eq!(buffer.len(), 1);
        assert_eq!(buffer.size(), buffer_size);

        let request = swoole_rs_server::Connection::from(&mut conn).take_resp_request();
        drop(unsafe { Box::from_raw(request.unwrap()) });
        buffer.dispose();
        unsafe { libc::close(pairs[1]) };
    }
}
