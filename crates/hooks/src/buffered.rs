//! `Socket::recv_with_buffer` and `Socket::recv_line` of the coroutine socket:
//! reads served from the socket's read buffer, refilled by one `recv`. Native code
//! keeps the buffer, which it lends as `BufferFields`, and the `recv`.
use crate::co_io::{available, host};
use crate::co_socket::Co;
use crate::packet::{BufferSync, ReadBuffer};
use core::mem::MaybeUninit;
use swoole_rs_corebase::sys::ReadSlot;
use swoole_rs_reactor::Events;

fn to_off(offset: usize) -> libc::off_t {
    libc::off_t::try_from(offset).unwrap_or(libc::off_t::MAX)
}

/// The socket's read String as this read borrows it: its allocation, of which the
/// first `filled` bytes are written and the rest may be uninitialized. Only this
/// module makes one, from the native view, and only [`Bytes::recv`] (or the tests,
/// which write what they count) grows `filled`.
struct Bytes<'a> {
    all: &'a mut [MaybeUninit<u8>],
    filled: usize,
}

impl Bytes<'_> {
    /// The written bytes.
    fn content(&self) -> &[u8] {
        let written = self.all.get(..self.filled).unwrap_or_default();
        // SAFETY: the first `filled` slots are written: the native view's length when
        // the read began, then only what `recv` reported writing.
        unsafe { written.assume_init_ref() }
    }

    /// Moves the written bytes from `at` to the start: `String::reduce`.
    fn drop_front(&mut self, at: usize) {
        let at = at.min(self.filled);
        self.all.copy_within(at..self.filled, 0);
        self.filled -= at;
    }

    /// The host's `recv` of `socket` into the free space, which then counts what it
    /// wrote.
    fn recv(&mut self, socket: Co) -> Option<usize> {
        let free = self.all.get_mut(self.filled..).unwrap_or_default();
        let received = (host().recv)(socket, free, false);
        if let Some(count) = received {
            self.filled = (self.filled + count).min(self.all.len());
        }
        received
    }
}

/// `recv_with_buffer` after the availability check, over the buffer `data`, read up
/// to `*offset`: up to `out.len()` bytes into `out`, refilling it with one `recv`
/// when it holds too few. `recv` gets the buffer's length and offset, to publish
/// before it may yield, and the buffer to receive into; it returns the bytes read,
/// 0 at EOF or None on error.
fn read_buffered<S: ReadSlot>(
    data: &mut Bytes<'_>,
    offset: &mut usize,
    out: &mut [S],
    recv: impl FnOnce(usize, usize, &mut Bytes<'_>) -> Option<usize>,
) -> Option<usize> {
    let mut at = (*offset).min(data.filled);
    let held = data.filled - at;
    let take = S::copy_in(out, data.content().get(at..).unwrap_or_default());
    at += take;
    if out.len() <= held {
        *offset = at;
        return Some(take);
    }
    // `String::reduce`: drop the consumed half.
    if at >= data.all.len() / 2 && at > 0 {
        data.drop_front(at);
        at = 0;
    }
    *offset = at;
    let received = recv(data.filled, at, data);
    if received <= Some(0) {
        return if held > 0 { Some(held) } else { received };
    }
    let copied = S::copy_in(
        out.get_mut(held..).unwrap_or_default(),
        data.content().get(at..).unwrap_or_default(),
    );
    *offset = at + copied;
    Some(held + copied)
}

/// `recv_line`: bytes up to and including the first `\n` or `\r`, at most
/// `out.len()`, one `read` (`recv_with_buffer`) of a byte at a time, each copied to
/// the next of `out`'s slots. None on error; at EOF, what was read.
pub(crate) fn recv_line<S: ReadSlot>(
    out: &mut [S],
    mut read: impl FnMut(&mut [u8]) -> Option<usize>,
) -> Option<usize> {
    let limit = out.len();
    let mut count = 0;
    let mut last = 0;
    // The byte at `count`: a NUL in the first slot until a read replaces it.
    let mut byte = [0];
    S::copy_in(out, &byte);
    while count < limit && !matches!(byte, [b'\n' | b'\r']) {
        if last > 0 {
            count += 1;
        }
        if count < limit {
            last = read(&mut byte)?;
            if last == 0 {
                return Some(count);
            }
            S::copy_in(out.get_mut(count..).unwrap_or_default(), &byte);
        }
    }
    if count < limit {
        count += 1;
    }
    Some(count)
}

struct Context {
    socket: Co,
    view: ReadBuffer,
}

impl Context {
    fn sync(&mut self, sync: BufferSync) {
        (host().read_buffer)(self.socket, &mut self.view, sync);
    }
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
    ctx.sync(BufferSync::Get);
    ctx
}

/// `Socket::recv_with_buffer`, including availability and lazy buffer allocation.
/// `out` cannot overlap the socket's buffer.
pub fn recv_buffered(socket: Co, out: &mut [impl ReadSlot]) -> Option<usize> {
    if !available(socket, Events::READ) {
        return None;
    }
    let mut ctx = buffer(socket);
    let all = if ctx.view.str.is_null() {
        &mut [][..]
    } else {
        // SAFETY: the socket's read String lends its `size` bytes to this read, the
        // socket's only reader, as slots (the room past its length is uninitialized);
        // `out` is outside them.
        unsafe { core::slice::from_raw_parts_mut(ctx.view.str.cast(), ctx.view.size) }
    };
    // The String's first `length` bytes are its written content.
    let filled = ctx.view.length.min(all.len());
    let mut data = Bytes { all, filled };
    let mut offset = usize::try_from(ctx.view.offset).unwrap_or(0);
    let result = read_buffered(&mut data, &mut offset, out, |length, offset, data| {
        ctx.view.length = length;
        ctx.view.offset = to_off(offset);
        ctx.sync(BufferSync::Store);
        data.recv(socket)
    });
    ctx.view.length = data.filled;
    ctx.view.offset = to_off(offset);
    ctx.sync(BufferSync::Store);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Source {
        data: Vec<u8>,
        calls: usize,
    }

    impl Source {
        fn recv(&mut self, into: &mut Bytes<'_>) -> usize {
            self.calls += 1;
            let at = &mut into.all[into.filled..];
            let n = at.len().min(self.data.len());
            at[..n].write_copy_of_slice(&self.data[..n]);
            into.filled += n;
            self.data.drain(..n);
            n
        }
    }

    struct Buffered {
        storage: Vec<MaybeUninit<u8>>,
        length: usize,
        offset: usize,
        source: Source,
    }

    impl Buffered {
        fn new(size: usize, data: &[u8]) -> Self {
            Self {
                storage: vec![MaybeUninit::new(0); size],
                length: 0,
                offset: 0,
                source: Source {
                    data: data.to_vec(),
                    calls: 0,
                },
            }
        }

        fn read(&mut self, n: usize) -> (Option<usize>, Vec<u8>) {
            let mut out = vec![0; n];
            let source = &mut self.source;
            let mut data = Bytes {
                all: &mut self.storage,
                filled: self.length,
            };
            let got = read_buffered(&mut data, &mut self.offset, &mut out, |_, _, data| {
                Some(source.recv(data))
            });
            self.length = data.filled;
            out.truncate(got.unwrap_or(0));
            (got, out)
        }
    }

    #[test]
    fn reads_are_served_from_the_buffer_and_refilled() {
        let mut b = Buffered::new(8, b"abcdefghij");
        assert_eq!(b.read(3), (Some(3), b"abc".to_vec()));
        assert_eq!((b.length, b.offset, b.source.calls), (8, 3, 1));
        assert_eq!(b.read(4), (Some(4), b"defg".to_vec()));
        // 1 byte held; the consumed half is dropped before the refill.
        assert_eq!(b.read(4), (Some(3), b"hij".to_vec()));
        assert_eq!((b.length, b.offset), (2, 2));
        assert_eq!(b.read(2), (Some(0), vec![]));
    }

    #[test]
    fn lines_end_at_a_newline_the_limit_or_eof() {
        let mut b = Buffered::new(16, b"ab\ncd");
        let line = |b: &mut Buffered, limit: usize| {
            let mut out = vec![0; limit];
            let n = recv_line(&mut out, |at| {
                let (got, bytes) = b.read(at.len());
                at[..bytes.len()].copy_from_slice(&bytes);
                got
            });
            out.truncate(n.unwrap_or(0));
            (n, out)
        };
        assert_eq!(line(&mut b, 10), (Some(3), b"ab\n".to_vec()));
        assert_eq!(line(&mut b, 1), (Some(1), b"c".to_vec()));
        assert_eq!(line(&mut b, 10), (Some(1), b"d".to_vec()));
        assert_eq!(line(&mut b, 10), (Some(0), vec![]));
    }
}
