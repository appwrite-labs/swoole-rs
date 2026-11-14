//! Upstream's `core-tests/src/protocol/base.cpp` TESTs that start a server, one function
//! per TEST with the same inputs and expected values. Built into the http server case
//! binary (tests/modules/http/server), whose http1/case.rs builds the Server with its port's
//! framing (`swr_case_protocol_server`). The client is a blocking `TcpStream`, as
//! upstream's synchronous `network::Client`. The file's other TESTs are unit tests in
//! crates/ports (`swap_byte_order`, `unpack`, `hton64`) and crates/protocols-ffi (`socks5_*`).

use super::http1::{SW_LOG_WARNING, swoole_set_log_level};
use super::{
    BODY, CHILD, CppString, SW_OK, Server, base_worker_start, primary_port, request, send,
    signal_block_all, string, string_append_random_bytes, swr_case_protocol_server,
    swr_case_shutdown, swr_case_start, value,
};
use std::ffi::c_int;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::panic::resume_unwind;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::sleep;
use std::time::Duration;

const PKG_N: usize = 128;
const MAX_SIZE: c_int = 1024 * 1024 + 65536;
const MIN_SIZE: c_int = 512;
/// `swr_case_protocol_server`'s framings.
const STREAM: c_int = 0;
const LENGTH_L4: c_int = 1;
const EOF_CRLF: c_int = 2;

unsafe extern "C" {
    #[link_name = "_ZN6swoole6StringD1Ev"]
    fn string_drop(this: *mut CppString);
    #[link_name = "_Z11swoole_randii"]
    fn swoole_rand(min: c_int, max: c_int) -> c_int;
    #[link_name = "_Z17swoole_random_intv"]
    fn swoole_random_int() -> u64;
}

static PKGS: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());
static RECV_COUNT: AtomicUsize = AtomicUsize::new(0);

fn rand(min: c_int, max: c_int) -> usize {
    // SAFETY: a plain call.
    usize::try_from(unsafe { swoole_rand(min, max) }).unwrap()
}

/// `String::append_random_bytes(length, base64)` on an empty String.
pub(super) fn random_bytes(length: usize, base64: bool) -> Vec<u8> {
    let mut str = string(64);
    // SAFETY: the live String; then its destructor, once.
    unsafe { string_append_random_bytes(&mut str, length, base64) };
    let bytes = value(&str).to_vec();
    // SAFETY: as above.
    unsafe { string_drop(&mut str) };
    bytes
}

/// A length-framed package: the body's length as a network-order int, then the body.
fn length_package(length: usize) -> Vec<u8> {
    let mut package = u32::try_from(length).unwrap().to_be_bytes().to_vec();
    package.extend(random_bytes(length, false));
    package
}

/// `network::Client::connect(TEST_HOST, port, 1, 0)`.
fn connect(port: c_int) -> TcpStream {
    let address = SocketAddr::from(([127, 0, 0, 1], u16::try_from(port).unwrap()));
    TcpStream::connect_timeout(&address, Duration::from_secs(1)).expect("connect")
}

/// Starts the created server; `body` runs in a thread once the worker starts.
pub(super) fn serve(serv: *mut Server, body: fn(*mut Server)) {
    *BODY.lock().unwrap() = Some(body);
    // SAFETY: the created server.
    unsafe { swr_case_start(serv) };
    let child = CHILD.lock().unwrap().take().expect("onWorkerStart ran");
    if let Err(panic) = child.join() {
        resume_unwind(panic);
    }
}

/// `test_protocol`'s `onReceive`: each package as sent, then shutdown after the last.
extern "C" fn package_receive(serv: *mut Server, req: *const std::ffi::c_void) -> c_int {
    let (_, data, _) = request(req);
    let count = RECV_COUNT.fetch_add(1, Ordering::Relaxed);
    // memcmp(req->data, pkgs[recv_count].str, req->info.len) == 0
    assert!(PKGS.lock().unwrap()[count].get(..data.len()) == Some(data));
    if count + 1 == PKG_N {
        sleep(Duration::from_millis(100));
        // SAFETY: the running server.
        unsafe { swr_case_shutdown(serv) };
    }
    SW_OK
}

/// `test_protocol(serv, port, pkgs)`: every package down one connection, in random
/// chunks of at most 64KiB.
fn test_protocol(framing: c_int) {
    RECV_COUNT.store(0, Ordering::Relaxed);
    // SAFETY: the builder's callbacks have the slots' signatures.
    let serv = unsafe { swr_case_protocol_server(framing, base_worker_start, package_receive) };
    serve(serv, |serv| {
        signal_block_all();
        let wbuf = PKGS.lock().unwrap().concat();
        let mut cli = connect(primary_port(serv));
        let mut offset = 0;
        while offset < wbuf.len() {
            let n = if wbuf.len() - offset > 65536 {
                // SAFETY: a plain call.
                usize::try_from(unsafe { swoole_random_int() } % 65536 + 1).unwrap()
            } else {
                wbuf.len() - offset
            };
            cli.write_all(&wbuf[offset..offset + n]).unwrap();
            offset += n;
        }
        sleep(Duration::from_millis(100));
    });
}

/// `onReceive` of length_2 and length_3: `OK\0` to every package.
extern "C" fn ok_receive(serv: *mut Server, req: *const std::ffi::c_void) -> c_int {
    let (session_id, _, _) = request(req);
    send(serv, session_id, b"OK\0");
    SW_OK
}

fn recv(cli: &mut TcpStream, rbuf: &mut [u8; 32]) -> usize {
    cli.read(rbuf).unwrap()
}

fn length() {
    *PKGS.lock().unwrap() = (0..PKG_N)
        .map(|_| length_package(rand(MIN_SIZE, MAX_SIZE)))
        .collect();
    // SAFETY: a plain call.
    unsafe { swoole_set_log_level(SW_LOG_WARNING) };
    test_protocol(STREAM);
}

fn length_2() {
    // SAFETY: the builder's callbacks have the slots' signatures.
    let serv = unsafe { swr_case_protocol_server(STREAM, base_worker_start, ok_receive) };
    serve(serv, |serv| {
        signal_block_all();
        let port = primary_port(serv);
        let mut rbuf = [0u8; 32];
        sleep(Duration::from_millis(50));

        // The length header in two sends.
        {
            let mut cli = connect(port);
            let wbuf = length_package(rand(MIN_SIZE, MAX_SIZE));
            assert_eq!(cli.write(&wbuf[..2]).unwrap(), 2);
            sleep(Duration::from_micros(10));
            assert_eq!(cli.write(&wbuf[2..6]).unwrap(), 4);
            sleep(Duration::from_micros(10));
            cli.write_all(&wbuf[2..2 + wbuf.len() - 6]).unwrap();
            assert_eq!(recv(&mut cli, &mut rbuf), 3);
            assert_eq!(&rbuf[..3], b"OK\0");
        }

        // A zero-length package.
        {
            let mut cli = connect(port);
            assert_eq!(cli.write(&0u32.to_be_bytes()).unwrap(), 4);
            assert_eq!(recv(&mut cli, &mut rbuf), 3);
            assert_eq!(&rbuf[..3], b"OK\0");
        }

        // An INT_MAX-length package.
        {
            let mut cli = connect(port);
            assert_eq!(cli.write(&i32::MAX.to_be_bytes()).unwrap(), 4);
            assert_eq!(recv(&mut cli, &mut rbuf), 0);
        }
        sleep(Duration::from_millis(50));
        // SAFETY: the running server.
        unsafe { swr_case_shutdown(serv) };
    });
}

fn length_3() {
    // SAFETY: the builder's callbacks have the slots' signatures.
    let serv = unsafe { swr_case_protocol_server(LENGTH_L4, base_worker_start, ok_receive) };
    serve(serv, |serv| {
        signal_block_all();
        let mut rbuf = [0u8; 32];
        sleep(Duration::from_millis(50));

        let mut cli = connect(primary_port(serv));
        assert_eq!(cli.write(&(-1i32).to_ne_bytes()).unwrap(), 4);
        assert_eq!(recv(&mut cli, &mut rbuf), 0);

        sleep(Duration::from_millis(50));
        // SAFETY: the running server.
        unsafe { swr_case_shutdown(serv) };
    });
}

fn eof() {
    *PKGS.lock().unwrap() = (0..PKG_N)
        .map(|_| {
            let mut package = random_bytes(rand(MIN_SIZE, MAX_SIZE), true);
            package.extend_from_slice(b"\r\n");
            package
        })
        .collect();
    // SAFETY: a plain call.
    unsafe { swoole_set_log_level(SW_LOG_WARNING) };
    test_protocol(EOF_CRLF);
}

pub(super) const CASES: super::Cases = &[
    ("length", length),
    ("length_2", length_2),
    ("length_3", length_3),
    ("eof", eof),
];
