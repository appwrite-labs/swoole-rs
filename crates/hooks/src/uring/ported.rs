// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 core-tests/src/coroutine/iouring.cpp (Swoole contributors).
//! Upstream's `core-tests/src/coroutine/iouring.cpp`, one function per TEST with the
//! same inputs and expected values, over the engine's test host: where a coroutine
//! would suspend, the host drives the ring until the request completes. Upstream's
//! second coroutine is a thread with its own ring. tests/modules/hooks/README.md maps
//! each TEST and records the calls that are not in the build.
use crate::uring::testing::{HOST, with_config, with_engine};
use crate::uring::{self, Config, sys};
use std::ffi::{CStr, c_int, c_uint};
use std::io::Read;
use std::net::{IpAddr, ToSocketAddrs};
use std::sync::{Mutex, MutexGuard, PoisonError, mpsc};
use std::time::{Duration, Instant};
use std::{mem, thread};

const TEST_TMP_FILE: &CStr = c"/tmp/swoole_core_test_file";
const TEST_STR: &str = "hello world, hello swoole\n";
const TEST_HTTP_DOMAIN: &str = "www.gov.cn";
const TEST_HTTP_EXPECT: &str = "Location: https://www.gov.cn/";
/// The size of the pin's `examples/test.jpg`, which the crate's tests cannot read.
const TEST_JPG_SIZE: usize = 218_787;
const SW_NUM_MILLION: i64 = 1_000_000;

// GoogleTest ran these one at a time: they share paths under /tmp, and `wait`
// collects any child of the process.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `swoole::test::spawn_exec` of a child that sleeps `seconds`, then exits 0.
fn spawn_exec(seconds: c_uint) -> libc::pid_t {
    let pid = unsafe { libc::fork() };
    assert_ne!(pid, -1, "fork");
    if pid == 0 {
        unsafe {
            libc::sleep(seconds);
            libc::_exit(0);
        }
    }
    pid
}

fn socklen<T>() -> libc::socklen_t {
    libc::socklen_t::try_from(mem::size_of::<T>()).unwrap()
}

/// `Address::assign(SW_SOCK_TCP, host, port, true)`: the name's first IPv4 address.
fn resolve(host: &str, port: u16) -> libc::sockaddr_in {
    let ip = (host, port)
        .to_socket_addrs()
        .unwrap()
        .find_map(|address| match address.ip() {
            IpAddr::V4(ip) => Some(ip),
            IpAddr::V6(_) => None,
        })
        .unwrap();
    let mut addr: libc::sockaddr_in = unsafe { mem::zeroed() };
    addr.sin_family = libc::sa_family_t::try_from(libc::AF_INET).unwrap();
    addr.sin_port = port.to_be();
    addr.sin_addr.s_addr = u32::from_ne_bytes(ip.octets());
    addr
}

/// `swoole::test::http_get_request`.
fn http_get_request(domain: &str, path: &str) -> String {
    format!(
        "GET {path} HTTP/1.1\r\nHost: {domain}\r\nConnection: close\r\nUser-Agent: Mozilla/5.0 (Windows NT \
         10.0; WOW64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/51.0.2704.106 Safari/537.36\r\n\r\n"
    )
}

/// Upstream's `uring_create_server_socket`. `Iouring::socket` is not in the build, so
/// the socket is socket(2)'s; `Iouring::bind` and `listen` are bind(2) and listen(2).
fn uring_create_server_socket(
    actual_server_addr: &mut libc::sockaddr_in,
    actual_server_addr_len: &mut libc::socklen_t,
) -> c_int {
    unsafe {
        let server_sock = libc::socket(libc::AF_INET, libc::SOCK_STREAM | libc::SOCK_NONBLOCK, 0);
        assert!(server_sock > 0, "socket");

        let mut server_addr: libc::sockaddr_in = mem::zeroed();
        server_addr.sin_family = libc::sa_family_t::try_from(libc::AF_INET).unwrap();
        server_addr.sin_addr.s_addr = u32::from_ne_bytes([127, 0, 0, 1]);
        server_addr.sin_port = 0;

        let retval = libc::bind(
            server_sock,
            (&raw const server_addr).cast(),
            socklen::<libc::sockaddr_in>(),
        );
        assert_eq!(retval, 0, "bind");

        assert_eq!(libc::listen(server_sock, 128), 0, "listen");

        let retval = libc::getsockname(
            server_sock,
            std::ptr::from_mut(actual_server_addr).cast(),
            actual_server_addr_len,
        );
        assert_eq!(retval, 0, "getsockname");
        server_sock
    }
}

#[test]
fn create() {
    let _serial = serial();
    let config = Config {
        entries: 4,
        workers: 65_536,
        ..Config::default()
    };
    with_config(config, || unsafe {
        let fd = uring::open_file(&HOST, TEST_TMP_FILE.as_ptr(), libc::O_CREAT, 0o666);
        assert!(fd >= 0, "open");
        assert_ne!(uring::close_fd(&HOST, fd), -1, "close");
    });
}

#[test]
fn open_and_close() {
    let _serial = serial();
    with_engine(|| unsafe {
        let test_file = c"/tmp/file_1";
        let fd = uring::open_file(&HOST, test_file.as_ptr(), libc::O_CREAT, 0o666);
        assert!(fd > 0, "open");

        assert_eq!(uring::close_fd(&HOST, fd), 0, "close");

        assert_eq!(uring::unlink(&HOST, test_file.as_ptr()), 0, "unlink");
    });
}

#[test]
fn mkdir_and_rmdir() {
    let _serial = serial();
    with_engine(|| unsafe {
        let directory = c"/tmp/aaaa";

        assert_eq!(uring::mkdir(&HOST, directory.as_ptr(), 0o755), 0, "mkdir");

        assert_eq!(uring::rmdir(&HOST, directory.as_ptr()), 0, "rmdir");
    });
}

#[test]
fn write_and_read() {
    let _serial = serial();
    with_engine(|| unsafe {
        let test_file = c"/tmp/file_2";
        let fd = uring::open_file(
            &HOST,
            test_file.as_ptr(),
            libc::O_CREAT | libc::O_RDWR,
            0o666,
        );
        assert!(fd > 0, "open");

        let data = b"aaaaaaaaaaaaaaaaaaaaaaa";
        let length = data.len();
        let result = uring::write(&HOST, fd, data.as_ptr().cast(), length, -1.0);
        assert!(result > 0, "write");
        assert_eq!(result, length.cast_signed(), "written");

        libc::lseek(fd, 0, libc::SEEK_SET);

        let mut buf = [0_u8; 128];
        let result = uring::read(&HOST, fd, buf.as_mut_ptr().cast(), 128, -1.0);
        assert!(result > 0, "read");
        assert_eq!(result, length.cast_signed(), "read back");
        assert_eq!(&buf[..length], data, "contents");

        assert_eq!(uring::close_fd(&HOST, fd), 0, "close");

        assert_eq!(uring::unlink(&HOST, test_file.as_ptr()), 0, "unlink");
    });
}

#[test]
fn rename() {
    let _serial = serial();
    with_engine(|| unsafe {
        let oldpath = c"/tmp/file_2";
        let newpath = c"/tmp/file_3";
        let fd = uring::open_file(&HOST, oldpath.as_ptr(), libc::O_CREAT | libc::O_RDWR, 0o666);
        assert!(fd > 0, "open");

        assert_eq!(uring::close_fd(&HOST, fd), 0, "close");

        let result = uring::rename(&HOST, oldpath.as_ptr(), newpath.as_ptr());
        assert_eq!(result, 0, "rename");

        assert_eq!(uring::unlink(&HOST, newpath.as_ptr()), 0, "unlink");
    });
}

#[test]
fn fstat_and_stat() {
    let _serial = serial();
    with_engine(|| unsafe {
        let mut statbuf: libc::stat = mem::zeroed();
        let fd = uring::open_file(
            &HOST,
            TEST_TMP_FILE.as_ptr(),
            libc::O_CREAT | libc::O_RDWR,
            0o666,
        );
        assert!(fd > 0, "open");

        let written = uring::write(&HOST, fd, TEST_STR.as_ptr().cast(), TEST_STR.len(), -1.0);
        assert_eq!(written, TEST_STR.len().cast_signed(), "write");

        assert_eq!(uring::fstat(&HOST, fd, &raw mut statbuf), 0, "fstat");
        assert!(statbuf.st_size > 0, "fstat size");

        assert_eq!(uring::close_fd(&HOST, fd), 0, "close");

        statbuf = mem::zeroed();
        let result = uring::stat(&HOST, TEST_TMP_FILE.as_ptr(), &raw mut statbuf);
        assert_eq!(result, 0, "stat");
        assert!(statbuf.st_size > 0, "stat size");
    });
}

#[test]
fn fsync_and_fdatasync() {
    let _serial = serial();
    with_engine(|| unsafe {
        let test_file = c"/tmp/file_2";
        let fd = uring::open_file(
            &HOST,
            test_file.as_ptr(),
            libc::O_CREAT | libc::O_RDWR,
            0o666,
        );
        assert!(fd > 0, "open");

        let data = b"aaaaaaaaaaaaaaaaaaaaaaa";
        let length = data.len();
        let write_length = uring::write(&HOST, fd, data.as_ptr().cast(), length, -1.0);
        assert_eq!(write_length, length.cast_signed(), "write");

        assert_eq!(uring::fsync(&HOST, fd), 0, "fsync");

        let write_length = uring::write(&HOST, fd, data.as_ptr().cast(), length, -1.0);
        assert_eq!(write_length, length.cast_signed(), "write");

        assert_eq!(uring::fdatasync(&HOST, fd), 0, "fdatasync");

        assert_eq!(uring::close_fd(&HOST, fd), 0, "close");

        assert_eq!(uring::unlink(&HOST, test_file.as_ptr()), 0, "unlink");
    });
}

#[test]
fn ftruncate() {
    let _serial = serial();
    with_engine(|| unsafe {
        let test_file = c"/tmp/file_3";
        let fd = uring::open_file(
            &HOST,
            test_file.as_ptr(),
            libc::O_CREAT | libc::O_RDWR,
            0o666,
        );
        assert!(fd > 0, "open");

        let data = b"aaaaaaaaaaaaaaaaaaaaaaa";
        let length = data.len();
        let write_length = uring::write(&HOST, fd, data.as_ptr().cast(), length, -1.0);
        assert_eq!(write_length, length.cast_signed(), "write");

        assert_eq!(uring::ftruncate(&HOST, fd, 0), 0, "ftruncate");
        let _closed = uring::close_fd(&HOST, fd);
    });
}

/// `connect` and `send_recv`: a GET to `TEST_HTTP_DOMAIN` over the ring, with the
/// stream calls (`write`/`read`) or the socket ones (`send`/`recv`).
fn http_get(socket_calls: bool) {
    with_engine(|| unsafe {
        let fd = libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0);
        assert_ne!(fd, -1, "socket");

        let addr = resolve(TEST_HTTP_DOMAIN, 80);

        let _connected = uring::connect(
            &HOST,
            fd,
            (&raw const addr).cast(),
            socklen::<libc::sockaddr_in>(),
            -1.0,
        );

        let req = http_get_request(TEST_HTTP_DOMAIN, "/");
        let rv = if socket_calls {
            uring::send(&HOST, fd, req.as_ptr().cast(), req.len(), 0, -1.0)
        } else {
            uring::write(&HOST, fd, req.as_ptr().cast(), req.len(), -1.0)
        };
        assert_eq!(rv, req.len().cast_signed(), "request sent");

        let mut buf = [0_u8; 4096];
        let rv = if socket_calls {
            uring::recv(&HOST, fd, buf.as_mut_ptr().cast(), buf.len(), 0, -1.0)
        } else {
            uring::read(&HOST, fd, buf.as_mut_ptr().cast(), buf.len(), -1.0)
        };
        assert!(rv > 100, "response");

        let response = String::from_utf8_lossy(&buf[..rv.unsigned_abs()]);
        assert!(response.contains(TEST_HTTP_EXPECT), "{response}");

        let _closed = uring::close_fd(&HOST, fd);
    });
}

#[test]
#[ignore = "external DNS and network (www.gov.cn)"]
fn connect() {
    let _serial = serial();
    http_get(false);
}

#[test]
#[ignore = "external DNS and network (www.gov.cn)"]
fn send_recv() {
    let _serial = serial();
    http_get(true);
}

#[test]
fn sendfile() {
    let _serial = serial();
    let path = std::env::temp_dir().join(format!("swr-uring-sendfile-{}.jpg", std::process::id()));
    std::fs::write(&path, vec![0xa5_u8; TEST_JPG_SIZE]).unwrap();
    let file = std::fs::File::open(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    let file_fd = std::os::fd::AsRawFd::as_raw_fd(&file);

    let mut pairs = [0; 2];
    unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, pairs.as_mut_ptr()) };
    let out_fd = pairs[0];

    // Upstream's sending coroutine. Its read below is one read of the whole file, so
    // the file fits in the socket's buffers: the case reads once the send returns.
    let (sent_tx, sent) = mpsc::channel();
    thread::spawn(move || {
        with_engine(|| {
            let mut offset: libc::off_t = 0;
            let rv = uring::sendfile(
                &HOST,
                out_fd,
                file_fd,
                Some(&mut offset),
                TEST_JPG_SIZE,
                -1.0,
                0,
            );
            sent_tx.send(rv).unwrap();
        });
    });
    let rv = sent.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(rv, TEST_JPG_SIZE.cast_signed(), "sendfile");

    with_engine(|| unsafe {
        let mut data = vec![0_u8; 250_000];
        let result = uring::read(&HOST, pairs[1], data.as_mut_ptr().cast(), 250_000, -1.0);
        let _closed = uring::close_fd(&HOST, pairs[1]);
        assert_eq!(result, TEST_JPG_SIZE.cast_signed(), "read");
    });
    drop(file);
}

#[test]
fn accept() {
    let _serial = serial();
    with_engine(|| unsafe {
        let mut actual_server_addr: libc::sockaddr_in = mem::zeroed();
        let mut actual_server_addr_len = socklen::<libc::sockaddr_in>();
        let server_sock =
            uring_create_server_socket(&mut actual_server_addr, &mut actual_server_addr_len);

        let mut client_addr: libc::sockaddr_in = mem::zeroed();
        let mut client_addr_len = socklen::<libc::sockaddr_in>();

        let client = thread::spawn(move || {
            with_engine(|| {
                // Give the server time to start listening
                let _slept = uring::sleep(&HOST, 0.01);

                let client_sock = libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0);
                assert!(client_sock > 0, "socket");

                let retval = uring::connect(
                    &HOST,
                    client_sock,
                    (&raw const actual_server_addr).cast(),
                    actual_server_addr_len,
                    -1.0,
                );
                assert_eq!(retval, 0, "connect");

                let test_message = b"test_data";
                let sent_bytes = uring::send(
                    &HOST,
                    client_sock,
                    test_message.as_ptr().cast(),
                    test_message.len(),
                    0,
                    -1.0,
                );
                assert_eq!(sent_bytes, test_message.len().cast_signed(), "send");

                let _closed = uring::close_fd(&HOST, client_sock);
            });
        });

        let client_sock = uring::accept(
            &HOST,
            server_sock,
            (&raw mut client_addr).cast(),
            &raw mut client_addr_len,
            0,
            -1.0,
        );
        assert!(client_sock > 0, "accept");

        let mut buffer = [0_u8; 256];
        let received_bytes = uring::recv(
            &HOST,
            client_sock,
            buffer.as_mut_ptr().cast(),
            buffer.len() - 1,
            0,
            -1.0,
        );
        assert!(received_bytes > 0, "recv");
        assert_eq!(
            CStr::from_bytes_until_nul(&buffer).unwrap(),
            c"test_data",
            "received"
        );

        let _closed = uring::close_fd(&HOST, client_sock);
        let _closed = uring::close_fd(&HOST, server_sock);
        client.join().unwrap();
    });
}

#[test]
fn sleep() {
    let _serial = serial();
    with_engine(|| {
        let begin = Instant::now();
        let ts = sys::__kernel_timespec {
            tv_sec: 1,
            tv_nsec: 200 * SW_NUM_MILLION,
        };
        assert_eq!(uring::sleep_timespec(&HOST, ts), 0, "sleep(1, 200ms)");
        assert!(begin.elapsed().as_secs_f64() >= 1.2, "slept 1.2s");

        let begin = Instant::now();
        let ts = sys::__kernel_timespec {
            tv_sec: 0,
            tv_nsec: 300 * SW_NUM_MILLION,
        };
        assert_eq!(uring::sleep_timespec(&HOST, ts), 0, "sleep(0, 300ms)");
        assert!(begin.elapsed().as_secs_f64() >= 0.3, "slept 0.3s");
    });
}

/// `Iouring::wait(stat_loc, timeout)`, which is `waitpid(-1, stat_loc, 0, timeout)`.
fn wait(status: &mut c_int, timeout: f64) -> libc::pid_t {
    uring::waitpid(&HOST, -1, Some(status), 0, timeout)
}

#[test]
fn wait_success() {
    let _serial = serial();
    let pid = spawn_exec(1);

    with_engine(|| {
        let mut status = 0;
        assert_eq!(wait(&mut status, 5.0), pid, "waited");
        assert_eq!(status, 0, "status");
    });
}

#[test]
fn wait_timeout() {
    let _serial = serial();
    let pid = spawn_exec(2000);

    with_engine(|| {
        let mut status = 0x9501;
        assert_eq!(wait(&mut status, 0.5), -1, "timed out");
        assert_eq!(uring::errno(), libc::ETIMEDOUT, "ETIMEDOUT");
        // After the timeout, the status will not be set.
        assert_eq!(status, 0x9501, "status untouched");
    });

    unsafe { libc::kill(pid, libc::SIGKILL) };
    // GoogleTest's next case waited for its own pid; here `wait_success` may follow.
    unsafe { libc::waitpid(pid, std::ptr::null_mut(), 0) };
}

#[test]
fn waitpid() {
    let _serial = serial();
    let pid = spawn_exec(2000);

    with_engine(|| unsafe {
        let mut status = 0;
        assert_eq!(
            uring::waitpid(&HOST, pid, Some(&mut status), libc::WNOHANG, -1.0),
            0,
            "WNOHANG"
        );
        assert_eq!(
            uring::waitpid(&HOST, pid, Some(&mut status), 0, 0.1),
            -1,
            "timed out"
        );
        assert_eq!(uring::errno(), libc::ETIMEDOUT, "ETIMEDOUT");

        libc::kill(pid, libc::SIGKILL);
        let _slept = uring::sleep(&HOST, 0.3);
        assert_eq!(
            uring::waitpid(&HOST, pid, Some(&mut status), 0, 0.1),
            pid,
            "reaped"
        );
    });
}

#[test]
fn poll() {
    let _serial = serial();
    // sw_tg_buffer()->append_random_bytes(256 * 1024, false)
    let mut buffer = vec![0_u8; 256 * 1024];
    std::fs::File::open("/dev/urandom")
        .unwrap()
        .read_exact(&mut buffer)
        .unwrap();

    with_engine(|| unsafe {
        let mut pair = [0; 2];
        let mut fds: [libc::pollfd; 2] = mem::zeroed();
        let mut buf = [0_u8; 4096];
        libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, pair.as_mut_ptr());

        let (fd0, fd1) = (pair[0], pair[1]);

        fds[0] = mem::zeroed();
        fds[0].fd = fd0;
        fds[0].events = libc::POLLIN | libc::POLLOUT;
        assert_eq!(uring::poll(&HOST, &mut fds[0], 1000), 1, "writable");
        assert_ne!(fds[0].revents & libc::POLLOUT, 0, "POLLOUT");

        let writer = thread::spawn(move || {
            with_engine(|| {
                let result = uring::write(&HOST, fd0, buffer.as_ptr().cast(), buffer.len(), -1.0);
                assert!(result > 0, "write");
                let _slept = uring::sleep(&HOST, 0.1);
                let result = uring::write(&HOST, fd1, buffer.as_ptr().cast(), 16 * 1024, -1.0);
                assert!(result > 0, "write back");
            });
        });

        fds[0] = mem::zeroed();
        fds[0].fd = fd1;
        fds[0].events = libc::POLLIN;

        assert_eq!(uring::poll(&HOST, &mut fds[0], 1000), 1, "readable");
        assert_ne!(fds[0].revents & libc::POLLIN, 0, "POLLIN");

        let result = uring::read(&HOST, fd1, buf.as_mut_ptr().cast(), buf.len(), -1.0);
        assert!(result > 1024, "read");

        // `poll(fds, 2, 1000)`: the engine's poll takes one pollfd (README).

        let _slept = uring::sleep(&HOST, 0.02);

        fds[0] = mem::zeroed();
        fds[0].fd = fd0;
        fds[0].events = libc::POLLIN | libc::POLLOUT;
        assert_eq!(uring::poll(&HOST, &mut fds[0], 1000), 1, "ready");
        assert_ne!(fds[0].revents & libc::POLLIN, 0, "POLLIN");
        writer.join().unwrap();
    });
}

#[test]
fn accept_timeout() {
    let _serial = serial();
    with_engine(|| unsafe {
        let mut actual_server_addr: libc::sockaddr_in = mem::zeroed();
        let mut actual_server_addr_len = socklen::<libc::sockaddr_in>();
        let server_sock =
            uring_create_server_socket(&mut actual_server_addr, &mut actual_server_addr_len);

        let mut client_addr: libc::sockaddr_in = mem::zeroed();
        let mut client_addr_len = socklen::<libc::sockaddr_in>();

        let client_sock = uring::accept(
            &HOST,
            server_sock,
            (&raw mut client_addr).cast(),
            &raw mut client_addr_len,
            0,
            0.1,
        );
        assert_eq!(client_sock, -1, "timed out");

        let _closed = uring::close_fd(&HOST, server_sock);
    });
}
