//! Buffered coroutine reads. The host lends written bytes only for each copy;
//! receives commit their initialized content through the native buffer owner.
use crate::co_io::{available, host};
use crate::co_socket::Co;
use crate::packet::{BufferSync, ReadBuffer};
use swoole_rs_corebase::sys::ReadSlot;
use swoole_rs_reactor::Events;

fn to_off(offset: usize) -> libc::off_t {
    libc::off_t::try_from(offset).unwrap_or(libc::off_t::MAX)
}

/// Operations over the stored bytes, each loan ending before another operation.
trait Buffer {
    fn length(&self) -> usize;
    fn capacity(&self) -> usize;
    fn copy<S: ReadSlot>(&mut self, at: usize, out: &mut [S]) -> usize;
    fn reduce(&mut self, at: usize);
    fn recv(&mut self, offset: usize) -> Option<usize>;
}

fn read_buffered<S: ReadSlot>(
    data: &mut impl Buffer,
    offset: &mut usize,
    out: &mut [S],
) -> Option<usize> {
    let mut at = (*offset).min(data.length());
    let held = data.length() - at;
    let take = data.copy(at, out);
    at += take;
    if out.len() <= held {
        *offset = at;
        return Some(take);
    }
    if at >= data.capacity() / 2 && at > 0 {
        data.reduce(at);
        at = 0;
    }
    *offset = at;
    let received = data.recv(at);
    if received <= Some(0) {
        return if held > 0 { Some(held) } else { received };
    }
    let copied = data.copy(at, out.get_mut(held..).unwrap_or_default());
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

impl Buffer for Context {
    fn length(&self) -> usize {
        self.view.length
    }
    fn capacity(&self) -> usize {
        self.view.size
    }
    fn copy<S: ReadSlot>(&mut self, at: usize, out: &mut [S]) -> usize {
        let mut copied = 0;
        (host().read_bytes)(self.socket, &mut |bytes| {
            copied = S::copy_in(out, bytes.get(at..).unwrap_or_default());
        });
        copied
    }
    fn reduce(&mut self, at: usize) {
        self.sync(BufferSync::Reduce(at));
    }
    fn recv(&mut self, offset: usize) -> Option<usize> {
        self.view.offset = to_off(offset);
        self.sync(BufferSync::Store);
        let received = (host().recv)(
            self.socket,
            self.view.length,
            self.view.size.saturating_sub(self.view.length),
            false,
        );
        if let Some(n) = received {
            self.view.length += n;
        }
        received
    }
}

/// `Socket::recv_with_buffer`, including availability and lazy buffer allocation.
/// `out` cannot overlap the socket's buffer.
pub fn recv_buffered(socket: Co, out: &mut [impl ReadSlot]) -> Option<usize> {
    if !available(socket, Events::READ) {
        return None;
    }
    let mut ctx = Context {
        socket,
        view: ReadBuffer {
            length: 0,
            size: 0,
            offset: 0,
        },
    };
    ctx.sync(BufferSync::Get);
    let mut offset = usize::try_from(ctx.view.offset).unwrap_or(0);
    let result = read_buffered(&mut ctx, &mut offset, out);
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
    struct Buffered {
        storage: Vec<u8>,
        length: usize,
        offset: usize,
        source: Source,
    }
    impl Buffer for Buffered {
        fn length(&self) -> usize {
            self.length
        }
        fn capacity(&self) -> usize {
            self.storage.len()
        }
        fn copy<S: ReadSlot>(&mut self, at: usize, out: &mut [S]) -> usize {
            S::copy_in(out, &self.storage[at..self.length])
        }
        fn reduce(&mut self, at: usize) {
            self.storage.copy_within(at..self.length, 0);
            self.length -= at;
        }
        fn recv(&mut self, _: usize) -> Option<usize> {
            self.source.calls += 1;
            let n = (self.storage.len() - self.length).min(self.source.data.len());
            self.storage[self.length..self.length + n].copy_from_slice(&self.source.data[..n]);
            self.length += n;
            self.source.data.drain(..n);
            Some(n)
        }
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
        fn read(&mut self, n: usize) -> (Option<usize>, Vec<u8>) {
            let mut out = vec![0; n];
            let mut offset = self.offset;
            let got = read_buffered(self, &mut offset, &mut out);
            self.offset = offset;
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
