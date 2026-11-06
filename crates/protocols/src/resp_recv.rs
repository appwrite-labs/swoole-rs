//! `swoole::redis::recv_packet`: reads a connection until a whole RESP command is
//! buffered, then hands it to `onPackage`. [`swr_v1_resp_recv`] is the loop over
//! borrowed effects; the export runs it over the server's own records.
use crate::{RespRequest, swr_v1_resp_extend_size, swr_v1_resp_receive_range};
use core::ffi::{c_int, c_void};
use swoole_rs_buffer::string::CppString;
use swoole_rs_corebase::{DataHead, log, net_types::Protocol, sw_malloc};
use swoole_rs_reactor::SocketRecord;
use swoole_rs_server::ConnectionRecord;

const SW_OK: c_int = 0;
const SW_ERR: c_int = -1;
/// `catch_read_error`'s `SW_CLOSE` and `SW_ERROR`.
const CLOSE: c_int = 3;
const ERROR: c_int = 4;
const ORIGIN: &core::ffi::CStr = c"redis::recv_packet";

/// [`RespIo::recv`]: the connection closes (`SW_ERR`).
pub const RESP_RECV_CLOSE: isize = -1;
/// [`RespIo::recv`]: the read ends with `SW_OK` (it would block, or a logged error).
pub const RESP_RECV_DONE: isize = -2;

/// The head of `swoole::String`.
#[repr(C)]
pub struct RespBuffer {
    pub length: usize,
    pub size: usize,
    pub offset: i64,
    pub str: *mut u8,
}

/// The socket and buffer effects of one receive, lent by the caller.
#[repr(C)]
pub struct RespIo {
    pub context: *mut c_void,
    /// `socket->recv` of at most `n` bytes into `at`, with upstream's read-error
    /// handling and warning: the byte count, [`RESP_RECV_CLOSE`] or [`RESP_RECV_DONE`].
    pub recv: unsafe extern "C" fn(context: *mut c_void, at: *mut u8, n: usize) -> isize,
    /// `buffer->extend(size)`.
    pub extend: unsafe extern "C" fn(context: *mut c_void, size: usize),
    /// `protocol->onPackage` of the buffer's first `length` bytes: negative when it
    /// fails, 1 when the socket was removed meanwhile, else 0.
    pub dispatch: unsafe extern "C" fn(context: *mut c_void, length: usize) -> c_int,
}

fn set_errno(value: c_int) {
    #[cfg(target_os = "linux")]
    // SAFETY: returns this thread's errno slot, live for the thread.
    let slot = unsafe { libc::__errno_location() };
    #[cfg(not(target_os = "linux"))]
    // SAFETY: returns this thread's errno slot, live for the thread.
    let slot = unsafe { libc::__error() };
    // SAFETY: `slot` is this thread's errno.
    unsafe { slot.write(value) };
}

/// `redis::recv_packet`'s loop: receives into `buffer` until the RESP state in
/// `*request` (allocated with `sw_malloc` when null) sees a whole command or must
/// wait, growing the buffer up to `maximum`. `page` is `swoole_pagesize()`.
///
/// # Safety
/// `io`'s callbacks are valid for its context; `buffer` is that context's live
/// String head, which only `extend` reallocates; `request` points to null or to a
/// live `swr_resp_request` from `sw_malloc`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_resp_recv(
    io: *const RespIo,
    buffer: *mut RespBuffer,
    request: *mut *mut RespRequest,
    maximum: u32,
    page: usize,
) -> c_int {
    // SAFETY: as the caller promises.
    let io = unsafe { &*io };
    // SAFETY: as the caller promises.
    let slot = unsafe { &mut *request };
    if slot.is_null() {
        let fresh = sw_malloc(size_of::<RespRequest>()).cast::<RespRequest>();
        if fresh.is_null() {
            log::warning(
                ORIGIN,
                format_args!("malloc({}) failed", size_of::<RespRequest>()),
            );
            return SW_ERR;
        }
        // SAFETY: a fresh allocation of one RespRequest.
        unsafe { fresh.write_bytes(0, 1) };
        *slot = fresh;
    }
    let request = *slot;
    let maximum = maximum as usize;
    loop {
        // SAFETY: the caller's live String head; this copy is not held across effects.
        let RespBuffer {
            length,
            size,
            str: data,
            ..
        } = unsafe { buffer.read() };
        // SAFETY: the free tail of the buffer, `size - length` writable bytes.
        let received = unsafe { (io.recv)(io.context, data.wrapping_add(length), size - length) };
        if received == 0 || received == RESP_RECV_CLOSE {
            return SW_ERR;
        }
        let Ok(received) = usize::try_from(received) else {
            return SW_OK;
        };
        let length = length + received;
        // SAFETY: as above.
        unsafe { (*buffer).length = length };
        let (mut set_offset, mut range) = (0u8, 0u8);
        // SAFETY: `length` initialized bytes of a `size`-byte buffer, and the live request.
        let action = unsafe {
            swr_v1_resp_receive_range(
                data,
                length,
                size,
                maximum,
                request,
                &raw mut set_offset,
                &raw mut range,
            )
        };
        if range != 0 {
            set_errno(libc::ERANGE);
        }
        if set_offset != 0 {
            // SAFETY: as above.
            unsafe { (*buffer).offset = i64::try_from(length).unwrap_or(i64::MAX) };
        }
        match action {
            1 => {
                let grown = swr_v1_resp_extend_size(length, size, maximum, page);
                if grown != 0 {
                    // SAFETY: as the caller promises.
                    unsafe { (io.extend)(io.context, grown) };
                }
            }
            2 => return SW_OK,
            3 => {
                // SAFETY: as the caller promises.
                match unsafe { (io.dispatch)(io.context, length) } {
                    ..0 => return SW_ERR,
                    0 => {}
                    _ => return SW_OK,
                }
                // SAFETY: as above; `buffer->clear()`, then a fresh request.
                unsafe { (*buffer).length = 0 };
                // SAFETY: as above.
                unsafe { (*buffer).offset = 0 };
                // SAFETY: the live request.
                unsafe { request.write_bytes(0, 1) };
                return SW_OK;
            }
            4 => {
                log::warning(
                    ORIGIN,
                    format_args!("Package is too big. package_length={length}"),
                );
                return SW_ERR;
            }
            _ => {
                log::warning(ORIGIN, format_args!("redis protocol error"));
                return SW_ERR;
            }
        }
    }
}

/// `swoole::RecvData`.
#[repr(C)]
struct RecvData {
    info: DataHead,
    data: *const u8,
}

type OnPackage = unsafe extern "C" fn(*const Protocol, *mut SocketRecord, *const RecvData) -> c_int;

struct Connection {
    protocol: *const Protocol,
    socket: *mut SocketRecord,
    buffer: *mut CppString,
    fd: c_int,
}

unsafe extern "C" fn connection_recv(context: *mut c_void, at: *mut u8, n: usize) -> isize {
    // SAFETY: `recv_packet`'s context.
    let c = unsafe { &*context.cast::<Connection>() };
    // SAFETY: the connection's live socket and the buffer's free tail.
    let received = unsafe { swoole_rs_tls::upstream::socket_recv(c.socket, at.cast(), n, 0) };
    if received >= 0 {
        return received;
    }
    let error = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    // SAFETY: the connection's live socket.
    match unsafe { (*c.socket).catch_error(error) } {
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
    let buffer = unsafe { (*context.cast::<Connection>()).buffer };
    // SAFETY: its live String.
    unsafe { (*buffer).extend(size) };
}

unsafe extern "C" fn connection_dispatch(context: *mut c_void, length: usize) -> c_int {
    // SAFETY: `recv_packet`'s context.
    let c = unsafe { &*context.cast::<Connection>() };
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
    if unsafe { on_package(c.protocol, c.socket, &raw const data) } < 0 {
        return -1;
    }
    // SAFETY: the live socket.
    c_int::from(unsafe { (*c.socket).removed } != 0)
}

/// `int redis::recv_packet(Protocol *, Connection *, String *)`.
///
/// # Safety
/// `protocol`, `conn` and its socket, and `buffer` are live; `conn->object` is
/// null or this receive's request.
#[unsafe(export_name = "_ZN6swoole5redis11recv_packetEPNS_8ProtocolEPNS_10ConnectionEPNS_6StringE")]
pub unsafe extern "C" fn recv_packet(
    protocol: *const Protocol,
    conn: *mut ConnectionRecord,
    buffer: *mut CppString,
) -> c_int {
    // SAFETY: as the caller promises.
    let conn = unsafe { &mut *conn };
    let mut context = Connection {
        protocol,
        socket: conn.socket.cast(),
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
            &raw const io,
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
    use std::collections::VecDeque;

    struct Script {
        reads: VecDeque<Vec<u8>>,
        buffer: RespBuffer,
        packages: Vec<Vec<u8>>,
    }

    unsafe extern "C" fn script_recv(context: *mut c_void, at: *mut u8, n: usize) -> isize {
        let s = unsafe { &mut *context.cast::<Script>() };
        let Some(mut chunk) = s.reads.pop_front() else {
            return RESP_RECV_DONE;
        };
        if chunk.len() > n {
            s.reads.push_front(chunk.split_off(n));
        }
        unsafe { at.copy_from_nonoverlapping(chunk.as_ptr(), chunk.len()) };
        isize::try_from(chunk.len()).unwrap()
    }

    unsafe extern "C" fn script_extend(context: *mut c_void, size: usize) {
        let s = unsafe { &mut *context.cast::<Script>() };
        let grown = unsafe { libc::realloc(s.buffer.str.cast(), size) };
        s.buffer.str = grown.cast();
        s.buffer.size = size;
    }

    unsafe extern "C" fn script_dispatch(context: *mut c_void, length: usize) -> c_int {
        let s = unsafe { &mut *context.cast::<Script>() };
        let data = unsafe { std::slice::from_raw_parts(s.buffer.str, length) };
        s.packages.push(data.to_vec());
        0
    }

    fn run(reads: &[&[u8]], size: usize, maximum: u32) -> (Vec<c_int>, Script) {
        let mut script = Script {
            reads: reads.iter().map(|r| r.to_vec()).collect(),
            buffer: RespBuffer {
                length: 0,
                size,
                offset: 0,
                str: unsafe { libc::malloc(size) }.cast(),
            },
            packages: Vec::new(),
        };
        let mut request: *mut RespRequest = core::ptr::null_mut();
        let mut results = Vec::new();
        while !script.reads.is_empty() && results.last() != Some(&SW_ERR) {
            let io = RespIo {
                context: (&raw mut script).cast(),
                recv: script_recv,
                extend: script_extend,
                dispatch: script_dispatch,
            };
            let buffer = &raw mut script.buffer;
            results.push(unsafe {
                swr_v1_resp_recv(&raw const io, buffer, &raw mut request, maximum, 4096)
            });
        }
        unsafe { libc::free(request.cast()) };
        unsafe { libc::free(script.buffer.str.cast()) };
        (results, script)
    }

    #[test]
    fn a_command_split_across_reads_dispatches_whole() {
        // A read that ends mid-line reads on in the same call.
        let (results, script) = run(&[b"*2\r\n$3\r\nGE", b"T\r\n$1\r\nk\r\n"], 64, 1024);
        assert_eq!(results, [SW_OK]);
        assert_eq!(
            script.packages,
            [b"*2\r\n$3\r\nGET\r\n$1\r\nk\r\n".to_vec()]
        );
        assert_eq!((script.buffer.length, script.buffer.offset), (0, 0));
    }

    #[test]
    fn a_full_buffer_grows_and_an_oversized_command_fails() {
        let command = b"*1\r\n$40\r\n0123456789012345678901234567890123456789\r\n";
        let (results, script) = run(&[command], 16, 1024);
        assert_eq!(results, [SW_OK]);
        assert_eq!(script.packages, [command.to_vec()]);
        let (results, _) = run(&[command], 16, 32);
        assert_eq!(results, [SW_ERR]);
        let (results, _) = run(&[b"?bad\r\n"], 16, 32);
        assert_eq!(results, [SW_ERR]);
    }

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
            unsafe { recv_packet(&raw const protocol, &raw mut conn, &raw mut buffer) },
            SW_ERR
        );
        assert_eq!(buffer.length, 1);
        assert_eq!(buffer.size, buffer_size);

        unsafe { libc::free(conn.object) };
        buffer.dispose();
        unsafe { libc::close(pairs[1]) };
    }
}
