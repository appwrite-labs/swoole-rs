//! Upstream's `core-tests/src/os/pipe.cpp`, one function per TEST with the same inputs and
//! expected values, over the real Rust `SocketPair` and its typed operations.

use std::ffi::{CStr, c_int};
use swoole_rs_corebase::tls::socket_pair;

const SOCK_DGRAM: c_int = 2;

/// Close the pair at the same scope boundary as upstream's destructor.
struct SocketPair(socket_pair::SocketPair);

impl SocketPair {
    fn ready(&self) -> bool {
        self.0.master_end().is_some() && self.0.worker_end().is_some()
    }

    fn write(&self, data: &[u8]) -> isize {
        socket_pair::write_from(&self.0, data).map_or(-1, usize::cast_signed)
    }

    fn read(&self, buf: &mut [u8]) -> isize {
        let (at, length) = (buf.as_mut_ptr().cast(), buf.len());
        // SAFETY: `buf`'s own bytes, as ones a read may leave unwritten.
        let buf = unsafe { std::slice::from_raw_parts_mut(at, length) };
        socket_pair::read_into(&self.0, buf).map_or(-1, usize::cast_signed)
    }
}

impl Drop for SocketPair {
    fn drop(&mut self) {
        socket_pair::drop(&mut self.0);
    }
}

/// `strcmp(expected, buf) == 0`.
fn c_str_eq(expected: &CStr, buf: &[u8]) -> bool {
    super::c_str(buf) == expected.to_bytes()
}

fn unixsock() {
    let p = SocketPair(socket_pair::unix_socket(true, SOCK_DGRAM));
    assert!(p.ready());

    let mut buf = [0u8; 1024];

    // SW_STRS: the string and its NUL.
    assert!(p.write(b"hello world1\0") > 0);
    assert!(p.write(b"hello world2\0") > 0);
    assert!(p.write(b"hello world3\0") > 0);

    assert!(p.read(&mut buf) > 0);
    assert!(c_str_eq(c"hello world1", &buf));
    assert!(p.read(&mut buf) > 0);
    assert!(c_str_eq(c"hello world2", &buf));
    assert!(p.read(&mut buf) > 0);
    assert!(c_str_eq(c"hello world3", &buf));
}

fn base() {
    let mut data = [0u8; 256];

    let p = SocketPair(socket_pair::pipe(true));
    assert!(p.ready());

    assert!(p.write(b"hello world\n") > 0);
    assert!(p.write("你好中国。\n".as_bytes()) > 0);

    assert!(p.read(&mut data[..255]) > 0);
    assert!(c_str_eq(c"hello world\n你好中国。\n", &data));
}

pub(super) const CASES: super::Cases = &[("unixsock", unixsock), ("base", base)];
