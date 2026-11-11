//! The `Server` methods of the deleted worker-side files (`src/server/manager.cc`,
//! `worker.cc`, `reactor_process.cc`, `task_worker.cc` and `thread.cc`), defined under their upstream (Itanium)
//! symbols when the hybrid links this crate (`upstream-symbols`). Each one enters
//! crates/server with its operation and objects, which builds the context the C++
//! forward built and runs the operation over it.
use core::ffi::{c_char, c_int, c_void};
use core::ptr;
use std::sync::OnceLock;

use super::ops::{
    RUN_CREATE_THREAD_FACTORY, RUN_DESTROY_THREAD_FACTORY, RUN_FINISH, RUN_INIT_TASK_WORKERS,
    RUN_REACTOR_PROCESS_MAIN, RUN_REACTOR_PROCESS_START, RUN_READ_WORKER_MESSAGE, RUN_RELOAD,
    RUN_RESTART_WORKER, RUN_START_MANAGER, RUN_START_WORKER_THREADS, RUN_STOP_WORKER_THREADS,
    RUN_THREAD_FACTORY_START, RUN_THREAD_ID, RUN_THREAD_MANAGER_WAIT, RUN_THREAD_RELOAD,
    RUN_THREAD_TERMINATE_MANAGER,
};
use crate::methods::Narrow;

/// What an operation runs over: crates/server's builder of its context, which runs
/// it with `arg` over the objects.
pub type WorkersEnter = unsafe fn(
    operation: u32,
    object: *mut c_void,
    other: *mut c_void,
    extra: *mut c_void,
    arg: u64,
) -> i64;

/// crates/server's `workers_enter`: each operation's [`WorkersEnter`].
pub type WorkersRoute = fn(operation: u32) -> WorkersEnter;

static ROUTE: OnceLock<WorkersRoute> = OnceLock::new();

/// Registers crates/server's `workers_enter`, which the methods below enter.
pub fn set_enter(route: WorkersRoute) {
    // A second registration is the same function; keeping the first is equivalent.
    ROUTE.get_or_init(|| route);
}

/// Runs `operation` with `arg` over the context crates/server builds from the objects.
///
/// # Safety
/// The objects are what `operation`'s context takes, live for the call.
unsafe fn enter<R: Narrow>(
    operation: u32,
    object: *const c_void,
    other: *const c_void,
    extra: *const c_void,
    arg: u64,
) -> R {
    let Some(&route) = ROUTE.get() else {
        // crates/server registers when a Server opens.
        std::process::abort()
    };
    let enter = route(operation);
    // SAFETY: as the caller guarantees.
    R::narrow(unsafe {
        enter(
            operation,
            object.cast_mut(),
            other.cast_mut(),
            extra.cast_mut(),
            arg,
        )
    })
}

const NONE: *const c_void = ptr::null();

/// A method's object as `enter` takes it.
trait Object {
    fn object(self) -> *const c_void;
}

impl<T> Object for *const T {
    fn object(self) -> *const c_void {
        self.cast()
    }
}

impl<T> Object for *mut T {
    fn object(self) -> *const c_void {
        self.cast_const().cast()
    }
}

/// Each `Server` method that enters one operation: its Rust name,
/// parameters, return type, and the operation's objects and argument.
macro_rules! methods {
    ($($(#[$attr:meta])* fn $name:ident($($p:ident: $t:ty),*) -> $r:ty
        = $op:ident($object:expr, $other:expr, $extra:expr, $arg:expr);)*) => {$(
        #[doc = concat!("`Server::", stringify!($name), "`.")]
        ///
        /// # Safety
        /// The pointers are live objects of the types the C++ method takes.
        $(#[$attr])*
        pub unsafe extern "C" fn $name($($p: $t),*) -> $r {
            // SAFETY: as the caller guarantees.
            unsafe { enter($op, $object.object(), $other.object(), $extra.object(), $arg) }
        }
    )*};
}

methods! {
    fn start_manager_process(serv: *mut c_void) -> c_int = RUN_START_MANAGER(serv, NONE, NONE, 0);
    fn restart_worker_process(serv: *mut c_void, exit_status: *const c_void) -> c_int
        = RUN_RESTART_WORKER(serv, exit_status, NONE, 0);
    fn read_worker_message(pool: *mut c_void, msg: *mut c_void) -> ()
        = RUN_READ_WORKER_MESSAGE(pool, msg, NONE, 0);
    #[must_use]
    fn reload(serv: *const c_void, reload_all_workers: bool) -> bool
        = RUN_RELOAD(serv, NONE, NONE, u64::from(reload_all_workers));
    fn start_reactor_processes(serv: *mut c_void) -> c_int
        = RUN_REACTOR_PROCESS_START(serv, NONE, NONE, 0);
    fn reactor_process_main_loop(pool: *mut c_void, worker: *mut c_void) -> c_int
        = RUN_REACTOR_PROCESS_MAIN(pool, worker, NONE, 0);
}

methods! {
    fn create_thread_factory(serv: *mut c_void) -> *mut c_void
        = RUN_CREATE_THREAD_FACTORY(serv, NONE, NONE, 0);
    fn destroy_thread_factory(serv: *const c_void) -> () = RUN_DESTROY_THREAD_FACTORY(serv, NONE, NONE, 0);
    fn start_worker_threads(serv: *mut c_void) -> c_int = RUN_START_WORKER_THREADS(serv, NONE, NONE, 0);
    fn stop_worker_threads(serv: *mut c_void) -> () = RUN_STOP_WORKER_THREADS(serv, NONE, NONE, 0);
    #[must_use]
    fn reload_worker_threads(serv: *const c_void, reload_all_workers: bool) -> bool
        = RUN_THREAD_RELOAD(serv, NONE, NONE, u64::from(reload_all_workers));
}

/// Each `ThreadFactory` method that enters one operation over its server and itself: the
/// upstream method, Rust name, parameters, return type and the operation's argument.
macro_rules! thread_factory_methods {
    ($($(#[$attr:meta])* $method:literal fn $name:ident($($p:ident: $t:ty),*) -> $r:ty
        = $op:ident($arg:expr);)*) => {$(
        #[doc = concat!("`ThreadFactory::", $method, "`.")]
        ///
        /// # Safety
        /// `factory` is a live `ThreadFactory`.
        $(#[$attr])*
        pub unsafe extern "C" fn $name(factory: *mut c_void, $($p: $t),*) -> $r {
            // SAFETY: as the caller guarantees: `Factory::server_` is its first member.
            let serv = unsafe { *factory.cast::<*mut c_void>() };
            // SAFETY: as the caller guarantees.
            unsafe { enter($op, serv.object(), factory.object(), NONE, $arg) }
        }
    )*};
}

thread_factory_methods! {
    #[must_use]
    "start()"
    fn thread_factory_start() -> bool = RUN_THREAD_FACTORY_START(0);
    "wait()"
    fn thread_factory_wait() -> () = RUN_THREAD_MANAGER_WAIT(0);
    #[must_use]
    "reload(reload_all_workers)"
    fn thread_factory_reload(reload_all_workers: bool) -> bool
        = RUN_THREAD_RELOAD(u64::from(reload_all_workers));
    "terminate_manager_thread()"
    fn terminate_manager_thread() -> () = RUN_THREAD_TERMINATE_MANAGER(0);
    #[must_use]
    "get_master_thread_id()"
    fn get_master_thread_id() -> u32 = RUN_THREAD_ID(1);
}

/// `Server::finish`'s result bytes, as `workers_enter`'s `extra`.
pub struct Bytes {
    pub data: *const c_char,
    pub length: usize,
}

methods! {
    #[must_use]
    fn init_task_workers(serv: *mut c_void) -> bool = RUN_INIT_TASK_WORKERS(serv, NONE, NONE, 0);
}

/// `Server::finish(data, data_len, flags, current_task)`: the task's result to the
/// worker that sent it.
///
/// # Safety
/// `serv` is a live `Server`, `data` holds `data_len` bytes and `current_task` is
/// null or a live `EventData`.
#[must_use]
pub unsafe fn finish(
    serv: *mut c_void,
    data: *const c_char,
    data_len: usize,
    flags: c_int,
    current_task: *const c_void,
) -> bool {
    let bytes = Bytes {
        data,
        length: data_len,
    };
    let flags = u64::from(flags.cast_unsigned());
    // SAFETY: as the caller guarantees; `bytes` lives for the call.
    unsafe {
        enter(
            RUN_FINISH,
            serv,
            current_task,
            (&raw const bytes).cast(),
            flags,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Operation, object, other, extra (or the session it points to), arg.
    type Entry = (u32, usize, usize, usize, u64);
    static SEEN: Mutex<Vec<Entry>> = Mutex::new(Vec::new());

    unsafe fn fake(
        operation: u32,
        object: *mut c_void,
        other: *mut c_void,
        extra: *mut c_void,
        arg: u64,
    ) -> i64 {
        let extra = match operation {
            // SAFETY: finish lends its result's bytes.
            RUN_FINISH => unsafe { (*extra.cast::<Bytes>()).length },
            _ => extra.addr(),
        };
        SEEN.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((operation, object.addr(), other.addr(), extra, arg));
        match operation {
            RUN_RESTART_WORKER => 1234,
            _ => 1,
        }
    }

    #[test]
    fn methods_enter_with_their_operation_and_objects() {
        set_enter(|_| fake);
        let at = |a: usize| ptr::without_provenance_mut::<c_void>(a);
        // A `ThreadFactory`: `server_`, then `manager_`.
        let mut factory: [usize; 2] = [0x10, 0];
        let this = (&raw mut factory).cast::<c_void>();
        // SAFETY: the fake only records the pointers.
        unsafe {
            assert_eq!(start_manager_process(at(0x10)), 1);
            assert_eq!(restart_worker_process(at(0x10), at(0x20)), 1234);
            read_worker_message(at(0x30), at(0x40));
            assert!(reload(at(0x10), true));
            assert_eq!(reactor_process_main_loop(at(0x30), at(0x20)), 1);
            assert!(finish(at(0x10), c"ok".as_ptr(), 2, -1, at(0x80)));
            assert!(!create_thread_factory(at(0x10)).is_null());
            assert!(reload_worker_threads(at(0x10), false));
            assert_eq!(get_master_thread_id(this), 1);
        }
        let seen = SEEN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            *seen,
            [
                (RUN_START_MANAGER, 0x10, 0, 0, 0),
                (RUN_RESTART_WORKER, 0x10, 0x20, 0, 0),
                (RUN_READ_WORKER_MESSAGE, 0x30, 0x40, 0, 0),
                (RUN_RELOAD, 0x10, 0, 0, 1),
                (RUN_REACTOR_PROCESS_MAIN, 0x30, 0x20, 0, 0),
                (RUN_FINISH, 0x10, 0x80, 2, u64::from(u32::MAX)),
                (RUN_CREATE_THREAD_FACTORY, 0x10, 0, 0, 0),
                (RUN_THREAD_RELOAD, 0x10, 0, 0, 0),
                (RUN_THREAD_ID, 0x10, this.addr(), 0, 1),
            ]
        );
    }
}
