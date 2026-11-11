//! `Socket::recv_with_buffer` and `Socket::recv_line` of the coroutine socket:
//! reads served from the socket's read buffer, refilled by one `recv`. Native code
//! keeps the buffer, which it lends as `BufferFields`, and the `recv`.
use crate::co_io::{available, host};
use crate::co_socket::Co;
use crate::packet::ReadBuffer;
use crate::shutdown::READ;
use core::ffi::c_void;

/// The fields of the socket's read buffer (`swoole::String`).
#[repr(C)]
pub struct BufferFields {
    pub str: *mut u8,
    pub size: usize,
    pub length: *mut usize,
    pub offset: *mut libc::off_t,
}

fn to_off(offset: usize) -> libc::off_t {
    libc::off_t::try_from(offset).unwrap_or(libc::off_t::MAX)
}

/// A read of `length` bytes into `buffer`: bytes read, 0 at EOF, -1 on error.
pub type Read = unsafe extern "C" fn(*mut c_void, *mut u8, usize) -> isize;

/// `recv_with_buffer` after the availability check: up to `length` bytes into
/// `out` from the buffer, refilling it with one `recv` when it holds too few.
///
/// # Safety
/// `buffer` describes a live buffer that only this call touches, except through
/// `recv`, which reads into its free space; `out` holds `length` bytes.
pub unsafe extern "C" fn swr_v1_hook_recv_buffered(
    buffer: &BufferFields,
    recv: Read,
    context: *mut c_void,
    out: *mut u8,
    length: usize,
) -> isize {
    // SAFETY: the buffer's live field.
    let mut filled = unsafe { *buffer.length };
    // SAFETY: the buffer's live field.
    let mut offset = usize::try_from(unsafe { *buffer.offset }).unwrap_or(0);
    let held = filled - offset;
    let take = length.min(held);
    // SAFETY: `take` bytes of the buffer's data into `out`, which holds `length`.
    unsafe { core::ptr::copy_nonoverlapping(buffer.str.wrapping_add(offset), out, take) };
    offset += take;
    if length <= held {
        // SAFETY: the buffer's live field.
        unsafe { *buffer.offset = to_off(offset) };
        return take.cast_signed();
    }
    // `String::reduce`: drop the consumed half.
    if offset >= buffer.size / 2 && offset > 0 {
        filled -= offset;
        // SAFETY: moves the unread bytes to the start of the same live allocation.
        unsafe { core::ptr::copy(buffer.str.wrapping_add(offset), buffer.str, filled) };
        offset = 0;
    }
    // SAFETY: the buffer's live fields, written before `recv` may yield.
    unsafe { *buffer.length = filled };
    // SAFETY: the buffer's live field.
    unsafe { *buffer.offset = to_off(offset) };
    // SAFETY: `recv` reads into the buffer's free space.
    let received = unsafe {
        recv(
            context,
            buffer.str.wrapping_add(filled),
            buffer.size - filled,
        )
    };
    if received <= 0 {
        return if held > 0 {
            held.cast_signed()
        } else {
            received
        };
    }
    filled += received.unsigned_abs();
    let copied = (length - held).min(filled - offset);
    let (from, to) = (buffer.str.wrapping_add(offset), out.wrapping_add(held));
    // SAFETY: `copied` fresh bytes into the rest of `out`.
    unsafe { core::ptr::copy_nonoverlapping(from, to, copied) };
    // SAFETY: the buffer's live field.
    unsafe { *buffer.length = filled };
    // SAFETY: the buffer's live field.
    unsafe { *buffer.offset = to_off(offset + copied) };
    (held + copied).cast_signed()
}

/// `recv_line`: bytes up to and including the first `\n` or `\r`, at most `limit`,
/// one `read` (`recv_with_buffer`) at a time. -1 on error; at EOF, what was read.
///
/// # Safety
/// `out` holds `limit` bytes and one more.
pub unsafe extern "C" fn swr_v1_hook_recv_line(
    read: Read,
    context: *mut c_void,
    out: *mut u8,
    limit: usize,
) -> isize {
    let mut count = 0;
    let mut last = 0;
    // SAFETY: `out` holds at least one byte.
    unsafe { *out = 0 };
    // SAFETY: `count` < `limit` here, so the byte is inside `out`.
    while count < limit && !matches!(unsafe { *out.wrapping_add(count) }, b'\n' | b'\r') {
        if last > 0 {
            count += 1;
        }
        if count < limit {
            // SAFETY: one byte at `count` < `limit`, inside `out`.
            last = unsafe { read(context, out.wrapping_add(count), 1) };
            if last < 0 {
                return -1;
            }
            if last == 0 {
                return count.cast_signed();
            }
        }
    }
    if count < limit {
        count += 1;
    }
    count.cast_signed()
}

struct Context {
    socket: Co,
    view: ReadBuffer,
}

impl Context {
    fn sync(&mut self, action: u32, size: usize) {
        (host().read_buffer)(self.socket, &mut self.view, action, size);
    }
}

unsafe fn context<'a>(at: *mut c_void) -> &'a mut Context {
    // SAFETY: the buffered read lends its live Context to every callback.
    unsafe { &mut *at.cast::<Context>() }
}

unsafe extern "C" fn record_recv(at: *mut c_void, buffer: *mut u8, size: usize) -> isize {
    // SAFETY: the buffered read's live context.
    let ctx = unsafe { context(at) };
    ctx.sync(1, 0);
    // SAFETY: the buffered read lends `size` free bytes of the buffer at `buffer`.
    let buffer = unsafe { core::slice::from_raw_parts_mut(buffer, size) };
    (host().recv)(ctx.socket, buffer, false)
}

fn buffer(socket: Co) -> Context {
    let mut ctx = Context {
        socket,
        view: ReadBuffer {
            length: 0,
            size: 0,
            offset: 0,
            str: core::ptr::null_mut(),
        },
    };
    ctx.sync(0, 0);
    ctx
}

/// `Socket::recv_with_buffer`, including availability and lazy buffer allocation.
///
/// # Safety
/// `out` holds `size` writable bytes and cannot overlap the socket's buffer.
pub unsafe fn recv_buffered(socket: Co, out: *mut u8, size: usize) -> isize {
    if !available(socket, READ) {
        return -1;
    }
    let mut ctx = buffer(socket);
    let view = BufferFields {
        str: ctx.view.str,
        size: ctx.view.size,
        length: &raw mut ctx.view.length,
        offset: &raw mut ctx.view.offset,
    };
    // SAFETY: the exclusive buffer fields and output remain live for the receive.
    let result =
        unsafe { swr_v1_hook_recv_buffered(&view, record_recv, (&raw mut ctx).cast(), out, size) };
    ctx.sync(1, 0);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Source {
        data: Vec<u8>,
        calls: usize,
    }

    unsafe extern "C" fn recv(c: *mut c_void, at: *mut u8, room: usize) -> isize {
        let source = unsafe { &mut *c.cast::<Source>() };
        source.calls += 1;
        let n = room.min(source.data.len());
        unsafe { core::ptr::copy_nonoverlapping(source.data.as_ptr(), at, n) };
        source.data.drain(..n);
        n.cast_signed()
    }

    struct Buffered {
        storage: Vec<u8>,
        length: usize,
        offset: libc::off_t,
        source: Source,
    }

    impl Buffered {
        fn new(size: usize, data: &[u8]) -> Self {
            Self {
                storage: vec![0; size],
                length: 0,
                offset: 0,
                source: Source {
                    data: data.to_vec(),
                    calls: 0,
                },
            }
        }

        fn read(&mut self, n: usize) -> (isize, Vec<u8>) {
            let view = BufferFields {
                str: self.storage.as_mut_ptr(),
                size: self.storage.len(),
                length: &raw mut self.length,
                offset: &raw mut self.offset,
            };
            let mut out = vec![0; n];
            let got = unsafe {
                swr_v1_hook_recv_buffered(
                    &view,
                    recv,
                    (&raw mut self.source).cast(),
                    out.as_mut_ptr(),
                    n,
                )
            };
            out.truncate(got.max(0).unsigned_abs());
            (got, out)
        }
    }

    unsafe extern "C" fn buffered(c: *mut c_void, at: *mut u8, n: usize) -> isize {
        let this = unsafe { &mut *c.cast::<Buffered>() };
        let (got, bytes) = this.read(n);
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), at, bytes.len()) };
        got
    }

    #[test]
    fn reads_are_served_from_the_buffer_and_refilled() {
        let mut b = Buffered::new(8, b"abcdefghij");
        assert_eq!(b.read(3), (3, b"abc".to_vec()));
        assert_eq!((b.length, b.offset, b.source.calls), (8, 3, 1));
        assert_eq!(b.read(4), (4, b"defg".to_vec()));
        // 1 byte held; the consumed half is dropped before the refill.
        assert_eq!(b.read(4), (3, b"hij".to_vec()));
        assert_eq!((b.length, b.offset), (2, 2));
        assert_eq!(b.read(2), (0, vec![]));
    }

    #[test]
    fn lines_end_at_a_newline_the_limit_or_eof() {
        let mut b = Buffered::new(16, b"ab\ncd");
        let line = |b: &mut Buffered, limit: usize| {
            let mut out = vec![0; limit + 1];
            let n = unsafe {
                swr_v1_hook_recv_line(buffered, (&raw mut *b).cast(), out.as_mut_ptr(), limit)
            };
            out.truncate(n.max(0).unsigned_abs());
            (n, out)
        };
        assert_eq!(line(&mut b, 10), (3, b"ab\n".to_vec()));
        assert_eq!(line(&mut b, 1), (1, b"c".to_vec()));
        assert_eq!(line(&mut b, 10), (1, b"d".to_vec()));
        assert_eq!(line(&mut b, 10), (0, vec![]));
    }
}
