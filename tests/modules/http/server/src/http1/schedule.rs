//! Upstream's `core-tests/src/server/server.cpp` `schedule*` TESTs, one Rust function per
//! TEST with the same inputs and expected values: a created process-mode Server's
//! `schedule_worker` (crates/server `dispatch.rs`) under each dispatch mode, over fake
//! connections. Nothing starts.

use super::*;
use std::collections::BTreeSet;
use std::sync::atomic::Ordering::Relaxed;
use swoole_rs_corebase::net_types::AddressStorage;
use swoole_rs_process::worker_record::WorkerStatus::{
    Busy as SW_WORKER_BUSY, Idle as SW_WORKER_IDLE,
};

const SW_SOCK_UDP: c_int = 2;
const SW_SOCK_TCP6: c_int = 3;
const DISPATCH_ROUND: u8 = 1;
const DISPATCH_IPMOD: u8 = 4;
const DISPATCH_UIDMOD: u8 = 5;
const DISPATCH_CO_CONN_LB: u8 = 8;
const DISPATCH_CO_REQ_LB: u8 = 9;
const DISPATCH_CONCURRENT_LB: u8 = 10;

/// `DgramPacket` up to its address.
#[repr(C)]
struct DgramPacket {
    socket_type: c_int,
    socket_addr: Address,
}

unsafe extern "C" {
    #[link_name = "_Z11swoole_randii"]
    fn swoole_rand(min: c_int, max: c_int) -> c_int;
}

/// `network::Address::assign(type, host, port, false)`, as swoole_socket.h's inline does.
fn assign(address: &mut Address, kind: c_int, host: &str, port: c_int) {
    address.kind = kind;
    let host = CString::new(host).unwrap();
    // SAFETY: the record's union, which is `AddressStorage`'s size and alignment.
    let storage = unsafe { &mut *(&raw mut address.addr).cast::<AddressStorage>() };
    let host = host.as_bytes_with_nul();
    let error =
        swoole_rs_clients::address::assign(storage, &mut address.len, kind, host, port, false);
    assert_eq!(error, 0);
}

/// `network::Socket fake_sock{}; fake_sock.fd = fd`, its `info` assigned when `address`
/// is given. It leaks, as the stack socket outlives every schedule.
fn fake_sock(fd: c_int, address: Option<(c_int, &str, c_int)>) -> *mut c_void {
    // SAFETY: a new socket record of `fd`.
    let sock = unsafe { case::make_socket(fd, 0) }.cast::<c_void>();
    if let Some((kind, host, port)) = address {
        // SAFETY: the socket's `info` (case.rs's SOCKET_INFO), an `Address`.
        assign(
            unsafe { &mut *sock.cast::<u8>().add(case::SOCKET_INFO).cast::<Address>() },
            kind,
            host,
            port,
        );
    }
    sock
}

/// `port->get_fd()`: the fd heads the port's socket record.
fn port_fd(port: Port) -> c_int {
    // SAFETY: a listening port's live socket.
    unsafe { *port.config().socket.cast::<c_int>() }
}

/// `serv.add_connection(port, sock, port->get_fd())`.
fn add_connection(serv: Server, port: Port, sock: *mut c_void) -> &'static mut Connection {
    let serv_view = serv.view();
    let listen = serv_view
        .ports()
        .iter()
        .find(|p| p.raw() == port.0)
        .unwrap();
    // SAFETY: a live socket.
    let sock = unsafe { swoole_rs_ffi::Socket::from_ptr(sock.cast()) }.unwrap();
    let conn = server_methods::add_connection(serv_view, *listen, sock, port_fd(port)).unwrap();
    // SAFETY: the new connection, live as long as the server.
    unsafe { &mut *conn.as_ptr().cast::<Connection>() }
}

fn schedule_worker(serv: Server, fd: c_int, data: Option<&swoole_rs_server::SendData>) -> usize {
    let worker_id = server_methods::schedule_worker(serv.view(), fd, data);
    usize::try_from(worker_id).unwrap()
}

use swoole_rs_process::worker_record::WorkerRecord;

type Worker = swoole_rs_process::worker_record::Worker<'static>;

/// `serv.workers[i]`: the event worker array.
fn worker(serv: Server, id: usize) -> Worker {
    assert!(id < serv.config().worker_num as usize);
    let workers = serv.config().workers.cast::<WorkerRecord>();
    // SAFETY: the created server's `worker_num` event workers.
    unsafe { Worker::new(workers.add(id)) }
}

/// A process-mode Server of `worker_num` workers and `dispatch_mode`, listening on
/// `TEST_HOST` (and `::` when `ipv6`), created.
fn create(worker_num: u32, dispatch_mode: u8, ipv6: bool) -> (Server, Port, Option<Port>) {
    let serv = Server::new(MODE_PROCESS);
    serv.config().worker_num = worker_num;
    serv.config().dispatch_mode = dispatch_mode;
    let port = serv.add_port(SW_SOCK_TCP, TEST_HOST, 0);
    assert!(port.is_some());
    let port6 = ipv6.then(|| serv.add_port(SW_SOCK_TCP6, "::", 0));
    if let Some(port6) = port6 {
        assert!(port6.is_some());
    }
    assert_eq!(SW_OK, serv.create());
    (serv, port.unwrap(), port6.flatten())
}

/// `TEST(server, schedule)`.
fn schedule() {
    let (serv, _, _) = create(6, DISPATCH_IDLE_WORKER, false);
    let worker_num = serv.config().worker_num as usize;

    for i in 0..worker_num {
        worker(serv, i).set_status(SW_WORKER_BUSY);
    }

    let mut worker_id_set = BTreeSet::new();
    for i in 0..worker_num {
        worker_id_set.insert(schedule_worker(serv, (i * 13) as c_int, None));
    }
    assert_eq!(worker_id_set.len(), worker_num);

    for i in 1..worker_num - 1 {
        worker(serv, i).set_status(SW_WORKER_IDLE);
    }

    worker_id_set.clear();
    for i in 0..worker_num {
        worker_id_set.insert(schedule_worker(serv, (i * 13) as c_int, None));
    }
    assert_eq!(worker_id_set.len(), worker_num - 2);

    server_methods::destroy(serv.view());
    serv.delete();
}

/// `TEST(server, schedule_1)`.
fn schedule_1() {
    let (serv, _, _) = create(8, DISPATCH_ROUND, false);
    let worker_num = serv.config().worker_num as usize;

    let schedule_count = 1024;
    let mut counter = vec![0usize; worker_num];
    for i in 0..schedule_count {
        counter[schedule_worker(serv, (i * 13) as c_int, None)] += 1;
    }

    for count in counter {
        assert_eq!(count, schedule_count / worker_num);
    }
    serv.delete();
}

fn average_combined(v1: &[usize], v2: &[usize]) -> f64 {
    let total_size = v1.len() + v2.len();
    if total_size == 0 {
        return 0.0;
    }
    (v1.iter().sum::<usize>() + v2.iter().sum::<usize>()) as f64 / total_size as f64
}

/// `test_worker_schedule<Worker, M, member>(dispatch_mode)`: `member` is the worker's
/// field the mode balances, read by `get` and written by `set`.
fn test_worker_schedule(dispatch_mode: u8, get: fn(Worker) -> usize, set: fn(Worker, usize)) {
    let (serv, port, _) = create(8, dispatch_mode, false);
    let worker_num = serv.config().worker_num as usize;

    let mut counter = vec![0usize; worker_num];
    let schedule_count = 256 * worker_num;
    let mut init_counter = vec![0usize; worker_num];

    for (i, init) in init_counter.iter_mut().enumerate() {
        // SAFETY: random in a non-empty range.
        *init = usize::try_from(unsafe { swoole_rand(32, 512) }).unwrap();
        set(worker(serv, i), *init);
    }

    let sock = fake_sock(199, None);
    add_connection(serv, port, sock);

    for _ in 0..schedule_count {
        let worker_id = schedule_worker(serv, 199, None);
        counter[worker_id] += 1;
        let worker = worker(serv, worker_id);
        set(worker, get(worker) + 1);
    }

    let avg_elem = average_combined(&init_counter, &counter);
    for i in 0..worker_num {
        // The starting loads are random; on a failure, print them so the draw can be
        // replayed against upstream's scheduler (#3281).
        assert!(
            (counter[i] + init_counter[i]) as i64 >= avg_elem as i64 * 2 - 10,
            "worker {i}: started {init_counter:?}, scheduled {counter:?}, avg_elem {avg_elem}"
        );
    }
    serv.delete();
}

/// `TEST(server, schedule_4)`.
fn schedule_4() {
    let (serv, port, port6) = create(4, DISPATCH_IPMOD, true);
    let port6 = port6.unwrap();
    let worker_num = serv.config().worker_num as usize;

    let mut counter = vec![0usize; worker_num];
    let schedule_count = 256 * worker_num;

    let sock = fake_sock(100, Some((SW_SOCK_TCP, "127.0.0.1", 9501)));
    add_connection(serv, port, sock);
    for _ in 0..schedule_count {
        counter[schedule_worker(serv, 100, None)] += 1;
    }

    let sock6 = fake_sock(101, Some((SW_SOCK_TCP6, "::1", 9502)));
    add_connection(serv, port6, sock6);
    for _ in 0..schedule_count {
        counter[schedule_worker(serv, 101, None)] += 1;
    }

    // SAFETY: the thread's buffer, which holds a `DgramPacket`.
    let pkt = unsafe { (*case::swr_case_tg_buffer()).str }.cast::<DgramPacket>();
    // SAFETY: as above.
    assign(
        unsafe { &mut (*pkt).socket_addr },
        SW_SOCK_UDP,
        "192.168.1.103",
        29321,
    );
    let sdata = swoole_rs_server::SendData {
        // Upstream leaves the head unset and reads none of it.
        info: Default::default(),
        data: pkt.cast(),
    };
    counter[schedule_worker(serv, 9999, Some(&sdata))] += 1;

    assert_eq!(counter[0], 0);
    assert_eq!(counter[1], schedule_count + 1);
    assert_eq!(counter[2], 0);
    assert_eq!(counter[3], schedule_count);
    serv.delete();
}

/// `TEST(server, schedule_5)`.
fn schedule_5() {
    let (serv, port, port6) = create(4, DISPATCH_UIDMOD, true);
    let port6 = port6.unwrap();
    let worker_num = serv.config().worker_num as usize;

    let mut counter = vec![0usize; worker_num];
    let schedule_count = 256 * worker_num;

    let sock = fake_sock(100, Some((SW_SOCK_TCP, "127.0.0.1", 9501)));
    add_connection(serv, port, sock).uid = 0;
    for _ in 0..schedule_count {
        counter[schedule_worker(serv, 100, None)] += 1;
    }

    let sock6 = fake_sock(101, Some((SW_SOCK_TCP6, "::1", 9502)));
    add_connection(serv, port6, sock6).uid = 839922;
    for _ in 0..schedule_count {
        counter[schedule_worker(serv, 101, None)] += 1;
    }

    assert_eq!(counter[0], schedule_count);
    assert_eq!(counter[1], 0);
    assert_eq!(counter[2], schedule_count);
    assert_eq!(counter[3], 0);
    serv.delete();
}

/// `TEST(server, schedule_8)`.
fn schedule_8() {
    let (serv, port, port6) = create(4, DISPATCH_CO_CONN_LB, true);
    let port6 = port6.unwrap();
    let worker_num = serv.config().worker_num as usize;

    let mut counter = vec![0usize; worker_num];
    let schedule_count = 256 * worker_num;

    let sock = fake_sock(100, Some((SW_SOCK_TCP, "127.0.0.1", 9501)));
    add_connection(serv, port, sock).worker_id = 1;
    for _ in 0..schedule_count {
        counter[schedule_worker(serv, 100, None)] += 1;
    }

    let sock6 = fake_sock(101, Some((SW_SOCK_TCP6, "::1", 9502)));
    add_connection(serv, port6, sock6);
    for _ in 0..schedule_count {
        counter[schedule_worker(serv, 101, None)] += 1;
    }

    counter[schedule_worker(serv, 9999, None)] += 1;

    assert_eq!(counter[0], schedule_count + 1);
    assert_eq!(counter[1], schedule_count);
    assert_eq!(counter[2], 0);
    assert_eq!(counter[3], 0);
    serv.delete();
}

/// `TEST(server, schedule_9)`: `Worker::coroutine_num`, a `size_t`.
fn schedule_9() {
    test_worker_schedule(
        DISPATCH_CO_REQ_LB,
        // SAFETY: the live record's field.
        |worker| unsafe { (*worker.raw()).coroutine_num },
        // SAFETY: as above.
        |worker, value| unsafe { (*worker.raw()).coroutine_num = value },
    );
}

/// `TEST(server, schedule_10)`: `Worker::concurrency`, a `uint32_t`.
fn schedule_10() {
    test_worker_schedule(
        DISPATCH_CONCURRENT_LB,
        // SAFETY: the live record's field.
        |worker| unsafe { &(*worker.raw()).concurrency }.load(Relaxed) as usize,
        // SAFETY: as above.
        |worker, value| {
            unsafe { &(*worker.raw()).concurrency }.store(u32::try_from(value).unwrap(), Relaxed);
        },
    );
}

pub const CASES: &[(&str, fn())] = &[
    ("schedule", schedule),
    ("schedule_1", schedule_1),
    ("schedule_4", schedule_4),
    ("schedule_5", schedule_5),
    ("schedule_8", schedule_8),
    ("schedule_9", schedule_9),
    ("schedule_10", schedule_10),
];
