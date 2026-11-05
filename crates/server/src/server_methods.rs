//! The public `Server` methods of `src/server/master.cc` that were one operation
//! over a fresh context, defined under their upstream (Itanium) symbols when the
//! hybrid links this crate (`upstream-symbols`). Native code registers the
//! guarded runner (`server_run`) once; each method fills the context the way its
//! C++ forward did and runs over it.

use core::ffi::{c_char, c_int, c_long, c_void};
use core::ptr;
use std::sync::OnceLock;

use crate::ops::{
    RUN_ADD_CONNECTION, RUN_ADD_PORT, RUN_DISABLE_ACCEPT, RUN_FEEDBACK, RUN_IDLE_TASK_WORKERS,
    RUN_IDLE_WORKERS, RUN_REOPEN_LOGGER, RUN_SCHEDULE_WORKER, RUN_SET_MAX_CONNECTION, RUN_SHUTDOWN,
    RUN_ST_ABORT, RUN_ST_CLEAR_TIMER, RUN_ST_FOREACH, RUN_ST_STORE_PIPE_FD, RUN_STOP_MASTER_THREAD,
    RUN_SYSTEMD_SOCKETS, RUN_TASKING,
};

/// master.cc's `ServerRustContext`, which asserts this layout.
#[repr(C)]
pub struct NativeContext {
    serv: *mut c_void,
    reactor: *mut c_void,
    event: *mut c_void,
    port: *mut c_void,
    sock: *mut c_void,
    conn: *mut c_void,
    server_fd: c_int,
    reactor_id: u8,
    send: *mut c_void,
    data: *const c_void,
    length: u32,
    session_id: c_long,
    buffer: *mut c_char,
    offset: i64,
    socket_type: c_int,
    host: *const c_char,
    number: c_int,
    object: *mut c_void,
}

const _: () = assert!(
    size_of::<NativeContext>() == 136 && core::mem::offset_of!(NativeContext, object) == 128,
    "master.cc ServerRustContext layout"
);

impl NativeContext {
    fn new(serv: *mut c_void) -> Self {
        Self {
            serv,
            reactor: ptr::null_mut(),
            event: ptr::null_mut(),
            port: ptr::null_mut(),
            sock: ptr::null_mut(),
            conn: ptr::null_mut(),
            server_fd: 0,
            reactor_id: 0,
            send: ptr::null_mut(),
            data: ptr::null(),
            length: 0,
            session_id: 0,
            buffer: ptr::null_mut(),
            offset: 0,
            socket_type: 0,
            host: ptr::null(),
            number: 0,
            object: ptr::null_mut(),
        }
    }
}

/// master.cc's `server_run`: one operation over a context, native exception custody included.
pub type NativeRun = unsafe extern "C-unwind" fn(u32, u64, *mut NativeContext) -> i64;

static NATIVE: OnceLock<NativeRun> = OnceLock::new();

/// Registers master.cc's runner, which the methods below call.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_server_set_native(run: NativeRun) {
    // A second registration is the same function; keeping the first is equivalent.
    NATIVE.get_or_init(|| run);
}

/// Runs `op` over `ctx` through the registered runner.
///
/// # Safety
/// `ctx.serv` is a live `Server`, and the other fields are what `op` reads.
unsafe fn run(op: u32, arg: u64, ctx: &mut NativeContext) -> i64 {
    let Some(native) = NATIVE.get() else {
        // master.cc registers before any Server exists.
        std::process::abort()
    };
    // SAFETY: as the caller guarantees.
    unsafe { native(op, arg, ctx) }
}

/// Each method that is one operation over the server alone: its symbol, Rust
/// name, return type and operation. `bool` and `int` returns narrow like C++.
macro_rules! plain_methods {
    ($($symbol:literal fn $name:ident() -> $r:ty = $op:ident;)*) => {$(
        #[doc = concat!("`", $symbol, "`.")]
        ///
        /// # Safety
        /// `serv` is a live `Server`.
        #[cfg_attr(feature = "upstream-symbols", unsafe(export_name = $symbol))]
        pub unsafe extern "C" fn $name(serv: *mut c_void) -> $r {
            // SAFETY: as the caller guarantees.
            Narrow::narrow(unsafe { run($op, 0, &mut NativeContext::new(serv)) })
        }
    )*};
}

/// A method's C++ return type, from the runner's `i64`.
trait Narrow {
    fn narrow(value: i64) -> Self;
}

impl Narrow for () {
    fn narrow(_: i64) {}
}

impl Narrow for bool {
    fn narrow(value: i64) -> bool {
        value != 0
    }
}

impl Narrow for c_int {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the C++ method's (int) conversion of the runner's result"
    )]
    fn narrow(value: i64) -> c_int {
        value as c_int
    }
}

impl Narrow for u32 {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the C++ method's (uint32_t) conversion of the runner's result"
    )]
    fn narrow(value: i64) -> u32 {
        value as u32
    }
}

plain_methods! {
    "_ZN6swoole6Server14disable_acceptEv" fn disable_accept() -> () = RUN_DISABLE_ACCEPT;
    "_ZN6swoole6Server11clear_timerEv" fn clear_timer() -> () = RUN_ST_CLEAR_TIMER;
    "_ZN6swoole6Server8shutdownEv" fn shutdown() -> bool = RUN_SHUTDOWN;
    "_ZN6swoole6Server18stop_master_threadEv" fn stop_master_thread() -> () = RUN_STOP_MASTER_THREAD;
    "_ZNK6swoole6Server28signal_handler_reopen_loggerEv" fn signal_handler_reopen_logger() -> bool = RUN_REOPEN_LOGGER;
    "_ZN6swoole6Server18add_systemd_socketEv" fn add_systemd_socket() -> c_int = RUN_SYSTEMD_SOCKETS;
    "_ZNK6swoole6Server19get_idle_worker_numEv" fn get_idle_worker_num() -> u32 = RUN_IDLE_WORKERS;
    "_ZNK6swoole6Server24get_idle_task_worker_numEv" fn get_idle_task_worker_num() -> c_int = RUN_IDLE_TASK_WORKERS;
    "_ZNK6swoole6Server15get_tasking_numEv" fn get_tasking_num() -> c_int = RUN_TASKING;
}

/// `Server::set_max_connection(max)`.
///
/// # Safety
/// `serv` is a live `Server`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole6Server18set_max_connectionEj")
)]
pub unsafe extern "C" fn set_max_connection(serv: *mut c_void, max: u32) {
    // SAFETY: as the caller guarantees.
    unsafe {
        run(
            RUN_SET_MAX_CONNECTION,
            max.into(),
            &mut NativeContext::new(serv),
        )
    };
}

/// `Server::feedback(conn, event)`.
///
/// # Safety
/// `serv` is a live `Server` and `conn` one of its connections.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole6Server8feedbackEPNS_10ConnectionENS_15ServerEventTypeE")
)]
pub unsafe extern "C" fn feedback(serv: *mut c_void, conn: *mut c_void, event: c_int) -> bool {
    let mut ctx = NativeContext::new(serv);
    ctx.conn = conn;
    // SAFETY: as the caller guarantees.
    unsafe { run(RUN_FEEDBACK, i64::from(event).cast_unsigned(), &mut ctx) != 0 }
}

/// `Server::store_pipe_fd(pipe)`.
///
/// # Safety
/// `serv` is a live `Server` and `pipe` a live `UnixSocket`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole6Server13store_pipe_fdEPNS_10UnixSocketE")
)]
pub unsafe extern "C" fn store_pipe_fd(serv: *mut c_void, pipe: *mut c_void) {
    let mut ctx = NativeContext::new(serv);
    ctx.object = pipe;
    // SAFETY: as the caller guarantees.
    unsafe { run(RUN_ST_STORE_PIPE_FD, 0, &mut ctx) };
}

/// `Server::schedule_worker(fd, data)`.
///
/// # Safety
/// `serv` is a live `Server`; `data` null or a live `SendData`.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole6Server15schedule_workerEiPNS_8SendDataE")
)]
pub unsafe extern "C" fn schedule_worker(serv: *mut c_void, fd: c_int, data: *mut c_void) -> c_int {
    let mut ctx = NativeContext::new(serv);
    ctx.server_fd = fd;
    ctx.send = data;
    // SAFETY: as the caller guarantees.
    c_int::narrow(unsafe { run(RUN_SCHEDULE_WORKER, i64::from(fd).cast_unsigned(), &mut ctx) })
}

/// `Server::add_port(type, host, port)`: the new port, or null.
///
/// # Safety
/// `serv` is a live `Server`; `host` a NUL-terminated string.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_ZN6swoole6Server8add_portE12swSocketTypePKci")
)]
pub unsafe extern "C" fn add_port(
    serv: *mut c_void,
    socket_type: c_int,
    host: *const c_char,
    port: c_int,
) -> *mut c_void {
    let mut ctx = NativeContext::new(serv);
    ctx.socket_type = socket_type;
    ctx.host = host;
    ctx.number = port;
    // SAFETY: as the caller guarantees.
    if unsafe { run(RUN_ADD_PORT, 0, &mut ctx) } != 0 {
        ctx.port
    } else {
        ptr::null_mut()
    }
}

/// `Server::foreach_connection(callback)`.
///
/// # Safety
/// `serv` is a live `Server`; `callback` a live `std::function<void(Connection *)>`.
#[cfg_attr(
    all(feature = "upstream-symbols", target_os = "macos"),
    unsafe(
        export_name = "_ZNK6swoole6Server18foreach_connectionERKNSt3__18functionIFvPNS_10ConnectionEEEE"
    )
)]
#[cfg_attr(
    all(feature = "upstream-symbols", not(target_os = "macos")),
    unsafe(
        export_name = "_ZNK6swoole6Server18foreach_connectionERKSt8functionIFvPNS_10ConnectionEEE"
    )
)]
pub unsafe extern "C" fn foreach_connection(serv: *mut c_void, callback: *const c_void) {
    let mut ctx = NativeContext::new(serv);
    ctx.object = callback.cast_mut();
    // SAFETY: as the caller guarantees.
    unsafe { run(RUN_ST_FOREACH, 0, &mut ctx) };
}

/// `Server::abort_connection(reactor, port, socket)`.
///
/// # Safety
/// `serv` is a live `Server`; the others are the reactor, port and socket to abort.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(
        export_name = "_ZNK6swoole6Server16abort_connectionEPNS_7ReactorEPKNS_10ListenPortEPNS_7network6SocketE"
    )
)]
pub unsafe extern "C" fn abort_connection(
    serv: *mut c_void,
    reactor: *mut c_void,
    port: *const c_void,
    sock: *mut c_void,
) {
    let mut ctx = NativeContext::new(serv);
    ctx.reactor = reactor;
    ctx.port = port.cast_mut();
    ctx.sock = sock;
    // SAFETY: as the caller guarantees.
    unsafe { run(RUN_ST_ABORT, 0, &mut ctx) };
}

/// `Server::add_connection(port, socket, server_fd)`: the new connection, or null.
///
/// # Safety
/// `serv` is a live `Server`; `port` and `sock` the accepting port and accepted socket.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(
        export_name = "_ZN6swoole6Server14add_connectionEPKNS_10ListenPortEPNS_7network6SocketEi"
    )
)]
pub unsafe extern "C" fn add_connection(
    serv: *mut c_void,
    port: *const c_void,
    sock: *mut c_void,
    server_fd: c_int,
) -> *mut c_void {
    let mut ctx = NativeContext::new(serv);
    ctx.port = port.cast_mut();
    ctx.sock = sock;
    ctx.server_fd = server_fd;
    // SAFETY: as the caller guarantees.
    if unsafe { run(RUN_ADD_CONNECTION, 0, &mut ctx) } != 0 {
        ctx.conn
    } else {
        ptr::null_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// The last call the fake runner saw: op, arg and the context's fields.
    type Seen = (u32, u64, usize, usize, usize, usize, c_int, usize);
    static SEEN: Mutex<Vec<Seen>> = Mutex::new(Vec::new());

    unsafe extern "C-unwind" fn fake(op: u32, arg: u64, ctx: *mut NativeContext) -> i64 {
        // SAFETY: the methods pass their own live context.
        let ctx = unsafe { &mut *ctx };
        SEEN.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((
                op,
                arg,
                ctx.serv.addr(),
                ctx.port.addr(),
                ctx.sock.addr(),
                ctx.object.addr(),
                ctx.number,
                ctx.host.addr(),
            ));
        ctx.port = ptr::without_provenance_mut(0x50);
        ctx.conn = ptr::without_provenance_mut(0x60);
        match op {
            RUN_SHUTDOWN | RUN_ADD_PORT => 1,
            RUN_IDLE_WORKERS => -1,
            _ => 0,
        }
    }

    fn last() -> Seen {
        *SEEN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .last()
            .unwrap_or(&(0, 0, 0, 0, 0, 0, 0, 0))
    }

    #[test]
    fn methods_fill_the_context_their_forward_did() {
        swr_v1_server_set_native(fake);
        let serv = ptr::without_provenance_mut(0x10);
        let at = |a: usize| ptr::without_provenance_mut::<c_void>(a);
        unsafe {
            assert!(shutdown(serv), "shutdown's 1 is true");
            assert_eq!(last().0, RUN_SHUTDOWN, "shutdown op");
            assert_eq!(get_idle_worker_num(serv), u32::MAX, "(uint32_t) -1");
            set_max_connection(serv, 7);
            assert_eq!(last().1, 7, "max connection arg");
            assert_eq!(
                schedule_worker(serv, -2, ptr::null_mut()),
                0,
                "schedule result"
            );
            assert_eq!(
                last().1,
                u64::MAX - 1,
                "fd sign-extends like (uint64_t)(int64_t) fd"
            );
            assert_eq!(
                add_port(serv, 1, c"h".as_ptr(), 9).addr(),
                0x50,
                "nonzero yields the port"
            );
            let (op, _, s, _, _, _, number, host) = last();
            assert!(
                op == RUN_ADD_PORT && s == 0x10 && number == 9 && host != 0,
                "add_port context"
            );
            assert!(
                add_connection(serv, at(0x20), at(0x30), 3).is_null(),
                "zero yields null"
            );
            assert_eq!(
                last(),
                (RUN_ADD_CONNECTION, 0, 0x10, 0x20, 0x30, 0, 0, 0),
                "add_connection context"
            );
            foreach_connection(serv, at(0x40));
            assert_eq!(last().5, 0x40, "callback in object");
            assert!(!feedback(serv, at(0x60), 5), "feedback result");
            assert_eq!(last().1, 5, "event arg");
        }
    }
}
