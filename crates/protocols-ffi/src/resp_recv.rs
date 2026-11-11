//! `swoole::redis::recv_packet`: reads a connection until a whole RESP command is
//! buffered, then hands it to `onPackage`. [`swr_v1_resp_recv`] is the loop over
//! borrowed effects; the ports crate runs it over the server's own records.
use core::ffi::{c_int, c_void};
use swoole_rs_corebase::{log, sw_malloc};
use swoole_rs_protocols::{RespRequest, resp_extend_size, resp_receive_range};

pub(crate) const SW_OK: c_int = 0;
pub const SW_ERR: c_int = -1;
pub const ORIGIN: &core::ffi::CStr = c"redis::recv_packet";

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
    io: &RespIo,
    buffer: *mut RespBuffer,
    request: *mut *mut RespRequest,
    maximum: u32,
    page: usize,
) -> c_int {
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
        let mut range = false;
        // SAFETY: `length` initialized bytes of a `size`-byte buffer, read before any effect.
        let bytes = unsafe { crate::input(data, length) };
        // SAFETY: the live request, used by nothing else meanwhile.
        let state = unsafe { &mut *request };
        let (action, set_offset) = bytes.map_or((0, false), |bytes| {
            resp_receive_range(bytes, size, maximum, state, &mut range)
        });
        if range {
            set_errno(libc::ERANGE);
        }
        if set_offset {
            // SAFETY: as above.
            unsafe { (*buffer).offset = i64::try_from(length).unwrap_or(i64::MAX) };
        }
        match action {
            1 => {
                let grown = resp_extend_size(length, size, maximum, page);
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
