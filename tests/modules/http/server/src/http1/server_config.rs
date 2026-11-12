//! Upstream's `core-tests/src/server/server.cpp` connection and configuration TESTs (#2536),
//! one Rust function per TEST with the same inputs and expected values, over the crates'
//! `Server` (crates/server). `server.protocols` needs only a port, so it is crates/ports'
//! `methods::tests::server_protocols`.

use super::*;

const SW_MAX_CONNECTION: u32 = 100_000;
const SW_SESSION_LIST_SIZE: u32 = 1 << 20;
const SW_MAX_THREAD_NCPU: u32 = 4;
const SW_MAX_WORKER_NCPU: u32 = 1000;
const SW_SOCK_UDP: c_int = 2;
const SW_LOG_NONE: c_int = 6;
const SW_ERROR_SERVER_INVALID_CALLBACK: c_int = 9015;
const CB_TASK: usize = 4;

unsafe extern "C-unwind" {
    /// `SwooleG`, whose first member is `max_sockets`.
    #[link_name = "SwooleG"]
    static mut SWOOLE_G_MAX_SOCKETS: u32;
    /// `SW_CPU_NUM`.
    fn swr_v1_core_cpu_num() -> u16;
    #[link_name = "_Z20swoole_get_log_levelv"]
    fn swoole_get_log_level() -> c_int;
}

fn max_sockets() -> u32 {
    // SAFETY: SwooleG, which only this thread uses here.
    unsafe { SWOOLE_G_MAX_SOCKETS }
}

fn set_max_sockets(value: u32) {
    // SAFETY: as above.
    unsafe { SWOOLE_G_MAX_SOCKETS = value };
}

fn cpu_num() -> u32 {
    // SAFETY: reads swoole_init's CPU count.
    u32::from(unsafe { swr_v1_core_cpu_num() })
}

impl Server {
    fn set_max_connection(self, max: u32) {
        server_methods::set_max_connection(self.view(), max);
    }

    fn get_max_connection(self) -> u32 {
        // SAFETY: the state's tables live as long as the server.
        unsafe { (*self.head().tables).max_connection }
    }

    fn get_startup_error_message(self) -> String {
        server_methods::get_startup_error_message()
            .to_string_lossy()
            .into_owned()
    }
}

unsafe extern "C" fn return_ok(_: *mut c_void, _: *mut c_void, _: *mut c_void) -> c_int {
    SW_OK
}

/// `TEST(server, max_connection)`.
fn max_connection() {
    let serv = Server::new(MODE_BASE);

    let ori_max_sockets = max_sockets();

    serv.set_max_connection(0);
    assert_eq!(
        serv.get_max_connection(),
        SW_MAX_CONNECTION.min(max_sockets())
    );

    serv.set_max_connection(max_sockets() + 13);
    assert_eq!(serv.get_max_connection(), max_sockets());

    serv.set_max_connection(max_sockets() - 13);
    assert_eq!(serv.get_max_connection(), max_sockets() - 13);

    set_max_sockets(SW_SESSION_LIST_SIZE + 1024);
    serv.set_max_connection(SW_SESSION_LIST_SIZE + 999);
    assert_eq!(serv.get_max_connection(), SW_SESSION_LIST_SIZE);
    set_max_sockets(ori_max_sockets);

    let last_value = serv.get_max_connection();

    assert!(serv.add_port(SW_SOCK_TCP, TEST_HOST, 0).is_some());

    serv.create();

    serv.set_max_connection(100);
    assert_eq!(serv.get_max_connection(), last_value);
    serv.delete();
}

/// `TEST(server, min_connection)`.
fn min_connection() {
    let serv = Server::new(MODE_BASE);

    serv.config().task_worker_num = 14;
    serv.config().worker_num = 5;

    serv.add_port(SW_SOCK_TCP, TEST_HOST, 0);

    serv.set_max_connection(15);
    serv.create();
    assert_eq!(serv.get_max_connection(), max_sockets());
    serv.delete();
}

/// `TEST(server, worker_num)`.
fn worker_num() {
    let serv = Server::new(MODE_BASE);

    serv.config().worker_num = cpu_num() * SW_MAX_WORKER_NCPU + 99;
    serv.config().task_worker_num = cpu_num() * SW_MAX_WORKER_NCPU + 99;

    assert!(serv.add_port(SW_SOCK_TCP, TEST_HOST, 0).is_some());

    serv.create();

    assert_eq!(serv.config().worker_num, cpu_num() * SW_MAX_WORKER_NCPU);
    assert_eq!(
        serv.config().task_worker_num,
        cpu_num() * SW_MAX_WORKER_NCPU
    );
    serv.delete();
}

/// `serv.reactor_num = SW_CPU_NUM * SW_MAX_THREAD_NCPU + 99`.
fn too_many_reactors() -> u16 {
    u16::try_from(cpu_num() * SW_MAX_THREAD_NCPU + 99).unwrap()
}

/// `TEST(server, reactor_num_base)`.
fn reactor_num_base() {
    let serv = Server::new(MODE_BASE);
    serv.config().reactor_num = too_many_reactors();
    assert!(serv.add_port(SW_SOCK_TCP, TEST_HOST, 0).is_some());
    serv.create();

    assert_eq!(
        u32::from(serv.config().reactor_num),
        serv.config().worker_num
    );
    serv.delete();
}

/// `TEST(server, reactor_num_large)`.
fn reactor_num_large() {
    let serv = Server::new(MODE_PROCESS);
    serv.config().worker_num = cpu_num() * SW_MAX_WORKER_NCPU;
    serv.config().reactor_num = too_many_reactors();
    assert!(serv.add_port(SW_SOCK_TCP, TEST_HOST, 0).is_some());
    serv.create();

    assert_eq!(
        u32::from(serv.config().reactor_num),
        cpu_num() * SW_MAX_THREAD_NCPU
    );
    serv.delete();
}

/// `TEST(server, reactor_num_large2)`.
fn reactor_num_large2() {
    let serv = Server::new(MODE_PROCESS);
    serv.config().reactor_num = too_many_reactors();
    assert!(serv.add_port(SW_SOCK_TCP, TEST_HOST, 0).is_some());
    serv.create();

    assert_eq!(
        u32::from(serv.config().reactor_num),
        serv.config().worker_num
    );
    serv.delete();
}

/// `TEST(server, reactor_num_zero)`.
fn reactor_num_zero() {
    let serv = Server::new(MODE_BASE);
    serv.config().reactor_num = 0;
    assert!(serv.add_port(SW_SOCK_TCP, TEST_HOST, 0).is_some());
    serv.create();

    assert_eq!(u32::from(serv.config().reactor_num), cpu_num());
    serv.delete();
}

/// `TEST(server, startup_error)`. The `new` Server leaks, as upstream's.
fn startup_error() {
    let server = Server::new(MODE_PROCESS);
    server.config().task_worker_num = 2;

    assert!(server.add_port(SW_SOCK_TCP, TEST_HOST, 0).is_some());
    assert!(server.add_port(SW_SOCK_UDP, TEST_HOST, 0).is_some());
    assert_eq!(server.create(), 0);

    assert_eq!(server.start(), -1);
    let startup_error = server.get_startup_error_message();
    assert!(startup_error.contains("require 'onTask' callback"));
    assert_eq!(last_error(), SW_ERROR_SERVER_INVALID_CALLBACK);

    server.set_callback(CB_TASK, return_ok as *const c_void);

    assert_eq!(server.start(), -1);
    assert!(
        server
            .get_startup_error_message()
            .contains("require 'onReceive' callback")
    );

    // SAFETY: reads the global log level.
    let ori_log_level = unsafe { swoole_get_log_level() };
    // SAFETY: sets it.
    unsafe { swoole_set_log_level(SW_LOG_NONE) };

    assert_eq!(server.start(), -1);
    let startup_error2 = server.get_startup_error_message();
    assert_eq!(startup_error2, SW_ERROR_SERVER_INVALID_CALLBACK.to_string());
    assert_eq!(last_error(), SW_ERROR_SERVER_INVALID_CALLBACK);

    // SAFETY: restores it.
    unsafe { swoole_set_log_level(ori_log_level) };

    server.set_callback(CB_RECEIVE, return_ok as *const c_void);

    assert_eq!(server.start(), -1);
    assert!(
        server
            .get_startup_error_message()
            .contains("require 'onPacket' callback")
    );
}

pub const CASES: &[(&str, fn())] = &[
    ("max_connection", max_connection),
    ("min_connection", min_connection),
    ("worker_num", worker_num),
    ("reactor_num_base", reactor_num_base),
    ("reactor_num_large", reactor_num_large),
    ("reactor_num_large2", reactor_num_large2),
    ("reactor_num_zero", reactor_num_zero),
    ("startup_error", startup_error),
];
