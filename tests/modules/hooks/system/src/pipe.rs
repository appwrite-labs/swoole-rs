//! Upstream's `core-tests/src/os/pipe.cpp`, one function per TEST with the same inputs and
//! expected values, over the Rust `SocketPair` (crates/process) under its upstream symbols.
//! `SocketPair` mirrors the record those symbols construct (SWR_SOCKET_PAIR_RECORD_FIELDS).

use std::ffi::{CStr, c_int, c_void};

const SOCK_DGRAM: c_int = 2;

/// `swoole::SocketPair`'s fields.
#[repr(C)]
struct SocketPair {
    blocking: bool,
    timeout: f64,
    socks: [c_int; 2],
    master_socket: *mut c_void,
    worker_socket: *mut c_void,
}

unsafe extern "C" {
    #[link_name = "_ZN6swoole10UnixSocketC1Ebi"]
    fn unix_socket_new(this: *mut SocketPair, blocking: bool, protocol: c_int);
    #[link_name = "_ZN6swoole4PipeC1Eb"]
    fn pipe_new(this: *mut SocketPair, blocking: bool);
    #[link_name = "_ZN6swoole10SocketPairD1Ev"]
    fn socket_pair_drop(this: *mut SocketPair);
    #[link_name = "_ZNK6swoole10SocketPair4readEPvm"]
    fn socket_pair_read(this: *const SocketPair, buf: *mut c_void, length: usize) -> isize;
    #[link_name = "_ZNK6swoole10SocketPair5writeEPKvm"]
    fn socket_pair_write(this: *const SocketPair, buf: *const c_void, length: usize) -> isize;
}

impl SocketPair {
    fn new(construct: impl FnOnce(*mut SocketPair)) -> Box<Self> {
        // SAFETY: integers, a bool and raw pointers, for which all-zero is valid.
        let mut pair: Box<Self> = Box::new(unsafe { std::mem::zeroed() });
        construct(&raw mut *pair);
        pair
    }

    fn ready(&self) -> bool {
        !self.master_socket.is_null() && !self.worker_socket.is_null()
    }

    fn write(&self, data: &[u8]) -> isize {
        // SAFETY: `data` is readable for its length.
        unsafe { socket_pair_write(self, data.as_ptr().cast(), data.len()) }
    }

    fn read(&self, buf: &mut [u8]) -> isize {
        // SAFETY: `buf` is writable for its length.
        unsafe { socket_pair_read(self, buf.as_mut_ptr().cast(), buf.len()) }
    }
}

impl Drop for SocketPair {
    fn drop(&mut self) {
        // SAFETY: a constructed pair, destroyed once.
        unsafe { socket_pair_drop(self) };
    }
}

/// `strcmp(expected, buf) == 0`.
fn c_str_eq(expected: &CStr, buf: &[u8]) -> bool {
    super::c_str(buf) == expected.to_bytes()
}

fn unixsock() {
    // SAFETY: a zeroed record for the constructor.
    let p = SocketPair::new(|this| unsafe { unix_socket_new(this, true, SOCK_DGRAM) });
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

    // SAFETY: a zeroed record for the constructor.
    let p = SocketPair::new(|this| unsafe { pipe_new(this, true) });
    assert!(p.ready());

    assert!(p.write(b"hello world\n") > 0);
    assert!(p.write("你好中国。\n".as_bytes()) > 0);

    assert!(p.read(&mut data[..255]) > 0);
    assert!(c_str_eq(c"hello world\n你好中国。\n", &data));
}

pub(super) const CASES: super::Cases = &[("unixsock", unixsock), ("base", base)];
