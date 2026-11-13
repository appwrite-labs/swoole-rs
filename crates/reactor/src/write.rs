//! The reactor's buffered write (upstream `Reactor::write_func`, behind `_write` and
//! `_writev`) over the socket's write-path fields, which `network::Socket` embeds as
//! one `SocketOutput` block. Native code performs the four effects that need its
//! objects: making the socket nonblocking, the send, creating the `Buffer`, and
//! registering write interest. It logs and raises from the returned outcome.
use crate::Socket;
use std::ffi::{c_int, c_void};
use swoole_rs_buffer::cpp::{
    CppBuffer, swr_cpp_buffer_append_iovec, swr_cpp_buffer_count, swr_cpp_buffer_length,
};
use swoole_rs_corebase::CCast;

/// `SwReturnCode::SW_WAIT`.
const SW_WAIT: c_int = swoole_rs_corebase::constants::ReturnCode::SW_WAIT as c_int;

/// `network::Socket`'s write-path fields, in the order and types it embeds them.
#[repr(C)]
pub struct SocketOutput {
    /// `Buffer *out_buffer`: the `Buffer` the socket owns, a null pointer without one.
    pub out_buffer: Option<Box<CppBuffer>>,
    /// `TimerNode *send_timer`.
    pub send_timer: *mut c_void,
    pub buffer_size: u32,
    pub close_wait: bool,
    pub nonblock: bool,
    pub ssl_send: bool,
    pub send_wait: bool,
}

impl SocketOutput {
    /// `out_buffer`, or `None` without one.
    #[must_use]
    pub fn buffer(&self) -> Option<&CppBuffer> {
        self.out_buffer.as_deref()
    }

    /// `Buffer::empty(out_buffer)`: none, or no entries.
    pub(crate) fn empty(&self) -> bool {
        self.buffer()
            .is_none_or(|buffer| swr_cpp_buffer_count(buffer) == 0)
    }
}

/// What to append when the data can't all go out now: `length` bytes at `data`, or
/// `count` iovecs at `iov` (`_writev`).
#[repr(C)]
pub struct WritePayload {
    pub data: *const u8,
    pub iov: *const libc::iovec,
    pub count: usize,
    pub length: usize,
}

/// The write's effects (`crates/tls`'s `reactor_write.rs`); `send` preserves the
/// kernel's errno.
pub trait WriteEffects {
    /// `socket->set_fd_option(1, -1)`, which also sets `nonblock`.
    fn set_nonblock(&self);
    fn send(&self) -> isize;
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
    Overflow(isize),
    /// The send failed: the last error comes from errno.
    Send,
}

/// `Reactor::write_func`: send what the kernel takes now and queue the rest, or
/// queue it all behind earlier output.
///
/// # Safety
/// `payload` is valid for the call.
///
/// # Errors
/// The [`WriteError`] to report.
pub unsafe fn swr_v1_reactor_write(
    socket: Socket<'_>,
    default_buffer_size: u32,
    payload: &WritePayload,
    ops: &impl WriteEffects,
) -> Result<isize, WriteError> {
    let length = payload.length;
    // Making the socket nonblocking leaves the queue as it is.
    let (mut buffer_size, nonblock, ssl_send, held) = socket.read(|record| {
        let output = &record.output;
        let held = output
            .buffer()
            .filter(|&buffer| swr_cpp_buffer_count(buffer) != 0)
            .map(|buffer| swr_cpp_buffer_length(buffer));
        (output.buffer_size, output.nonblock, output.ssl_send, held)
    });
    if buffer_size == 0 {
        buffer_size = default_buffer_size;
        socket.edit(|record| record.output.buffer_size = buffer_size);
    }
    if !nonblock {
        ops.set_nonblock();
    }
    // Upstream compares `(uint32_t) _len`.
    if length.c_cast::<u32>() > buffer_size {
        return Err(WriteError::TooLarge);
    }
    let mut sent = 0isize;
    if let Some(held) = held {
        if u64::from(held) + length as u64 > u64::from(buffer_size) {
            return Err(WriteError::Overflow(
                isize::try_from(held).unwrap_or(isize::MAX),
            ));
        }
    } else {
        if !ssl_send {
            loop {
                sent = ops.send();
                if sent > 0 {
                    if usize::try_from(sent).is_ok_and(|sent| sent == length) {
                        return Ok(sent);
                    }
                    break;
                }
                let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                if socket.catch_error(errno) == SW_WAIT {
                    break;
                }
                if errno != libc::EINTR {
                    return Err(WriteError::Send);
                }
            }
        }
        if socket.read(|record| record.output.out_buffer.is_none()) {
            ops.create_buffer();
        }
        ops.watch_writable();
    }
    let offset = libc::off_t::try_from(sent.max(0)).unwrap_or(libc::off_t::MAX);
    // `_write`'s bytes as one iovec, so both kinds append past `offset` alike.
    let bytes = libc::iovec {
        iov_base: payload.data.cast_mut().cast(),
        iov_len: length,
    };
    let (iov, count) = if payload.iov.is_null() {
        (&raw const bytes, 1)
    } else {
        (payload.iov, payload.count)
    };
    socket.read(|record| {
        let Some(buffer) = record.output.buffer() else {
            // `new Buffer` threw, which nothing catches.
            swoole_rs_corebase::fatal("a socket's output buffer can't be created");
        };
        // SAFETY: `iov` holds `count` iovecs over the payload; `offset` bytes were sent.
        unsafe { swr_cpp_buffer_append_iovec(buffer, iov, count, offset) };
    });
    Ok(isize::try_from(length).unwrap_or(isize::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SocketRecord;
    use std::cell::RefCell;
    use swoole_rs_buffer::cpp::new_buffer;

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
        fn send(&self) -> isize {
            let (sent, errno) = NATIVE.with_borrow_mut(|n| {
                n.trace.push("send");
                (n.sends.remove(0), n.errno)
            });
            if sent < 0 {
                swoole_rs_corebase::sys::set_errno(errno);
            }
            sent
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
    ) -> (Result<isize, WriteError>, Vec<&'static str>, u32) {
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
        let payload = WritePayload {
            data: data.as_ptr(),
            iov: std::ptr::null(),
            count: 0,
            length: data.len(),
        };
        let result = unsafe { swr_v1_reactor_write(socket, 64, &payload, &Ops) };
        let held = record
            .output
            .buffer()
            .map_or(0, |buffer| swr_cpp_buffer_length(buffer));
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
            send_timer: std::ptr::null_mut(),
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
