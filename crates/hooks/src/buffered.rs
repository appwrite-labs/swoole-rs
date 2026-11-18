//! Buffered coroutine reads. The host lends written bytes only for each copy;
//! receives commit their initialized content through the native buffer owner.
use crate::co_io::{available, host};
use crate::co_socket::Co;
use crate::packet::{BufferSync, ReadBuffer};
use swoole_rs_corebase::lent::{Spare, lend_spare};
use swoole_rs_corebase::reactor::Events;

fn to_off(offset: usize) -> libc::off_t {
    libc::off_t::try_from(offset).unwrap_or(libc::off_t::MAX)
}

/// Operations over the stored bytes, each loan ending before another operation.
trait Buffer {
    fn length(&self) -> usize;
    fn capacity(&self) -> usize;
    /// Copies what fits of the bytes from `at` into `out`'s room: the count.
    fn copy(&mut self, at: usize, out: &mut Spare<'_>) -> usize;
    fn reduce(&mut self, at: usize);
    fn recv(&mut self, offset: usize) -> Option<usize>;
}

/// What fits of `bytes` into `out`'s room: the count.
fn put(out: &mut Spare<'_>, bytes: &[u8]) -> usize {
    let n = bytes.len().min(out.room());
    out.put(bytes.get(..n).unwrap_or_default());
    n
}

fn read_buffered(data: &mut impl Buffer, offset: &mut usize, out: &mut Spare<'_>) -> Option<usize> {
    let mut at = (*offset).min(data.length());
    let held = data.length() - at;
    let wanted = out.room();
    let take = data.copy(at, out);
    at += take;
    if wanted <= held {
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
    let copied = data.copy(at, out);
    *offset = at + copied;
    Some(held + copied)
}

/// `recv_line`: bytes up to and including the first `\n` or `\r`, at most
/// `out.room()`, one `read` (`recv_with_buffer`) of a byte at a time, each appended
/// to `out`. None on error; at EOF, what was read.
pub(crate) fn recv_line(
    out: &mut Spare<'_>,
    mut read: impl FnMut(&mut Spare<'_>) -> Option<usize>,
) -> Option<usize> {
    let mut count = 0;
    let mut byte = [0];
    while out.room() > 0 {
        let (got, wrote) = lend_spare(&mut byte, &mut read);
        if got? == 0 || wrote == 0 {
            break;
        }
        out.put(&byte);
        count += 1;
        if matches!(byte, [b'\n' | b'\r']) {
            break;
        }
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
    fn copy(&mut self, at: usize, out: &mut Spare<'_>) -> usize {
        let mut copied = 0;
        (host().read_bytes)(self.socket, &mut |bytes| {
            copied = put(out, bytes.get(at..).unwrap_or_default());
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
pub fn recv_buffered(socket: Co, out: &mut Spare<'_>) -> Option<usize> {
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
        fn copy(&mut self, at: usize, out: &mut Spare<'_>) -> usize {
            put(out, &self.storage[at..self.length])
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
            let (got, wrote) = lend_spare(&mut out, |out| read_buffered(self, &mut offset, out));
            self.offset = offset;
            assert_eq!(wrote, got.unwrap_or(0));
            out.truncate(wrote);
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
            let read = |at: &mut Spare<'_>| {
                let (got, bytes) = b.read(at.room());
                at.put(&bytes);
                got
            };
            let (n, wrote) = lend_spare(&mut out, |out| recv_line(out, read));
            assert_eq!(wrote, n.unwrap_or(0));
            out.truncate(wrote);
            (n, out)
        };
        assert_eq!(line(&mut b, 10), (Some(3), b"ab\n".to_vec()));
        assert_eq!(line(&mut b, 1), (Some(1), b"c".to_vec()));
        assert_eq!(line(&mut b, 10), (Some(1), b"d".to_vec()));
        assert_eq!(line(&mut b, 10), (Some(0), vec![]));
    }
}
