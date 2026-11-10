//! The reactor's buffered write (upstream `Reactor::write_func`, behind `_write` and
//! `_writev`) over the socket's write-path fields, which `network::Socket` embeds as
//! one `SocketOutput` block. Native code performs the four effects that need its
//! objects: making the socket nonblocking, the send, creating the `Buffer`, and
//! registering write interest. It logs and raises from the returned outcome.
use crate::catch_error;
use std::ffi::{c_int, c_void};
use swoole_rs_buffer::{
    Buffer, swr_v1_buffer_append, swr_v1_buffer_append_iovec, swr_v1_buffer_stats,
};

/// `SwReturnCode::SW_WAIT`.
const SW_WAIT: c_int = 2;

/// `network::Socket`'s write-path fields, in the order and types it embeds them.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SocketOutput {
    /// `Buffer *out_buffer`, created and owned natively.
    pub out_buffer: *mut c_void,
    /// `TimerNode *send_timer`.
    pub send_timer: *mut c_void,
    pub buffer_size: u32,
    pub close_wait: u8,
    pub nonblock: u8,
    pub ssl_send: u8,
    pub send_wait: u8,
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
    /// `socket->out_buffer = new Buffer(chunk_size)`; its Rust handle.
    fn create_buffer(&self) -> *mut Buffer;
    /// `add_write_event` unless the socket already wants writes.
    fn watch_writable(&self);
}

/// `value` is the result when `outcome` is 0. Otherwise native code reports and
/// returns `SW_ERR`: 1 packet larger than the buffer size, 2 output buffer overflow
/// (both warnings), 3 send failed (last error from errno), 4 a buffer `status` to
/// raise.
#[repr(C)]
#[derive(Debug, PartialEq, Eq)]
pub struct WriteResult {
    pub value: isize,
    pub outcome: i32,
    pub status: i32,
}

fn done(value: isize) -> WriteResult {
    WriteResult {
        value,
        outcome: 0,
        status: 0,
    }
}

fn failed(outcome: i32, status: i32) -> WriteResult {
    WriteResult {
        value: -1,
        outcome,
        status,
    }
}

/// # Safety
/// `buffer` is a live Rust buffer handle.
unsafe fn length(buffer: *mut Buffer) -> Result<u32, i32> {
    let (mut length, mut count) = (0u32, 0usize);
    // SAFETY: as the caller promises; both outputs are local.
    match unsafe { swr_v1_buffer_stats(buffer, &raw mut length, &raw mut count) } {
        0 => Ok(if count == 0 { 0 } else { length }),
        status => Err(status),
    }
}

/// `Buffer::empty`: null or no entries.
///
/// # Safety
/// As for [`length`], or null.
unsafe fn empty(buffer: *mut Buffer) -> Result<bool, i32> {
    if buffer.is_null() {
        return Ok(true);
    }
    let (mut length, mut count) = (0u32, 0usize);
    // SAFETY: as the caller promises; both outputs are local.
    match unsafe { swr_v1_buffer_stats(buffer, &raw mut length, &raw mut count) } {
        0 => Ok(count == 0),
        status => Err(status),
    }
}

/// `Reactor::write_func`: send what the kernel takes now and queue the rest, or
/// queue it all behind earlier output.
///
/// # Safety
/// `output` is the socket's embedded block and `kernel_nobufs` its flag; `buffer` is
/// its `out_buffer`'s Rust handle or null; `payload` is valid for the call.
pub unsafe fn swr_v1_reactor_write(
    output: *mut SocketOutput,
    kernel_nobufs: *mut bool,
    buffer: *mut Buffer,
    default_buffer_size: u32,
    payload: *const WritePayload,
    ops: &impl WriteEffects,
) -> WriteResult {
    // SAFETY: as the caller promises.
    let output = unsafe { &mut *output };
    // SAFETY: as above.
    let payload = unsafe { &*payload };
    let length = payload.length;
    if output.buffer_size == 0 {
        output.buffer_size = default_buffer_size;
    }
    if output.nonblock == 0 {
        ops.set_nonblock();
    }
    // Upstream compares `(uint32_t) _len`.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream's (uint32_t) cast"
    )]
    if length as u32 > output.buffer_size {
        return failed(1, 0);
    }
    // SAFETY: `buffer` is the socket's handle or null.
    let queued = match unsafe { empty(buffer) } {
        Ok(empty) => !empty,
        Err(status) => return failed(4, status),
    };
    let mut buffer = buffer;
    let mut sent = 0isize;
    if queued {
        // SAFETY: non-empty, so a live handle.
        let held = match unsafe { self::length(buffer) } {
            Ok(held) => held,
            Err(status) => return failed(4, status),
        };
        if u64::from(held) + length as u64 > u64::from(output.buffer_size) {
            return failed(2, 0);
        }
    } else {
        if output.ssl_send == 0 {
            loop {
                sent = ops.send();
                if sent > 0 {
                    if usize::try_from(sent).is_ok_and(|sent| sent == length) {
                        return done(sent);
                    }
                    break;
                }
                let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                // SAFETY: the socket's own flag.
                if unsafe { catch_error(errno, &mut *kernel_nobufs) } == SW_WAIT {
                    break;
                }
                if errno != libc::EINTR {
                    return failed(3, 0);
                }
            }
        }
        if output.out_buffer.is_null() {
            buffer = ops.create_buffer();
        }
        ops.watch_writable();
    }
    let offset = sent.max(0).unsigned_abs();
    let status = if payload.iov.is_null() {
        let rest = length.saturating_sub(offset);
        // SAFETY: `data` holds `length` bytes, and `offset` of them were sent.
        unsafe { swr_v1_buffer_append(buffer, payload.data.wrapping_add(offset), rest) }
    } else {
        let offset = i64::try_from(offset).unwrap_or(i64::MAX);
        // SAFETY: `iov` holds `count` iovecs; `offset` bytes of them were sent.
        unsafe { swr_v1_buffer_append_iovec(buffer, payload.iov, payload.count, offset) }
    };
    if status != 0 {
        return failed(4, status);
    }
    done(isize::try_from(length).unwrap_or(isize::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use swoole_rs_buffer::{swr_v1_buffer_release, swr_v1_buffer_typed_create};

    #[derive(Default)]
    struct Native {
        sends: Vec<isize>,
        errno: c_int,
        trace: Vec<&'static str>,
        buffer: *mut Buffer,
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
                unsafe { *libc_errno() = errno };
            }
            sent
        }
        fn create_buffer(&self) -> *mut Buffer {
            NATIVE.with_borrow_mut(|n| {
                n.trace.push("create");
                n.buffer
            })
        }
        fn watch_writable(&self) {
            NATIVE.with_borrow_mut(|n| n.trace.push("watch"));
        }
    }
    #[cfg(target_os = "linux")]
    unsafe fn libc_errno() -> *mut c_int {
        unsafe { libc::__errno_location() }
    }
    #[cfg(target_os = "macos")]
    unsafe fn libc_errno() -> *mut c_int {
        unsafe { libc::__error() }
    }

    fn run(
        output: &mut SocketOutput,
        buffer: *mut Buffer,
        data: &[u8],
        sends: Vec<isize>,
        errno: c_int,
    ) -> (WriteResult, Vec<&'static str>, u32) {
        let mut fresh = std::ptr::null_mut();
        assert_eq!(unsafe { swr_v1_buffer_typed_create(0, &raw mut fresh) }, 0);
        NATIVE.with_borrow_mut(|n| {
            *n = Native {
                sends,
                errno,
                buffer: fresh,
                ..Native::default()
            }
        });
        let payload = WritePayload {
            data: data.as_ptr(),
            iov: std::ptr::null(),
            count: 0,
            length: data.len(),
        };
        let mut nobufs = false;
        let result = unsafe {
            swr_v1_reactor_write(
                output,
                &raw mut nobufs,
                buffer,
                64,
                &raw const payload,
                &Ops,
            )
        };
        let target = if buffer.is_null() { fresh } else { buffer };
        let held = unsafe { length(target) }.unwrap();
        unsafe { swr_v1_buffer_release(fresh) };
        (
            result,
            NATIVE.with_borrow_mut(|n| std::mem::take(&mut n.trace)),
            held,
        )
    }

    fn socket() -> SocketOutput {
        SocketOutput {
            out_buffer: std::ptr::null_mut(),
            send_timer: std::ptr::null_mut(),
            buffer_size: 0,
            close_wait: 0,
            nonblock: 0,
            ssl_send: 0,
            send_wait: 0,
        }
    }

    #[test]
    fn upstream_write_func_branches() {
        // Whole send: the default size is applied and the socket made nonblocking.
        let mut s = socket();
        let (r, trace, held) = run(&mut s, std::ptr::null_mut(), b"hello", vec![5], 0);
        assert_eq!(
            (r, trace, held, s.buffer_size),
            (done(5), vec!["nonblock", "send"], 0, 64)
        );

        // Partial send: the rest is queued in a new buffer, with write interest.
        let mut s = SocketOutput {
            nonblock: 1,
            ..socket()
        };
        let (r, trace, held) = run(&mut s, std::ptr::null_mut(), b"hello", vec![2], 0);
        assert_eq!(
            (r, trace, held),
            (done(5), vec!["send", "create", "watch"], 3)
        );

        // EINTR retries; EAGAIN waits and queues everything.
        let mut s = SocketOutput {
            nonblock: 1,
            ..socket()
        };
        let (r, trace, held) = run(
            &mut s,
            std::ptr::null_mut(),
            b"hey",
            vec![-1, 3],
            libc::EINTR,
        );
        assert_eq!((r, trace, held), (done(3), vec!["send", "send"], 0));
        let mut s = SocketOutput {
            nonblock: 1,
            ..socket()
        };
        let (r, trace, held) = run(&mut s, std::ptr::null_mut(), b"hey", vec![-1], libc::EAGAIN);
        assert_eq!(
            (r, trace, held),
            (done(3), vec!["send", "create", "watch"], 3)
        );

        // Other errors fail with the last error; TLS skips the direct send.
        let mut s = SocketOutput {
            nonblock: 1,
            ..socket()
        };
        assert_eq!(
            run(&mut s, std::ptr::null_mut(), b"x", vec![-1], libc::EPIPE).0,
            failed(3, 0)
        );
        let mut s = SocketOutput {
            nonblock: 1,
            ssl_send: 1,
            ..socket()
        };
        let (r, trace, held) = run(&mut s, std::ptr::null_mut(), b"tls", vec![], 0);
        assert_eq!((r, trace, held), (done(3), vec!["create", "watch"], 3));

        // Too large for the buffer size; queued output that would overflow.
        let mut s = SocketOutput {
            nonblock: 1,
            buffer_size: 2,
            ..socket()
        };
        assert_eq!(
            run(&mut s, std::ptr::null_mut(), b"abc", vec![], 0).0,
            failed(1, 0)
        );
        let mut held_buffer = std::ptr::null_mut();
        assert_eq!(
            unsafe { swr_v1_buffer_typed_create(0, &raw mut held_buffer) },
            0
        );
        assert_eq!(
            unsafe { swr_v1_buffer_append(held_buffer, b"12345".as_ptr(), 5) },
            0
        );
        let mut s = SocketOutput {
            nonblock: 1,
            buffer_size: 6,
            out_buffer: held_buffer.cast(),
            ..socket()
        };
        assert_eq!(run(&mut s, held_buffer, b"ab", vec![], 0).0, failed(2, 0));
        // Behind queued output, it's appended without a send.
        let mut s = SocketOutput {
            nonblock: 1,
            buffer_size: 64,
            out_buffer: held_buffer.cast(),
            ..socket()
        };
        let (r, trace, held) = run(&mut s, held_buffer, b"ab", vec![], 0);
        assert_eq!((r, trace, held), (done(2), vec![], 7));
        unsafe { swr_v1_buffer_release(held_buffer) };
    }
}
