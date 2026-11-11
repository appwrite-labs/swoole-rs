//! `swoole::redis::recv_packet`, the redis port's read: [`swr_v1_resp_recv`] over
//! the connection's own records.
use core::ffi::{c_int, c_void};
use swoole_rs_buffer::string::CppString;
use swoole_rs_corebase::{DataHead, log, net_types::Protocol};
use swoole_rs_protocols_ffi::{ORIGIN, RESP_RECV_CLOSE, RESP_RECV_DONE, RespIo, swr_v1_resp_recv};
use swoole_rs_reactor::{Socket, SocketRecord};
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

type OnPackage = unsafe extern "C" fn(*const Protocol, *mut SocketRecord, *const RecvData) -> c_int;

struct Connection<'a> {
    protocol: *const Protocol,
    socket: Socket<'a>,
    buffer: *mut CppString,
    fd: c_int,
}

unsafe extern "C" fn connection_recv(context: *mut c_void, at: *mut u8, n: usize) -> isize {
    // SAFETY: `recv_packet`'s context.
    let c = unsafe { &*context.cast::<Connection<'_>>() };
    let socket = c.socket.as_ptr();
    // SAFETY: the connection's live socket and the buffer's free tail.
    let received = unsafe { swoole_rs_tls::upstream::socket_recv(socket, at.cast(), n, 0) };
    if received >= 0 {
        return received;
    }
    let error = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    match c.socket.edit(|s| s.catch_error(error)) {
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
        data: unsafe { (*c.buffer).str },
    };
    // SAFETY: the live protocol.
    let on_package = unsafe { (*c.protocol).on_package };
    // SAFETY: `onPackage` is a function of this type, or null.
    let on_package = unsafe { core::mem::transmute::<*mut c_void, Option<OnPackage>>(on_package) };
    let Some(on_package) = on_package else {
        return -1;
    };
    // SAFETY: upstream's call, over the live protocol, socket and packet.
    if unsafe { on_package(c.protocol, c.socket.as_ptr(), &raw const data) } < 0 {
        return -1;
    }
    c_int::from(c.socket.read(|s| s.removed) != 0)
}

/// `int redis::recv_packet(Protocol *, Connection *, String *)`.
///
/// # Safety
/// `protocol`, `conn` and its socket, and `buffer` are live; `conn->object` is
/// null or this receive's request.
pub(crate) unsafe fn recv_packet(
    protocol: *const Protocol,
    conn: *mut c_void,
    buffer: *mut CppString,
) -> c_int {
    // SAFETY: as the caller promises.
    let conn = unsafe { &mut *conn.cast::<ConnectionRecord>() };
    // SAFETY: the connection's live socket, as the caller promises.
    let Some(socket) = (unsafe { Socket::from_ptr(conn.socket.cast()) }) else {
        return -1;
    };
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
    // SAFETY: the live protocol.
    let maximum = unsafe { (*protocol).package_max_length };
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
        let mut protocol: Protocol = unsafe { core::mem::zeroed() };
        protocol.package_max_length = 65536 * 2;
        let mut buffer = CppString::EMPTY;
        buffer.alloc(65536, core::ptr::null());
        let buffer_size = buffer.size;

        assert_eq!(
            unsafe { recv_packet(&raw const protocol, (&raw mut conn).cast(), &raw mut buffer) },
            SW_ERR
        );
        assert_eq!(buffer.length, 1);
        assert_eq!(buffer.size, buffer_size);

        unsafe { libc::free(conn.object) };
        buffer.dispose();
        unsafe { libc::close(pairs[1]) };
    }
}
