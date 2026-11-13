//! `Socket::recv_with_buffer` and `Socket::recv_line` of the coroutine socket:
//! reads served from the socket's read buffer, refilled by one `recv`. Native code
//! keeps the buffer, which it lends as `BufferFields`, and the `recv`.
use crate::co_io::{available, host};
use crate::co_socket::Co;
use crate::packet::{BufferSync, ReadBuffer};
use crate::shutdown::READ;

fn to_off(offset: usize) -> libc::off_t {
    libc::off_t::try_from(offset).unwrap_or(libc::off_t::MAX)
}

/// Copies what fits of `from` to the start of `to`; the bytes copied.
fn copy(to: &mut [u8], from: &[u8]) -> usize {
    to.iter_mut()
        .zip(from)
        .map(|(to, from)| *to = *from)
        .count()
}

/// `recv_with_buffer` after the availability check, over the buffer `data`, which
/// holds `*length` bytes read up to `*offset`: up to `out.len()` bytes into `out`,
/// refilling it with one `recv` when it holds too few. `recv` gets the buffer's
/// length and offset, to publish before it may yield, and the free space to read
/// into; it returns the bytes read, 0 at EOF or -1 on error.
fn read_buffered(
    data: &mut [u8],
    length: &mut usize,
    offset: &mut usize,
    out: &mut [u8],
    recv: impl FnOnce(usize, usize, &mut [u8]) -> isize,
) -> isize {
    let mut filled = (*length).min(data.len());
    let mut at = (*offset).min(filled);
    let held = filled - at;
    let take = copy(out, data.get(at..filled).unwrap_or_default());
    at += take;
    if out.len() <= held {
        *offset = at;
        return take.cast_signed();
    }
    // `String::reduce`: drop the consumed half.
    if at >= data.len() / 2 && at > 0 {
        filled -= at;
        data.copy_within(at..at + filled, 0);
        at = 0;
    }
    *length = filled;
    *offset = at;
    let received = recv(filled, at, data.get_mut(filled..).unwrap_or_default());
    if received <= 0 {
        return if held > 0 {
            held.cast_signed()
        } else {
            received
        };
    }
    filled = (filled + received.unsigned_abs()).min(data.len());
    let copied = copy(
        out.get_mut(held..).unwrap_or_default(),
        data.get(at..filled).unwrap_or_default(),
    );
    *length = filled;
    *offset = at + copied;
    (held + copied).cast_signed()
}

/// `recv_line`: bytes up to and including the first `\n` or `\r`, at most
/// `out.len()`, one `read` (`recv_with_buffer`) of a byte at a time. -1 on error;
/// at EOF, what was read.
pub(crate) fn recv_line(out: &mut [u8], mut read: impl FnMut(&mut [u8]) -> isize) -> isize {
    let limit = out.len();
    let mut count = 0;
    let mut last = 0;
    if let Some(first) = out.first_mut() {
        *first = 0;
    }
    while count < limit && !matches!(out.get(count), Some(b'\n' | b'\r')) {
        if last > 0 {
            count += 1;
        }
        if count < limit {
            last = read(out.get_mut(count..=count).unwrap_or_default());
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
pub fn recv_buffered(socket: Co, out: &mut [u8]) -> isize {
    if !available(socket, READ) {
        return -1;
    }
    let mut ctx = buffer(socket);
    let data = if ctx.view.str.is_null() {
        &mut [][..]
    } else {
        // SAFETY: the socket's read String lends its `size` bytes to this read, the
        // socket's only reader; `out` is outside them.
        unsafe { core::slice::from_raw_parts_mut(ctx.view.str, ctx.view.size) }
    };
    let mut length = ctx.view.length;
    let mut offset = usize::try_from(ctx.view.offset).unwrap_or(0);
    let result = read_buffered(
        data,
        &mut length,
        &mut offset,
        out,
        |length, offset, free| {
            ctx.view.length = length;
            ctx.view.offset = to_off(offset);
            ctx.sync(BufferSync::Store);
            (host().recv)(socket, free, false)
        },
    );
    ctx.view.length = length;
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
        fn recv(&mut self, at: &mut [u8]) -> isize {
            self.calls += 1;
            let n = at.len().min(self.data.len());
            at[..n].copy_from_slice(&self.data[..n]);
            self.data.drain(..n);
            n.cast_signed()
        }
    }

    struct Buffered {
        storage: Vec<u8>,
        length: usize,
        offset: usize,
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
            let mut out = vec![0; n];
            let source = &mut self.source;
            let got = read_buffered(
                &mut self.storage,
                &mut self.length,
                &mut self.offset,
                &mut out,
                |_, _, free| source.recv(free),
            );
            out.truncate(got.max(0).unsigned_abs());
            (got, out)
        }
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
            let mut out = vec![0; limit];
            let n = recv_line(&mut out, |at| {
                let (got, bytes) = b.read(at.len());
                at[..bytes.len()].copy_from_slice(&bytes);
                got
            });
            out.truncate(n.max(0).unsigned_abs());
            (n, out)
        };
        assert_eq!(line(&mut b, 10), (3, b"ab\n".to_vec()));
        assert_eq!(line(&mut b, 1), (1, b"c".to_vec()));
        assert_eq!(line(&mut b, 10), (1, b"d".to_vec()));
        assert_eq!(line(&mut b, 10), (0, vec![]));
    }
}
