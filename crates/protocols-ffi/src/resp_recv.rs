//! `swoole::redis::recv_packet`: reads a connection until a whole RESP command is
//! buffered, then hands it to `onPackage`. [`recv_packet`] is the loop over a
//! [`RespConnection`]; the ports crate runs it over the server's own records, and
//! [`swr_v1_resp_recv`] over C's effects.
use core::ffi::{c_int, c_void};
use swoole_rs_corebase::{log, set_errno, sw_malloc};
pub use swoole_rs_protocols::RespRequest;
use swoole_rs_protocols::{resp_extend_size, resp_receive_range};

pub(crate) const SW_OK: c_int = 0;
pub const SW_ERR: c_int = -1;
pub const ORIGIN: &core::ffi::CStr = c"redis::recv_packet";

/// [`RespConnection::recv`]: the connection closes (`SW_ERR`).
pub const RESP_RECV_CLOSE: isize = -1;
/// [`RespConnection::recv`]: the read ends with `SW_OK` (it would block, or a logged error).
pub const RESP_RECV_DONE: isize = -2;

/// The socket, receive buffer and request state one `recv_packet` works over. Each
/// method borrows them only for its own call, since `dispatch` may free them.
pub trait RespConnection {
    /// `socket->recv` into the buffer's free tail, with upstream's read-error
    /// handling and warning: the byte count, which joins the content, 0 or
    /// [`RESP_RECV_CLOSE`] when the connection closes, or [`RESP_RECV_DONE`].
    fn recv(&mut self) -> isize;
    /// The buffer's content, its allocation size, and the request state.
    fn view(&mut self) -> (&[u8], usize, &mut RespRequest);
    /// `buffer->offset = offset`.
    fn set_offset(&mut self, offset: i64);
    /// `buffer->extend(size)`.
    fn extend(&mut self, size: usize);
    /// `protocol->onPackage` of the whole content: negative when it fails, 1 when
    /// the socket was removed meanwhile, else 0.
    fn dispatch(&mut self) -> c_int;
    /// `buffer->clear()` and a fresh request state, after a dispatch.
    fn clear(&mut self);
}

/// `redis::recv_packet`'s loop: receives until the request state sees a whole
/// command or must wait, growing the buffer up to `maximum`. `page` is
/// `swoole_pagesize()`.
pub fn recv_packet(c: &mut impl RespConnection, maximum: u32, page: usize) -> c_int {
    let maximum = maximum as usize;
    loop {
        match c.recv() {
            0 | RESP_RECV_CLOSE => return SW_ERR,
            ..0 => return SW_OK,
            _ => {}
        }
        let mut range = false;
        let (length, size, (action, set_offset)) = {
            let (bytes, size, state) = c.view();
            let decision = resp_receive_range(bytes, size, maximum, state, &mut range);
            (bytes.len(), size, decision)
        };
        if range {
            set_errno(libc::ERANGE);
        }
        if set_offset {
            c.set_offset(i64::try_from(length).unwrap_or(i64::MAX));
        }
        match action {
            1 => {
                let grown = resp_extend_size(length, size, maximum, page);
                if grown != 0 {
                    c.extend(grown);
                }
            }
            2 => return SW_OK,
            3 => {
                match c.dispatch() {
                    ..0 => return SW_ERR,
                    0 => {}
                    _ => return SW_OK,
                }
                c.clear();
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

/// The request state in `slot`, allocated zeroed with `sw_malloc` when null; null
/// (after a warning) when that fails.
pub fn request(slot: &mut *mut RespRequest) -> *mut RespRequest {
    if slot.is_null() {
        let fresh = sw_malloc(size_of::<RespRequest>()).cast::<RespRequest>();
        if fresh.is_null() {
            log::warning(
                ORIGIN,
                format_args!("malloc({}) failed", size_of::<RespRequest>()),
            );
            return fresh;
        }
        // SAFETY: a fresh allocation of one RespRequest.
        unsafe { fresh.write(RespRequest::default()) };
        *slot = fresh;
    }
    *slot
}

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
    /// [`RespConnection::recv`] of at most `n` bytes into `at`.
    pub recv: unsafe extern "C" fn(context: *mut c_void, at: *mut u8, n: usize) -> isize,
    /// `buffer->extend(size)`.
    pub extend: unsafe extern "C" fn(context: *mut c_void, size: usize),
    /// `protocol->onPackage` of the buffer's first `length` bytes: negative when it
    /// fails, 1 when the socket was removed meanwhile, else 0.
    pub dispatch: unsafe extern "C" fn(context: *mut c_void, length: usize) -> c_int,
}

/// [`swr_v1_resp_recv`]'s connection: C's effects over its String head and request.
struct Effects<'a> {
    io: &'a RespIo,
    buffer: *mut RespBuffer,
    request: *mut RespRequest,
}

impl Effects<'_> {
    fn head(&mut self) -> &mut RespBuffer {
        // SAFETY: the caller's live String head, which only `extend` reallocates and
        // this borrow is not held across.
        unsafe { &mut *self.buffer }
    }

    fn state(&mut self) -> &mut RespRequest {
        // SAFETY: the live request, used by nothing else between effects.
        unsafe { &mut *self.request }
    }
}

impl RespConnection for Effects<'_> {
    fn recv(&mut self) -> isize {
        let RespBuffer {
            length, size, str, ..
        } = *self.head();
        // SAFETY: the free tail of the buffer, `size - length` writable bytes.
        let received =
            unsafe { (self.io.recv)(self.io.context, str.wrapping_add(length), size - length) };
        self.head().length = length + received.max(0).cast_unsigned();
        received
    }

    fn view(&mut self) -> (&[u8], usize, &mut RespRequest) {
        let RespBuffer {
            length, size, str, ..
        } = *self.head();
        // SAFETY: `length` initialized bytes of a `size`-byte buffer, read before any effect.
        let bytes = unsafe { crate::input(str, length) }.unwrap_or_default();
        (bytes, size, self.state())
    }

    fn set_offset(&mut self, offset: i64) {
        self.head().offset = offset;
    }

    fn extend(&mut self, size: usize) {
        // SAFETY: as the caller promises.
        unsafe { (self.io.extend)(self.io.context, size) };
    }

    fn dispatch(&mut self) -> c_int {
        let length = self.head().length;
        // SAFETY: as the caller promises.
        unsafe { (self.io.dispatch)(self.io.context, length) }
    }

    fn clear(&mut self) {
        let head = self.head();
        head.length = 0;
        head.offset = 0;
        *self.state() = RespRequest::default();
    }
}

/// [`recv_packet`] over `io`'s effects into `buffer`, with the request state in
/// `*request` (allocated with `sw_malloc` when null).
///
/// # Safety
/// `io`'s callbacks are valid for its context; `buffer` is that context's live
/// String head, which only `extend` reallocates; `request` points to null or to a
/// live `swr_resp_request` from `sw_malloc`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_resp_recv(
    io: &RespIo,
    buffer: *mut RespBuffer,
    request: *mut *mut RespRequest,
    maximum: u32,
    page: usize,
) -> c_int {
    // SAFETY: as the caller promises.
    let request = self::request(unsafe { &mut *request });
    if request.is_null() {
        return SW_ERR;
    }
    recv_packet(
        &mut Effects {
            io,
            buffer,
            request,
        },
        maximum,
        page,
    )
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::disallowed_types,
        reason = "a scripted socket's reads; tools/budget.py doesn't count test code"
    )]
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
            results.push(unsafe { swr_v1_resp_recv(&io, buffer, &raw mut request, maximum, 4096) });
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
}
