//! The parts of `native.cc` whose Rust side had no caller but that gate: the receive
//! loop against the pinned `redis::recv_packet` (`swoole_rs_oracle::resp_stream`), and
//! the handler-key model. Each C export the gate called is replaced by the safe
//! function that was its body (README.md maps them).
#![expect(
    clippy::disallowed_types,
    reason = "a modeled socket's reads; tools/budget.py doesn't count test code"
)]
use core::ffi::c_int;
use std::collections::VecDeque;

use swoole_rs_corebase::{set_errno, sys::errno};
use swoole_rs_oracle::resp_stream;
use swoole_rs_protocols::{RespRequest, resp_handler_key};
use swoole_rs_protocols_ffi::{RESP_RECV_DONE, RespConnection, SW_ERR, recv_packet};

/// The oracle shim's socket, String and connection: each recv hands out at most the
/// next chunk, then would block. Its log is the shim's trace.
struct Model {
    chunks: VecDeque<Vec<u8>>,
    content: Vec<u8>,
    size: usize,
    offset: i64,
    request: RespRequest,
    log: Vec<u8>,
}

impl RespConnection for Model {
    fn recv(&mut self) -> isize {
        let Some(chunk) = self.chunks.front_mut() else {
            set_errno(libc::EAGAIN);
            return RESP_RECV_DONE;
        };
        let count = chunk.len().min(self.size - self.content.len());
        self.content.extend(chunk.drain(..count));
        if chunk.is_empty() {
            self.chunks.pop_front();
        }
        isize::try_from(count).unwrap_or(isize::MAX)
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

    fn dispatch(&mut self) -> c_int {
        let error = errno();
        self.log
            .extend_from_slice(format!("P{}:", self.content.len()).as_bytes());
        self.log.extend_from_slice(&self.content);
        self.log.extend_from_slice(format!("E{error}").as_bytes());
        0
    }

    fn clear(&mut self) {
        self.content.clear();
        self.offset = 0;
    }
}

/// The shim's driver over Rust's loop: `recv_packet` until it fails or the chunks are
/// drained, logged as `resp_stream::UPSTREAM_TRACE` logs the pinned one.
fn rust_trace(command: &[u8], split: usize, buffer_size: usize, maximum: u32) -> Vec<u8> {
    let (first, second) = command.split_at(split);
    let mut model = Model {
        chunks: [first, second]
            .into_iter()
            .filter(|chunk| !chunk.is_empty())
            .map(<[u8]>::to_vec)
            .collect(),
        content: Vec::new(),
        size: buffer_size,
        offset: 0,
        request: RespRequest::default(),
        log: Vec::new(),
    };
    for _ in 0..4096 {
        let drained = model.chunks.is_empty();
        set_errno(libc::EDOM);
        let result = recv_packet(&mut model, maximum, 4096);
        let error = errno();
        let (length, offset, r) = (model.content.len(), model.offset, model.request);
        let call = format!(
            "R{result}E{error}L{length}O{offset},{},{}/{},{}/{},{};",
            r.state, r.lines_received, r.lines_total, r.bytes_received, r.bytes_total, r.offset
        );
        model.log.extend_from_slice(call.as_bytes());
        if result == SW_ERR {
            break;
        }
        if drained {
            model
                .log
                .extend_from_slice(format!("L{length}O{offset}").as_bytes());
            break;
        }
    }
    model.log
}

/// The pinned `redis::recv_packet` over the same stream, chunks and buffer.
fn reference_trace(command: &[u8], split: usize, buffer_size: u32, maximum: u32) -> Vec<u8> {
    let chunks = [split, command.len() - split].map(|size| u16::try_from(size).unwrap_or(u16::MAX));
    let stream = resp_stream::Stream {
        data: command.as_ptr(),
        length: command.len(),
        chunks: chunks.as_ptr(),
        chunk_count: chunks.len(),
        buffer_size,
        max_length: maximum,
        close_at_end: 0,
    };
    let mut out = vec![0u8; 1 << 16];
    // SAFETY: the stream's pointers outlive the call; `out` has its capacity.
    let length =
        unsafe { resp_stream::UPSTREAM_TRACE(&raw const stream, out.as_mut_ptr(), out.len()) };
    assert!(length <= out.len(), "the log fits");
    out.truncate(length);
    out
}

/// `native.cc`'s receive states: every split of eight commands at three package
/// limits, into a 64-byte buffer. Each call's result, errno (at every package and on
/// return), packages, buffer length and offset, and request state must match.
#[test]
fn receive_states() {
    let mut receives = 0;
    let commands: [&[u8]; 8] = [
        b"*1\r\n$3\r\nGET\r\n",
        b"*2\r\n$3\r\nSET\r\n$3\r\nabc\r\n",
        b"*1\r\n$10\r\nabc\r\n",
        b":1\r\n",
        b"*1\r\n$-1\r\n",
        b"*1\r\n:9223372036854775808\r\n",
        b"*1\r\n$9223372036854775808x\r\n",
        b"*-9223372036854775809\r\n",
    ];
    for command in commands {
        for split in 2..=command.len() {
            for maximum in [12, 64, 1_048_576] {
                let expected = reference_trace(command, split, 64, maximum);
                let actual = rust_trace(command, split, 64, maximum);
                assert_eq!(
                    String::from_utf8_lossy(&expected),
                    String::from_utf8_lossy(&actual),
                    "split {split}, maximum {maximum}"
                );
                receives += 1;
            }
        }
    }
    assert_eq!(
        receives,
        3 * commands.iter().map(|c| c.len() - 1).sum::<usize>()
    );
}

/// `native.cc`'s `handler_key_trace`: C printf truncation plus an ASCII case model.
/// Zend's case folding and property behaviour are compared by handlers.py on real
/// PHP engines.
#[test]
fn handler_keys() {
    fn check(wire: &[u8]) {
        let mut expected = [0u8; 64];
        let terminated = [wire, &[0]].concat();
        // SAFETY: a 64-byte output and a NUL-terminated argument for `%s`.
        let needed = unsafe {
            libc::snprintf(
                expected.as_mut_ptr().cast(),
                expected.len(),
                c"_handler_%s".as_ptr(),
                terminated.as_ptr(),
            )
        };
        let length = usize::try_from(needed)
            .expect("snprintf succeeds")
            .min(expected.len() - 1);
        expected[..length].make_ascii_lowercase();
        let (actual, actual_length) = resp_handler_key(wire);
        assert_eq!(actual_length, length);
        assert_eq!(expected[..=length], actual[..=length]);
    }
    let mut cases = 0;
    for byte in 0..=255u8 {
        for length in [0, 1, 53, 54, 55, 63, 64, 65, 1024] {
            check(&vec![byte; length]);
            cases += 1;
        }
    }
    for offset in 0..128 {
        let mut wire = [b'A'; 128];
        wire[offset] = 0;
        check(&wire);
        cases += 1;
    }
    assert_eq!(cases, 256 * 9 + 128);
}
