//! `swoole::redis::recv_packet`, the redis port's read: [`swr_v1_resp_recv`] over
//! the connection's own records.
use core::ffi::{c_int, c_void};
use swoole_rs_buffer::string::CppString;
use swoole_rs_corebase::{DataHead, log, net_types::Protocol};
use swoole_rs_protocols_ffi::{ORIGIN, RESP_RECV_CLOSE, RESP_RECV_DONE, RespIo, swr_v1_resp_recv};
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

struct Connection<'a> {
    protocol: &'a Protocol,
    socket: Socket<'a>,
    buffer: *mut CppString,
    fd: c_int,
}

unsafe extern "C" fn connection_recv(context: *mut c_void, at: *mut u8, n: usize) -> isize {
    // SAFETY: `recv_packet`'s context.
    let c = unsafe { &*context.cast::<Connection<'_>>() };
    // SAFETY: the buffer's free tail.
    let received = unsafe { swoole_rs_tls::upstream::socket_recv(c.socket, at.cast(), n, 0) };
    if received >= 0 {
        return received;
    }
    let error = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    match c.socket.catch_error(error) {
        ERROR => {
            log::sys_warning(ORIGIN, format_args!("recv from socket#{} failed", c.fd));
            RESP_RECV_DONE
        }
        CLOSE => RESP_RECV_CLOSE,
        _ => RESP_RECV_DONE,
    }
}

unsafe extern "C" fn connection_extend(context: *mut c_void, size: usize) {
    // SAFETY: `recv_packet`'s context.
    let buffer = unsafe { (*context.cast::<Connection<'_>>()).buffer };
    // SAFETY: its live String.
    unsafe { (*buffer).extend(size) };
}

unsafe extern "C" fn connection_dispatch(context: *mut c_void, length: usize) -> c_int {
    // SAFETY: `recv_packet`'s context.
    let c = unsafe { &*context.cast::<Connection<'_>>() };
    let data = RecvData {
        info: DataHead {
            len: u32::try_from(length).unwrap_or(u32::MAX),
            ..DataHead::default()
        },
        // SAFETY: the live String.
        data: unsafe { (*c.buffer).as_mut_ptr() },
    };
    let Some(on_package) = c.protocol.on_package else {
        return -1;
    };
    // SAFETY: upstream's call, over the live protocol, socket and packet.
    if unsafe {
        on_package(
            c.protocol,
            c.socket.as_ptr().cast(),
            (&raw const data).cast(),
        )
    } < 0
    {
        return -1;
    }
    c_int::from(c.socket.read(|s| s.removed) != 0)
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
    let mut context = Connection {
        protocol,
        socket,
        buffer,
        fd: conn.fd,
    };
    let io = RespIo {
        context: (&raw mut context).cast(),
        recv: connection_recv,
        extend: connection_extend,
        dispatch: connection_dispatch,
    };
    let maximum = protocol.package_max_length;
    let page = swoole_rs_corebase::swr_v1_core_pagesize() as usize;
    // SAFETY: the String's head is a RespBuffer; the request slot is the connection's.
    unsafe {
        swr_v1_resp_recv(
            &io,
            buffer.cast(),
            (&raw mut conn.object).cast(),
            maximum,
            page,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swoole_rs_protocols_ffi::SW_ERR;
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
