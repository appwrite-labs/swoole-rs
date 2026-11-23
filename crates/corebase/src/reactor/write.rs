//! The reactor's buffered write (upstream `Reactor::write_func`, behind `_write` and
//! `_writev`) over the socket's write-path fields, which `network::Socket` embeds as
//! one `SocketOutput` block. Native code performs the four effects that need its
//! objects: making the socket nonblocking, the send, creating the `Buffer`, and
//! registering write interest. It logs and raises from the returned outcome.
use crate::CCast;
use crate::buffer::EntryKind;
use crate::buffer::cpp::{
    OutputQueue, dispose_queue, new_buffer, swr_cpp_buffer_count, swr_cpp_buffer_length,
    swr_cpp_buffer_typed_close, swr_cpp_buffer_typed_front,
};
use crate::reactor::{Drain, Socket, SocketFields};
use std::io::IoSlice;

/// `network::Socket`'s write-path fields, in the order and types it embeds them.
/// Outside the reactor they are reached through the socket's output operations below.
pub struct SocketOutput {
    /// `Buffer *out_buffer`: the `Buffer` the socket owns, a null pointer without one.
    pub(super) out_buffer: Option<Box<OutputQueue>>,
    /// `TimerNode *send_timer`, held by id.
    pub(super) send_timer: Option<crate::timer_key::Held>,
    pub(super) buffer_size: u32,
    pub(super) close_wait: bool,
    pub(super) nonblock: bool,
    pub(super) ssl_send: bool,
    pub(super) send_wait: bool,
}

/// What a send loop does after a send that could not proceed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendStall {
    /// The socket is to close (`close_wait`).
    Close,
    /// The send would block: wait for writability (`send_wait`).
    Wait,
}

impl SocketOutput {
    /// `out_buffer`, or `None` without one.
    #[must_use]
    pub(super) fn buffer(&self) -> Option<&OutputQueue> {
        self.out_buffer.as_deref()
    }
}

/// The socket's queued output, as its callers use it.
impl SocketFields {
    /// `if (!out_buffer) out_buffer = new Buffer(chunk_size)`: the queue later output
    /// goes behind.
    #[inline]
    pub fn open_output_queue(&mut self, chunk_size: u32) {
        if self.output.out_buffer.is_none() {
            self.output.out_buffer = Some(new_buffer(chunk_size));
        }
    }

    /// Whether the socket has an output queue, empty or not: a send that may bypass the
    /// queue can't once it exists.
    #[inline]
    #[must_use]
    pub fn has_output_queue(&self) -> bool {
        self.output.out_buffer.is_some()
    }

    /// `Buffer::empty(out_buffer)`: nothing is queued, with or without a queue.
    #[inline]
    #[must_use]
    pub fn output_drained(&self) -> bool {
        self.output
            .buffer()
            .is_none_or(|buffer| swr_cpp_buffer_count(buffer) == 0)
    }

    /// `get_out_buffer_length()`: the bytes queued, 0 without a queue.
    #[inline]
    #[must_use]
    pub fn queued_bytes(&self) -> u32 {
        self.output.buffer().map_or(0, swr_cpp_buffer_length)
    }

    /// The kind of the next queued entry (`typed_front().kind`), None when nothing is
    /// queued.
    #[inline]
    #[must_use]
    pub fn next_output(&self) -> Option<EntryKind> {
        self.output
            .buffer()
            .filter(|&buffer| swr_cpp_buffer_count(buffer) != 0)
            .map(|buffer| swr_cpp_buffer_typed_front(buffer).kind)
    }

    /// Queues `data` behind the output, when the socket has a queue.
    #[inline]
    pub fn queue_output(&self, data: &[u8]) {
        if let Some(buffer) = self.output.buffer() {
            buffer.append(data);
        }
    }

    /// `typed_close()`: queues a close behind the output, when the socket has a queue.
    #[inline]
    pub fn queue_close(&self) {
        if let Some(buffer) = self.output.buffer() {
            swr_cpp_buffer_typed_close(buffer);
        }
    }

    /// The output queue, out of the socket, for the caller to free outside the record's
    /// borrow: an entry's disposer runs as it goes.
    #[must_use]
    pub fn take_output_queue(&mut self) -> Option<Box<OutputQueue>> {
        self.output.out_buffer.take()
    }

    /// `close_wait = true`: the socket closes once whatever still uses it lets go, and
    /// nothing more is sent or awaited on it meanwhile.
    #[inline]
    pub fn defer_close(&mut self) {
        self.output.close_wait = true;
    }

    /// Whether the socket is waiting to close (`close_wait`).
    #[inline]
    #[must_use]
    pub fn close_deferred(&self) -> bool {
        self.output.close_wait
    }

    /// Why a send that could not proceed stops the send loop: the socket is to close,
    /// or waits for writability; None to go on with the next entry.
    #[inline]
    #[must_use]
    pub fn send_stall(&self) -> Option<SendStall> {
        if self.output.close_wait {
            Some(SendStall::Close)
        } else if self.output.send_wait {
            Some(SendStall::Wait)
        } else {
            None
        }
    }

    /// `buffer_size = bytes`: the most output the socket queues (`u32::MAX` for none);
    /// 0 takes the reactor's default on the first write.
    #[inline]
    pub fn limit_output(&mut self, bytes: u32) {
        self.output.buffer_size = bytes;
    }

    /// The most output the socket queues (`buffer_size`).
    #[inline]
    #[must_use]
    pub fn output_limit(&self) -> u32 {
        self.output.buffer_size
    }

    /// Whether the queued output has reached the socket's limit.
    #[inline]
    #[must_use]
    pub fn output_full(&self) -> bool {
        self.queued_bytes() >= self.output.buffer_size
    }

    /// Records that the caller left the descriptor nonblocking (or not), so writes
    /// don't set it again (`nonblock`).
    #[inline]
    pub fn note_nonblocking(&mut self, nonblock: bool) {
        self.output.nonblock = nonblock;
    }

    /// Whether the socket knows its descriptor is nonblocking.
    #[inline]
    #[must_use]
    pub fn is_nonblocking(&self) -> bool {
        self.output.nonblock
    }

    /// `ssl_send = true`: writes go through the TLS session, never straight to the
    /// descriptor.
    #[inline]
    pub fn send_through_tls(&mut self) {
        self.output.ssl_send = true;
    }

    /// Whether writes go through the TLS session (`ssl_send`).
    #[inline]
    #[must_use]
    pub fn sends_through_tls(&self) -> bool {
        self.output.ssl_send
    }

    /// The idle timer watching the socket's sends (`send_timer`), if one is armed.
    #[inline]
    #[must_use]
    pub fn send_timer(&self) -> Option<crate::timer_key::Held> {
        self.output.send_timer
    }

    /// Arms `timer` as the idle timer watching the socket's sends.
    #[inline]
    pub fn arm_send_timer(&mut self, timer: crate::timer_key::Held) {
        self.output.send_timer = Some(timer);
    }

    /// The send idle timer, disarmed, for the caller to delete.
    #[inline]
    #[must_use]
    pub fn take_send_timer(&mut self) -> Option<crate::timer_key::Held> {
        self.output.send_timer.take()
    }

    /// The queue, out of the output buffer until [`Self::return_output_queue`]; None
    /// without a buffer, or while the queue is already lent.
    #[inline]
    fn lend_output_queue(&self) -> Option<Box<crate::buffer::Buffer>> {
        self.output.buffer().and_then(OutputQueue::take_queue)
    }

    /// Gives back a lent queue, or hands it back to be freed when the socket lost its
    /// buffer meanwhile.
    #[inline]
    fn return_output_queue(
        &self,
        queue: Box<crate::buffer::Buffer>,
    ) -> Option<Box<crate::buffer::Buffer>> {
        match self.output.buffer() {
            Some(buffer) => {
                buffer.put_queue(queue);
                None
            }
            None => Some(queue),
        }
    }

    /// Queues the bytes of `payload` past `offset` behind the output; the buffer must
    /// exist (`new Buffer` throwing is fatal upstream, which nothing catches).
    #[inline]
    fn queue_payload(&self, payload: WritePayload<'_>, offset: u64) {
        let Some(buffer) = self.output.buffer() else {
            crate::fatal("a socket's output buffer can't be created");
        };
        match payload {
            WritePayload::Bytes(data) => buffer.append_from(&[data.bytes()], offset),
            WritePayload::Vectors(vectors) => buffer.append_from(vectors, offset),
        }
    }

    /// Records why a send stopped, as its result asks: `close_wait` (the socket is to
    /// close) and `send_wait` (it waits for writability). It writes only those two
    /// flags of the output block, so pointers to the socket's other fields, such as the
    /// hooks' `kernel_nobufs`, stay usable.
    #[inline]
    pub const fn note_send_stall(&mut self, close_wait: bool, send_wait: bool) {
        if close_wait {
            self.output.close_wait = true;
        }
        if send_wait {
            self.output.send_wait = true;
        }
    }

    /// A send went out: restarts the idle clock when an idle timer watches sends.
    #[inline]
    pub fn note_sent(&mut self) {
        if self.output.send_timer.is_some() {
            self.last_sent_time = crate::reactor::steady_ms();
        }
    }
}

impl Socket<'_> {
    /// `f` over the socket's output queue (None without one, or while it is lent),
    /// taken out of its buffer meanwhile and put back after, or freed if the socket lost
    /// its buffer meanwhile. `f` may use the socket, but not reach its queue through it.
    pub fn with_output_queue<R>(
        self,
        f: impl FnOnce(Option<&mut crate::buffer::Buffer>) -> R,
    ) -> R {
        let queue = self.read(SocketFields::lend_output_queue);
        let Some(mut queue) = queue else {
            return f(None);
        };
        let result = f(Some(&mut queue));
        if let Some(queue) = self.read(|record| record.return_output_queue(queue)) {
            dispose_queue(queue);
        }
        result
    }
}

/// What a write sends, and appends when it can't all go out now: bytes (`_write`), or
/// vectors (`_writev`).
#[derive(Clone, Copy)]
pub enum WritePayload<'a> {
    Bytes(Drain<'a>),
    Vectors(&'a [IoSlice<'a>]),
}

impl WritePayload<'_> {
    /// The bytes it holds.
    #[must_use]
    pub fn length(self) -> usize {
        match self {
            Self::Bytes(data) => data.len(),
            Self::Vectors(vectors) => vectors.iter().map(|vector| vector.len()).sum(),
        }
    }
}

/// The write's effects (`corebase::tls`'s `reactor_write.rs`); `send` preserves the
/// kernel's errno.
pub trait WriteEffects {
    /// `socket->set_fd_option(1, -1)`, which also sets `nonblock`.
    fn set_nonblock(&self);
    /// How many bytes the kernel took, or None with errno set.
    fn send(&self) -> Option<usize>;
    /// `socket->out_buffer = new Buffer(chunk_size)`.
    fn create_buffer(&self);
    /// `add_write_event` unless the socket already wants writes.
    fn watch_writable(&self);
}

/// Why a write failed; the caller reports it and returns `SW_ERR`.
#[derive(Debug, PartialEq, Eq)]
pub enum WriteError {
    /// The packet is larger than the buffer size (a warning).
    TooLarge,
    /// The output buffer would overflow, with this many bytes queued (a warning).
    Overflow(usize),
    /// The send failed: the last error comes from errno.
    Send,
}

/// `Reactor::write_func`: send what the kernel takes now and queue the rest, or
/// queue it all behind earlier output.
///
/// # Errors
/// The [`WriteError`] to report.
pub fn swr_v1_reactor_write(
    socket: Socket<'_>,
    default_buffer_size: u32,
    payload: WritePayload<'_>,
    ops: &impl WriteEffects,
) -> Result<usize, WriteError> {
    let length = payload.length();
    // Making the socket nonblocking leaves the queue as it is.
    let (mut buffer_size, nonblock, ssl_send, held) = socket.read(|record| {
        let held = (!record.output_drained()).then(|| record.queued_bytes());
        (
            record.output_limit(),
            record.is_nonblocking(),
            record.sends_through_tls(),
            held,
        )
    });
    if buffer_size == 0 {
        buffer_size = default_buffer_size;
        socket.edit(|record| record.limit_output(buffer_size));
    }
    if !nonblock {
        ops.set_nonblock();
    }
    // Upstream compares `(uint32_t) _len`.
    if length.c_cast::<u32>() > buffer_size {
        return Err(WriteError::TooLarge);
    }
    let mut sent = 0;
    if let Some(held) = held {
        if u64::from(held) + length as u64 > u64::from(buffer_size) {
            return Err(WriteError::Overflow(held as usize));
        }
    } else {
        if !ssl_send {
            loop {
                if let Some(n @ 1..) = ops.send() {
                    if n == length {
                        return Ok(n);
                    }
                    sent = n;
                    break;
                }
                let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                if socket.catch_error(errno) == crate::reactor::Caught::Wait {
                    break;
                }
                if errno != libc::EINTR {
                    return Err(WriteError::Send);
                }
            }
        }
        if !socket.read(SocketFields::has_output_queue) {
            ops.create_buffer();
        }
        ops.watch_writable();
    }
    let offset = sent as u64;
    socket.read(|record| record.queue_payload(payload, offset));
    Ok(length)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reactor::SocketRecord;
    use std::cell::RefCell;
    use std::ffi::c_int;

    #[derive(Default)]
    struct Native {
        sends: Vec<isize>,
        errno: c_int,
        trace: Vec<&'static str>,
        record: usize,
    }
    thread_local! { static NATIVE: RefCell<Native> = RefCell::default(); }

    struct Ops;
    impl WriteEffects for Ops {
        fn set_nonblock(&self) {
            NATIVE.with_borrow_mut(|n| n.trace.push("nonblock"));
        }
        fn send(&self) -> Option<usize> {
            let (sent, errno) = NATIVE.with_borrow_mut(|n| {
                n.trace.push("send");
                (n.sends.remove(0), n.errno)
            });
            if sent < 0 {
                crate::sys::set_errno(errno);
            }
            usize::try_from(sent).ok()
        }
        fn create_buffer(&self) {
            let record = NATIVE.with_borrow_mut(|n| {
                n.trace.push("create");
                n.record as *mut SocketRecord
            });
            let socket = unsafe { Socket::from_ptr(record) }.unwrap();
            socket.edit(|record| record.output.out_buffer = Some(new_buffer(0)));
        }
        fn watch_writable(&self) {
            NATIVE.with_borrow_mut(|n| n.trace.push("watch"));
        }
    }

    /// Writes `data` from a socket whose output block is `output`, which the write
    /// updates; a buffer the write creates is measured, then freed, and one `output`
    /// held goes back to it.
    fn run(
        output: &mut SocketOutput,
        data: &[u8],
        sends: Vec<isize>,
        errno: c_int,
    ) -> (Result<usize, WriteError>, Vec<&'static str>, u32) {
        let had_buffer = output.out_buffer.is_some();
        let mut record = SocketRecord::new();
        record.output = SocketOutput {
            out_buffer: output.out_buffer.take(),
            ..*output
        };
        let socket = unsafe { Socket::from_ptr(&raw mut record) }.unwrap();
        NATIVE.with_borrow_mut(|n| {
            *n = Native {
                sends,
                errno,
                record: socket.as_ptr() as usize,
                ..Native::default()
            }
        });
        let payload = WritePayload::Bytes(Drain::new(data));
        let result = swr_v1_reactor_write(socket, 64, payload, &Ops);
        let held = record.output.buffer().map_or(0, swr_cpp_buffer_length);
        if !had_buffer {
            record.output.out_buffer = None;
        }
        *output = SocketOutput {
            out_buffer: record.output.out_buffer.take(),
            ..record.output
        };
        (
            result,
            NATIVE.with_borrow_mut(|n| std::mem::take(&mut n.trace)),
            held,
        )
    }

    fn socket() -> SocketOutput {
        SocketOutput {
            out_buffer: None,
            send_timer: None,
            buffer_size: 0,
            close_wait: false,
            nonblock: false,
            ssl_send: false,
            send_wait: false,
        }
    }

    #[test]
    fn upstream_write_func_branches() {
        // Whole send: the default size is applied and the socket made nonblocking.
        let mut s = socket();
        let (r, trace, held) = run(&mut s, b"hello", vec![5], 0);
        assert_eq!(
            (r, trace, held, s.buffer_size),
            (Ok(5), vec!["nonblock", "send"], 0, 64)
        );

        // Partial send: the rest is queued in a new buffer, with write interest.
        let mut s = SocketOutput {
            nonblock: true,
            ..socket()
        };
        let (r, trace, held) = run(&mut s, b"hello", vec![2], 0);
        assert_eq!(
            (r, trace, held),
            (Ok(5), vec!["send", "create", "watch"], 3)
        );

        // EINTR retries; EAGAIN waits and queues everything.
        let mut s = SocketOutput {
            nonblock: true,
            ..socket()
        };
        let (r, trace, held) = run(&mut s, b"hey", vec![-1, 3], libc::EINTR);
        assert_eq!((r, trace, held), (Ok(3), vec!["send", "send"], 0));
        let mut s = SocketOutput {
            nonblock: true,
            ..socket()
        };
        let (r, trace, held) = run(&mut s, b"hey", vec![-1], libc::EAGAIN);
        assert_eq!(
            (r, trace, held),
            (Ok(3), vec!["send", "create", "watch"], 3)
        );

        // Other errors fail with the last error; TLS skips the direct send.
        let mut s = SocketOutput {
            nonblock: true,
            ..socket()
        };
        assert_eq!(
            run(&mut s, b"x", vec![-1], libc::EPIPE).0,
            Err(WriteError::Send)
        );
        let mut s = SocketOutput {
            nonblock: true,
            ssl_send: true,
            ..socket()
        };
        let (r, trace, held) = run(&mut s, b"tls", vec![], 0);
        assert_eq!((r, trace, held), (Ok(3), vec!["create", "watch"], 3));

        // Too large for the buffer size; queued output that would overflow.
        let mut s = SocketOutput {
            nonblock: true,
            buffer_size: 2,
            ..socket()
        };
        assert_eq!(run(&mut s, b"abc", vec![], 0).0, Err(WriteError::TooLarge));
        let held_buffer = new_buffer(0);
        held_buffer.append(b"12345");
        let mut s = SocketOutput {
            nonblock: true,
            buffer_size: 6,
            out_buffer: Some(held_buffer),
            ..socket()
        };
        assert_eq!(
            run(&mut s, b"ab", vec![], 0).0,
            Err(WriteError::Overflow(5))
        );
        // Behind queued output, it's appended without a send.
        let mut s = SocketOutput {
            nonblock: true,
            buffer_size: 64,
            out_buffer: s.out_buffer.take(),
            ..socket()
        };
        let (r, trace, held) = run(&mut s, b"ab", vec![], 0);
        assert_eq!((r, trace, held), (Ok(2), vec![], 7));
    }
}
