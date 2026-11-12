//! `swoole::redis::recv_packet`, the redis port's read: protocols-ffi's
//! [`recv_packet`] loop over the connection's own records.
use core::ffi::c_int;
use swoole_rs_buffer::string::CppString;
use swoole_rs_corebase::{DataHead, log, net_types::Protocol};
use swoole_rs_protocols_ffi::{
    ORIGIN, RESP_RECV_CLOSE, RESP_RECV_DONE, RespConnection, RespRequest, SW_ERR, request,
};
use swoole_rs_reactor::Socket;
use swoole_rs_server::ConnectionRecord;

/// `catch_read_error`'s `SW_CLOSE` and `SW_ERROR`.
const CLOSE: c_int = 3;
const ERROR: c_int = 4;

/// `swoole::RecvData`.
#[repr(C)]
struct RecvData {
    info: DataHead,
    data: *const u8,
}

/// The connection's protocol, socket, receive buffer and request. The buffer and
/// request stay raw: `onPackage` may free both.
struct Connection<'a> {
    protocol: &'a Protocol,
    socket: Socket<'a>,
    buffer: *mut CppString,
    request: *mut RespRequest,
    fd: c_int,
}

impl Connection<'_> {
    fn string(&mut self) -> &mut CppString {
        // SAFETY: the live receive buffer; this borrow is not held across `onPackage`.
        unsafe { &mut *self.buffer }
    }
}

impl RespConnection for Connection<'_> {
    fn recv(&mut self) -> isize {
        let socket = self.socket;
        let buffer = self.string();
        let received = swoole_rs_tls::upstream::recv(socket, buffer.spare(), 0);
        if received >= 0 {
            let length = buffer.len() + received.cast_unsigned();
            // SAFETY: the read initialized the `received` spare bytes after the content.
            unsafe { buffer.set_len(length) };
            return received;
        }
        let error = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        match socket.catch_error(error) {
            ERROR => {
                log::sys_warning(ORIGIN, format_args!("recv from socket#{} failed", self.fd));
                RESP_RECV_DONE
            }
            CLOSE => RESP_RECV_CLOSE,
            _ => RESP_RECV_DONE,
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

    fn dispatch(&mut self) -> c_int {
        let buffer = self.string();
        let data = RecvData {
            info: DataHead {
                len: u32::try_from(buffer.len()).unwrap_or(u32::MAX),
                ..DataHead::default()
            },
            data: buffer.as_mut_ptr(),
        };
        let Some(on_package) = self.protocol.on_package else {
            return -1;
        };
        // SAFETY: upstream's call, over the live protocol, socket and packet.
        if unsafe {
            on_package(
                self.protocol,
                self.socket.as_ptr().cast(),
                (&raw const data).cast(),
            )
        } < 0
        {
            return -1;
        }
        c_int::from(self.socket.read(|s| s.removed) != 0)
    }

    fn clear(&mut self) {
        self.string().clear();
    }
}

/// `int redis::recv_packet(Protocol *, Connection *, String *)` over the connection's
/// protocol, record and `socket`.
///
/// # Safety
/// `buffer` is live; `conn->object` is null or this receive's request.
pub(crate) unsafe fn recv_packet(
    protocol: &Protocol,
    conn: &mut ConnectionRecord,
    socket: Socket<'_>,
    buffer: *mut CppString,
) -> c_int {
    let mut slot = conn.object.cast::<RespRequest>();
    let request = request(&mut slot);
    conn.object = slot.cast();
    if request.is_null() {
        return SW_ERR;
    }
    let mut connection = Connection {
        protocol,
        socket,
        buffer,
        request,
        fd: conn.fd,
    };
    let page = swoole_rs_corebase::swr_v1_core_pagesize() as usize;
    swoole_rs_protocols_ffi::recv_packet(&mut connection, protocol.package_max_length, page)
}

#[cfg(test)]
mod tests {
    use super::*;
    use swoole_rs_reactor::SocketRecord;

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
        conn.fd = pairs[1];
        conn.socket = (&raw mut socket).cast();
        let handle = unsafe { Socket::from_ptr(&raw mut socket) }.unwrap();
        let mut protocol = Protocol::EMPTY;
        protocol.package_max_length = 65536 * 2;
        let mut buffer = CppString::EMPTY;
        buffer.alloc(65536, None);
        let buffer_size = buffer.size();

        assert_eq!(
            unsafe { recv_packet(&protocol, &mut conn, handle, &raw mut buffer) },
            SW_ERR
        );
        assert_eq!(buffer.len(), 1);
        assert_eq!(buffer.size(), buffer_size);

        unsafe { libc::free(conn.object) };
        buffer.dispose();
        unsafe { libc::close(pairs[1]) };
    }
}
