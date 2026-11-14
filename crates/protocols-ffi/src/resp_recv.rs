//! `swoole::redis::recv_packet`: reads a connection until a whole RESP command is
//! buffered, then hands it to `onPackage`. [`recv_packet`] is the loop over a
//! [`RespConnection`], which the ports crate runs over the server's own records.
use core::ffi::c_int;
use swoole_rs_corebase::{log, set_errno, sw_calloc};
pub use swoole_rs_protocols::RespRequest;
use swoole_rs_protocols::{RespAction, resp_extend_size, resp_receive_range};

pub(crate) const SW_OK: c_int = swoole_rs_corebase::constants::ResultCode::SW_OK as c_int;
pub const SW_ERR: c_int = swoole_rs_corebase::constants::ResultCode::SW_ERR as c_int;
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
    /// `protocol->onPackage` of the whole content.
    fn dispatch(&mut self) -> Dispatched;
    /// `buffer->clear()`, after a dispatch.
    fn clear(&mut self);
}

/// What [`RespConnection::dispatch`] left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dispatched {
    /// The socket is still the connection's.
    Kept,
    /// The socket was removed meanwhile.
    Removed,
    /// `onPackage` failed, or there is none.
    Failed,
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
            RespAction::ReadAgain => {
                let grown = resp_extend_size(length, size, maximum, page);
                if grown != 0 {
                    c.extend(grown);
                }
            }
            RespAction::Wait => return SW_OK,
            RespAction::Dispatch => {
                match c.dispatch() {
                    Dispatched::Failed => return SW_ERR,
                    Dispatched::Removed => return SW_OK,
                    Dispatched::Kept => {}
                }
                c.clear();
                *c.view().2 = RespRequest::default();
                return SW_OK;
            }
            RespAction::TooBig => {
                log::warning(
                    ORIGIN,
                    format_args!("Package is too big. package_length={length}"),
                );
                return SW_ERR;
            }
            RespAction::Malformed => {
                log::warning(ORIGIN, format_args!("redis protocol error"));
                return SW_ERR;
            }
        }
    }
}

/// The request state in `slot`, allocated zeroed with `sw_calloc` when null; null
/// (after a warning) when that fails.
pub fn request(slot: &mut *mut RespRequest) -> *mut RespRequest {
    if slot.is_null() {
        // Zeroed, which every field of a RespRequest takes (`RespState::TotalLine`).
        let fresh = sw_calloc(1, size_of::<RespRequest>()).cast::<RespRequest>();
        if fresh.is_null() {
            log::warning(
                ORIGIN,
                format_args!("malloc({}) failed", size_of::<RespRequest>()),
            );
            return fresh;
        }
        *slot = fresh;
    }
    *slot
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::disallowed_types,
        reason = "a scripted socket's reads; tools/budget.py doesn't count test code"
    )]
    use super::*;
    use std::collections::VecDeque;

    /// A scripted socket, its receive buffer of `size` bytes and the request state.
    struct Script {
        reads: VecDeque<Vec<u8>>,
        content: Vec<u8>,
        size: usize,
        offset: i64,
        request: RespRequest,
        packages: Vec<Vec<u8>>,
    }

    impl RespConnection for Script {
        fn recv(&mut self) -> isize {
            let Some(mut chunk) = self.reads.pop_front() else {
                return RESP_RECV_DONE;
            };
            let room = self.size - self.content.len();
            if chunk.len() > room {
                self.reads.push_front(chunk.split_off(room));
            }
            self.content.extend_from_slice(&chunk);
            isize::try_from(chunk.len()).unwrap()
        }

        fn view(&mut self) -> (&[u8], usize, &mut RespRequest) {
            (&self.content, self.size, &mut self.request)
        }

        fn set_offset(&mut self, offset: i64) {
            self.offset = offset;
        }

        fn extend(&mut self, size: usize) {
            self.size = size;
        }

        fn dispatch(&mut self) -> Dispatched {
            self.packages.push(self.content.clone());
            Dispatched::Kept
        }

        fn clear(&mut self) {
            self.content.clear();
            self.offset = 0;
        }
    }

    fn run(reads: &[&[u8]], size: usize, maximum: u32) -> (Vec<c_int>, Script) {
        let mut script = Script {
            reads: reads.iter().map(|r| r.to_vec()).collect(),
            content: Vec::new(),
            size,
            offset: 0,
            request: RespRequest::default(),
            packages: Vec::new(),
        };
        let mut results = Vec::new();
        while !script.reads.is_empty() && results.last() != Some(&SW_ERR) {
            results.push(recv_packet(&mut script, maximum, 4096));
        }
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
        assert_eq!((script.content.len(), script.offset), (0, 0));
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
