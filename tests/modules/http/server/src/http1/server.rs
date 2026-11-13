//! Upstream's `core-tests/src/server/buffer.cpp`, one Rust function per TEST with the same
//! inputs and expected values, over crates/server.

use super::*;
use std::io::{Read, Write};

const PACKET: &[u8] = b"hello world\n";

/// `send_buffer`'s `onReceive`: the packet, answered with 16 MiB of `A` and a close.
unsafe extern "C" fn send_buffer_receive(
    _: *mut c_void,
    serv: *mut c_void,
    req: *mut RecvData,
) -> c_int {
    let serv = Server(serv);
    let (session, data) = request(req);
    expect(data == PACKET, "string(req->data, req->info.len) == packet");

    // `String resp(1024 * 1024 * 16); resp.repeat("A", 1, resp.capacity())`.
    let resp = vec![b'A'; 1024 * 1024 * 16];
    expect(
        serv.send(session, &resp),
        "serv->send(req->info.fd, resp.value(), resp.get_length())",
    );
    // SAFETY: the running server and the request's session.
    expect(
        unsafe { case::swr_case_close(serv.0, session) },
        "serv->close(req->info.fd, 0)",
    );
    SW_OK
}

/// `send_buffer`'s `onWorkerStart`, where upstream unlocks the mutex its client thread
/// waits on: the client starts here.
unsafe extern "C" fn send_buffer_worker_start(_: *mut c_void, serv: *mut c_void, _: *mut c_void) {
    let port = Server(serv).get_primary_port().port();
    // SAFETY: getpid(2).
    client_thread(
        || unsafe { getpid() },
        move || {
            block_all_signals();
            let mut c = std::net::TcpStream::connect((TEST_HOST, port)).unwrap();
            c.write_all(PACKET).unwrap();
            let mut buf = [0_u8; 4096];
            while matches!(c.read(&mut buf), Ok(retval) if retval > 0) {
                std::thread::sleep(Duration::from_micros(100));
            }
            drop(c);
            kill_self();
        },
    );
}

/// `TEST(server, send_buffer)`.
fn send_buffer() {
    let serv = Server::new(MODE_BASE);
    serv.config().worker_num = 1;

    set_log_warning();

    if serv.add_port(SW_SOCK_TCP, TEST_HOST, 0).is_none() {
        eprintln!("listen failed, [error={}]", last_error());
        std::process::exit(2);
    }

    assert_eq!(serv.create(), SW_OK);

    serv.set_callback(
        CallbackKind::WorkerStart,
        send_buffer_worker_start as *const c_void,
    );
    serv.set_callback(CallbackKind::Receive, send_buffer_receive as *const c_void);

    serv.start();
    join_client();
    serv.delete();
}

pub const CASES: &[(&str, fn())] = &[("send_buffer", send_buffer)];
