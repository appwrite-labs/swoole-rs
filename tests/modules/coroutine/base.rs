//! Upstream's `core-tests/src/coroutine/base.cpp` (`coroutine_base`) and
//! `coroutine/file.cpp` (`coroutine_async_file`), one function per TEST with the same
//! inputs and expected values.
//! Built into the hooks case binary (tests/modules/hooks/system), which links
//! the crates.
//!
//! A `swoole::Coroutine` is crates/coroutine's record (`swr_v1_coroutine`, then its
//! `swr_v1_context`; swoole_rs_coroutine.h): its getters are field reads here, its
//! operations the `swr_v1_coroutine_*` exports and upstream symbols.
//! `Coroutine::create`, `coroutine::run` and `Coroutine::bailout` take a function and
//! its data here (`swr_v1_coroutine_create`, `swoole_coroutine_create`,
//! `swr_v1_coroutine_bail`); `swoole_event_defer` has C++ linkage and is in
//! system/fixture.cc.
//! `AsyncFile`'s inline members are its `swr_v1_os_async_file_*` calls.

use super::hook::{
    ST_SIZE, Stat, swoole_coroutine_fstat, swoole_coroutine_fsync, swoole_coroutine_ftruncate,
    swoole_coroutine_lseek, swoole_coroutine_usleep,
};
use super::os::{AsyncEvent, async_dispatch, count_handle};
use super::{
    _exit, coroutine_cancel, fork, last_error, swoole_coroutine_close, swoole_coroutine_create,
    swoole_coroutine_get, swoole_coroutine_get_id, swoole_coroutine_open, swoole_coroutine_read,
    swoole_coroutine_write, system_sleep, trampoline,
};
use std::cell::Cell;
use std::ffi::{c_int, c_long, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::AtomicI32;

const SW_ERROR_CO_CANNOT_CANCEL: c_int = 10015;
const SW_FORK_PRECHECK: c_int = 8;
const RC_TIMEDOUT: i32 = -1;
const RC_CANCELED: i32 = -2;
const SWR_CO_ON_YIELD: u32 = 0;
const SWR_CO_ON_RESUME: u32 = 1;
const SWR_CO_ON_CLOSE: u32 = 2;
const CLOCK_MONOTONIC: c_int = 1;
// glibc's values (the case binary is Linux).
const O_RDWR: c_int = 2;
const O_CREAT: c_int = 0o100;
const SEEK_SET: c_int = 0;
const SEEK_CUR: c_int = 1;
/// The offset of the context's `end` in a coroutine's record: the
/// `swr_v1_coroutine` (96 bytes), then the `swr_v1_context` fields before `end` (64).
const FACADE_END: usize = 96 + 64;

type SwapCallback = Option<extern "C" fn(*mut c_void)>;
type Routine = unsafe extern "C-unwind" fn(*mut c_void);
type Callback = Option<unsafe extern "C" fn(*mut c_void)>;

/// `swr_v1_coroutine`.
#[repr(C)]
struct Co {
    native: *mut c_void,
    cid: c_long,
    state: i32,
    resume_code: i32,
    task: *mut c_void,
    cancel_fn: *mut c_void,
    origin: *mut Co,
    first_child: *mut Co,
    prev_sibling: *mut Co,
    next_sibling: *mut Co,
    init_msec: c_long,
    switch_usec: c_long,
    execute_usec: c_long,
}

/// `swr_v1_coroutine_scheduler`, up to the fields the cases read.
#[repr(C)]
struct Scheduler {
    current: *mut Co,
    last_cid: c_long,
    socket_bound_cid: c_long,
    peak_num: u64,
}

unsafe extern "C-unwind" {
    fn swr_v1_coroutine_scheduler_get() -> *mut Scheduler;
    fn swr_v1_coroutine_create(routine: Routine, arg: *mut c_void) -> c_long;
    fn swr_v1_coroutine_bail(fn_: Callback, data: *mut c_void);
    fn swr_v1_coroutine_get(s: *const Scheduler, cid: c_long) -> *mut Co;
    fn swr_v1_coroutine_count(s: *const Scheduler) -> usize;
    fn swr_v1_coroutine_resume(s: *mut Scheduler, co: *mut Co);
    fn swr_v1_coroutine_yield(s: *mut Scheduler, co: *mut Co);
    fn swr_v1_coroutine_set_task(co: *mut Co, task: *mut c_void);
    fn swr_v1_coroutine_set_callback(s: *mut Scheduler, slot: u32, function: SwapCallback);
    #[link_name = "_ZN6swoole9Coroutine8yield_exEd"]
    fn yield_ex(co: *mut Co, timeout: f64) -> bool;
    #[link_name = "_ZN6swoole9Coroutine10print_listEv"]
    fn print_list();
    #[link_name = "_Z22swoole_coroutine_countv"]
    fn swoole_coroutine_count() -> usize;
    #[link_name = "_Z31swoole_coroutine_iterator_resetv"]
    fn swoole_coroutine_iterator_reset();
    #[link_name = "_Z30swoole_coroutine_iterator_eachv"]
    fn swoole_coroutine_iterator_each() -> *mut Co;
    #[link_name = "_Z11swoole_forki"]
    fn swoole_fork(flags: c_int) -> c_int;

    // system/fixture.cc.
    fn swr_case_event_defer(fn_: Callback, data: *mut c_void);

    fn swr_v1_os_async_file_new() -> *mut c_void;
    fn swr_v1_os_async_file_free(file: *mut c_void);
    fn swr_v1_os_async_file_fd(file: *const c_void) -> c_int;
    fn swr_v1_os_async_file_open(
        file: *mut c_void,
        path: *const std::ffi::c_char,
        flags: c_int,
        mode: u32,
        effect: unsafe extern "C-unwind" fn(*const std::ffi::c_char, c_int, u32) -> c_int,
    ) -> bool;
    fn swr_v1_os_async_file_read(
        file: *const c_void,
        buf: *mut c_void,
        count: usize,
        effect: unsafe extern "C-unwind" fn(c_int, *mut c_void, usize) -> isize,
    ) -> isize;
    fn swr_v1_os_async_file_write(
        file: *const c_void,
        buf: *const c_void,
        count: usize,
        effect: unsafe extern "C-unwind" fn(c_int, *const c_void, usize) -> isize,
    ) -> isize;
    fn swr_v1_os_async_file_sync(
        file: *const c_void,
        effect: unsafe extern "C" fn(c_int) -> c_int,
    ) -> bool;
    fn swr_v1_os_async_file_truncate(
        file: *const c_void,
        length: i64,
        effect: unsafe extern "C" fn(c_int, i64) -> c_int,
    ) -> bool;
    fn swr_v1_os_async_file_stat(
        file: *const c_void,
        statbuf: *mut Stat,
        effect: unsafe extern "C" fn(c_int, *mut Stat) -> c_int,
    ) -> bool;
    fn swr_v1_os_async_file_offset(
        file: *const c_void,
        offset: i64,
        whence: c_int,
        effect: unsafe extern "C" fn(c_int, i64, c_int) -> i64,
    ) -> i64;
}

unsafe extern "C" {
    #[link_name = "_Z17swoole_event_initi"]
    fn swoole_event_init(flags: c_int) -> c_int;
    fn clock_gettime(clock: c_int, ts: *mut [i64; 2]) -> c_int;
    fn usleep(usec: u32) -> c_int;
    fn waitpid(pid: c_int, status: *mut c_int, options: c_int) -> c_int;
}

fn scheduler() -> *mut Scheduler {
    // SAFETY: this thread's scheduler.
    unsafe { swr_v1_coroutine_scheduler_get() }
}

/// `Coroutine::create(fn)`: runs `body` in a new coroutine until it first yields
/// or ends; its id.
fn create<'a>(body: impl FnOnce() + 'a) -> c_long {
    let body: Box<dyn FnOnce() + 'a> = Box::new(body);
    let arg = Box::into_raw(Box::new(body)).cast::<c_void>();
    // SAFETY: `trampoline` takes the box `arg` owns; the borrows outlive the
    // coroutine, as the C++ lambdas' captures do.
    unsafe { swr_v1_coroutine_create(trampoline, arg) }
}

/// `swoole::coroutine::run(fn)`: outside a loop, `swoole_coroutine_create` runs a
/// fresh one to the end.
fn co_run<'a>(body: impl FnOnce() + 'a) -> bool {
    let body: Box<dyn FnOnce() + 'a> = Box::new(body);
    let arg = Box::into_raw(Box::new(body)).cast::<c_void>();
    // SAFETY: as in `create`.
    unsafe { swoole_coroutine_create(trampoline, arg) > 0 }
}

/// `Coroutine::get_current()`.
fn get_current() -> *mut Co {
    // SAFETY: this thread's scheduler; the current record is live.
    unsafe { (*scheduler()).current.as_ref() }.map_or(std::ptr::null_mut(), |co| co.native.cast())
}

/// `Coroutine::get_current_cid()`.
fn get_current_cid() -> c_long {
    // SAFETY: as above.
    unsafe { (*scheduler()).current.as_ref() }.map_or(-1, |co| co.cid)
}

/// `Coroutine::get_current_task()`.
fn get_current_task() -> *mut c_void {
    // SAFETY: as above.
    unsafe { (*scheduler()).current.as_ref() }.map_or(std::ptr::null_mut(), |co| co.task)
}

/// `Coroutine::get_by_cid(cid)`.
fn get_by_cid(cid: c_long) -> *mut Co {
    // SAFETY: this thread's scheduler.
    unsafe { swr_v1_coroutine_get(scheduler(), cid) }
}

/// `Coroutine::count()`.
fn count() -> usize {
    // SAFETY: as above.
    unsafe { swr_v1_coroutine_count(scheduler()) }
}

/// `Timer::get_absolute_msec()`: the steady clock in milliseconds.
fn absolute_msec() -> c_long {
    let mut ts = [0i64; 2];
    // SAFETY: a writable timespec.
    unsafe { clock_gettime(CLOCK_MONOTONIC, &mut ts) };
    ts[0] * 1000 + ts[1] / 1_000_000
}

fn set_callback(slot: u32, function: SwapCallback) {
    // SAFETY: this thread's scheduler.
    unsafe { swr_v1_coroutine_set_callback(scheduler(), slot, function) };
}

/// A live coroutine's facade, read and driven as its C++ members do.
struct View(*mut Co);

impl View {
    fn current() -> Self {
        let co = get_current();
        assert!(!co.is_null());
        Self(co)
    }
    fn by_cid(cid: c_long) -> Self {
        Self(get_by_cid(cid))
    }
    fn co(&self) -> &Co {
        // SAFETY: the coroutine is live while the case holds its view.
        unsafe { &*self.0 }
    }
    fn get_cid(&self) -> c_long {
        self.co().cid
    }
    fn get_init_msec(&self) -> c_long {
        self.co().init_msec
    }
    fn get_task(&self) -> *mut c_void {
        self.co().task
    }
    fn set_task(&self, task: *mut c_void) {
        // SAFETY: the live record.
        unsafe { swr_v1_coroutine_set_task(self.0, task) };
    }
    fn get_origin(&self) -> *mut Co {
        // SAFETY: a non-null origin is live.
        unsafe { self.co().origin.as_ref() }.map_or(std::ptr::null_mut(), |co| co.native.cast())
    }
    fn get_origin_cid(&self) -> c_long {
        // SAFETY: as above.
        unsafe { self.co().origin.as_ref() }.map_or(-1, |co| co.cid)
    }
    fn is_end(&self) -> bool {
        // SAFETY: the facade's Context end flag (see FACADE_END).
        unsafe { self.0.cast::<u8>().add(FACADE_END).cast::<bool>().read() }
    }
    fn is_canceled(&self) -> bool {
        self.co().resume_code == RC_CANCELED
    }
    fn is_timedout(&self) -> bool {
        self.co().resume_code == RC_TIMEDOUT
    }
    fn yield_(&self) {
        // SAFETY: the running coroutine.
        unsafe { swr_v1_coroutine_yield(scheduler(), self.0) };
    }
    fn yield_ex(&self, timeout: f64) -> bool {
        // SAFETY: as above.
        unsafe { yield_ex(self.0, timeout) }
    }
    fn resume(&self) {
        // SAFETY: a suspended coroutine.
        unsafe { swr_v1_coroutine_resume(scheduler(), self.0) };
    }
    fn cancel(&self) -> bool {
        // SAFETY: a live coroutine.
        unsafe { coroutine_cancel(self.0.cast()) }
    }
}

/// Calls the `&dyn Fn()` at `data`.
unsafe extern "C" fn call(data: *mut c_void) {
    // SAFETY: the caller's closure, live for the call.
    let body = unsafe { &*data.cast::<&dyn Fn()>() };
    if catch_unwind(AssertUnwindSafe(body)).is_err() {
        // SAFETY: ends the process without unwinding into C++.
        unsafe { _exit(1) };
    }
}

/// `Coroutine::bailout(fn)`, or `bailout(nullptr)` for `None`.
fn bailout(body: Option<&dyn Fn()>) {
    match body {
        // SAFETY: `body` outlives the call, or the process ends first.
        Some(body) => unsafe {
            swr_v1_coroutine_bail(Some(call), (&raw const body).cast_mut().cast())
        },
        // SAFETY: no callback.
        None => unsafe { swr_v1_coroutine_bail(None, std::ptr::null_mut()) },
    }
}

/// core-tests' `test::spawn_exec_and_wait`: the child's wait status.
fn spawn_exec_and_wait(body: impl FnOnce()) -> c_int {
    // SAFETY: fork(2); the child runs `body` and exits.
    let pid = unsafe { fork() };
    assert_ne!(pid, -1);
    if pid == 0 {
        body();
        // SAFETY: ends the child.
        unsafe { _exit(0) };
    }
    let mut status = 0;
    // SAFETY: our child, a writable status.
    if unsafe { waitpid(pid, &mut status, 0) } == pid {
        status
    } else {
        -1
    }
}

fn wexitstatus(status: c_int) -> c_int {
    (status >> 8) & 0xff
}

fn create_() {
    let cid_ = Cell::new(0);
    let cid = create(|| cid_.set(get_current_cid()));

    assert!(cid > 0);
    assert_eq!(cid, cid_.get());
}

fn get_current_() {
    let cid_ = Cell::new(0);
    let cid = create(|| cid_.set(View::current().get_cid()));

    assert!(cid > 0);
    assert_eq!(cid, cid_.get());
}

fn get_init_msec() {
    create(|| {
        let init_msec = View::current().get_init_msec();

        assert!(init_msec > 0);
    });
}

extern "C" fn yield_resume_on_yield(_: *mut c_void) {
    let task = get_current_task().cast::<c_long>();
    assert!(!task.is_null());
    // SAFETY: the task is the case's live `cid2`.
    assert_eq!(unsafe { *task }, get_current_cid());
}

extern "C" fn yield_resume_on_resume(_: *mut c_void) {
    assert!(get_current().is_null());
}

fn yield_resume() {
    set_callback(SWR_CO_ON_YIELD, Some(yield_resume_on_yield));
    set_callback(SWR_CO_ON_RESUME, Some(yield_resume_on_resume));
    set_callback(SWR_CO_ON_CLOSE, Some(yield_resume_on_yield));

    let cid_ = Cell::new(0);
    let cid2 = Cell::new(0);
    let cid = create(|| {
        cid2.set(get_current_cid());
        let co = View::by_cid(cid2.get());
        co.set_task(cid2.as_ptr().cast());
        co.yield_();
        cid_.set(get_current_cid());
    });

    assert!(cid > 0);
    View::by_cid(cid).resume();
    set_callback(SWR_CO_ON_CLOSE, None);
    assert_eq!(cid, cid_.get());
}

fn get_cid() {
    create(|| {
        let cid = View::current().get_cid();

        assert!(cid > 0);
    });
}

fn get_origin() {
    create(|| {
        let co = get_current();

        create(move || {
            let origin_co = View::current().get_origin();

            assert_eq!(co, origin_co);
        });
    });
}

fn get_origin_cid() {
    create(|| {
        let cid_ = get_current_cid();

        create(move || {
            let origin_cid = View::current().get_origin_cid();

            assert_eq!(cid_, origin_cid);
        });
    });
}

fn expired_origin() {
    let child = Cell::new(std::ptr::null_mut());
    let child_cid = Cell::new(0);

    create(|| {
        child_cid.set(create(|| View::current().yield_()));
        child.set(get_by_cid(child_cid.get()));
    });

    assert!(!child.get().is_null());
    let child = View(child.get());
    assert!(child.get_origin().is_null());
    assert_eq!(-1, child.get_origin_cid());

    child.resume();
    assert!(get_by_cid(child_cid.get()).is_null());
}

fn is_end() {
    create(|| {
        assert!(!View::current().is_end());
    });
}

fn set_task() {
    create(|| {
        let mut task: c_int = 0;
        let co = View::current();
        co.set_task((&raw mut task).cast());
        let actual = co.get_task();
        assert_eq!(actual, (&raw mut task).cast());
    });
}

fn get_current_task_() {
    create(|| {
        let mut task: c_int = 0;
        let co = View::current();
        co.set_task((&raw mut task).cast());
        let actual = co.get_task();
        assert_eq!(actual, get_current_task());
    });
}

fn get_current_cid_() {
    create(|| {
        let actual = View::current().get_cid();
        assert_eq!(actual, get_current_cid());
        // SAFETY: in a coroutine.
        assert_eq!(actual, unsafe { swoole_coroutine_get_id() });
    });
}

fn get_by_cid_() {
    create(|| {
        let actual = View::current();
        let cid = actual.get_cid();
        assert_eq!(actual.0, get_by_cid(cid));
    });
}

fn get_task_by_cid() {
    create(|| {
        let mut task: c_int = 0;
        let co = View::current();
        co.set_task((&raw mut task).cast());
        let actual = co.get_task();
        // Coroutine::get_task_by_cid.
        let by_cid = View::by_cid(co.get_cid());
        assert_eq!(actual, by_cid.get_task());
    });
}

fn get_last_cid() {
    create(|| {});
    create(|| {});
    let cid = create(|| {});

    // SAFETY: this thread's scheduler.
    assert_eq!(unsafe { (*scheduler()).last_cid }, cid);
}

fn count_() {
    create(|| {
        assert_eq!(count(), 1);
        create(|| assert_eq!(count(), 2));
    });
    assert_eq!(count(), 0);
}

fn get_peak_num() {
    create(|| {
        create(|| {
            // SAFETY: this thread's scheduler.
            assert!(unsafe { (*scheduler()).peak_num } >= 2);
        });
    });
}

fn get_elapsed() {
    let elapsed_time = Cell::new(0);
    create(|| {
        let co = View::current();
        // SAFETY: usleep(3).
        unsafe { usleep(2000) };
        // Coroutine::get_elapsed(cid).
        let co = View::by_cid(co.get_cid());
        elapsed_time.set(absolute_msec() - co.get_init_msec());
    });
    assert!(elapsed_time.get() >= 2);
}

fn run() {
    let cid = c_long::from(co_run(|| {}));
    assert!(cid >= 1);
}

fn cancel_() {
    co_run(|| {
        let co = View::current();
        let target = co.0;
        create(move || {
            // SAFETY: in a coroutine.
            unsafe { system_sleep(0.002) };
            View(target).cancel();
        });
        assert!(!co.yield_ex(-1.0));
        assert!(co.is_canceled());
    });
}

fn noncancelable() {
    let flags: [Cell<bool>; 7] = Default::default();
    let [
        yield_1,
        yield_2,
        cancel_1,
        resume_1,
        cancel_2,
        resume_2,
        done,
    ] = &flags;
    co_run(|| {
        let cid = create(|| {
            let current = View::current();
            yield_1.set(true);
            current.yield_();
            assert!(!current.is_canceled());

            yield_2.set(true);
            current.yield_ex(-1.0);
            assert!(current.is_canceled());
        });

        let co = View::by_cid(cid);

        cancel_1.set(true);
        assert!(!co.cancel());
        assert_eq!(last_error(), SW_ERROR_CO_CANNOT_CANCEL);
        resume_1.set(true);
        co.resume();

        cancel_2.set(true);
        assert!(co.cancel());
        resume_2.set(true);

        done.set(true);
    });

    for flag in &flags {
        assert!(flag.get());
    }
}

fn timeout() {
    co_run(|| {
        let co = View::current();
        assert!(!co.yield_ex(0.005));
        assert!(co.is_timedout());
    });
}

fn gdb() {
    create(|| {
        let current = get_current();
        let cid = View(current).get_cid();
        // SAFETY: the gdb helpers, in a coroutine.
        unsafe {
            assert_eq!(swoole_coroutine_count(), 1);
            assert_eq!(swoole_coroutine_get(cid), current.cast());
            assert!(swoole_coroutine_get(999_999).is_null());

            swoole_coroutine_iterator_reset();
            assert_eq!(swoole_coroutine_iterator_each(), current);
            assert!(swoole_coroutine_iterator_each().is_null());

            swoole_coroutine_iterator_reset();
            assert_eq!(swoole_coroutine_iterator_each(), current);
            print_list();
        }
    });
}

fn bailout_() {
    let status = spawn_exec_and_wait(|| {
        let exit = Cell::new(false);
        let end = Cell::new(false);
        co_run(|| {
            create(|| {
                bailout(Some(&|| exit.set(true)));
                end.set(true);
            });
        });

        assert!(exit.get());
        assert!(!end.get());
    });
    assert_eq!(status, 0);

    let status = spawn_exec_and_wait(|| {
        let exit = Cell::new(false);
        let end = Cell::new(false);
        co_run(|| {
            bailout(None);
            end.set(true);
        });

        assert!(exit.get());
        assert!(!end.get());
    });
    assert_eq!(wexitstatus(status), 1);

    let status = spawn_exec_and_wait(|| {
        let exit = Cell::new(false);
        let end = Cell::new(false);
        let bailed = Cell::new(false);
        // Outside the coroutine, as upstream's std::function copy is: the coroutine
        // ends, and its stack is freed, before the loop runs the deferred call.
        let deferred = || {
            bailed.set(true);
            bailout(None);
            end.set(true);
        };
        let deferred: &dyn Fn() = &deferred;
        let data = (&raw const deferred).cast_mut().cast();
        co_run(|| {
            // SAFETY: the loop runs the deferred call before `co_run` returns, while
            // `deferred` is live.
            unsafe { swr_case_event_defer(Some(call), data) };
            exit.set(true);
        });

        assert!(exit.get());
        assert!(end.get());
    });
    assert_eq!(wexitstatus(status), 0);
}

extern "C" fn ignore(_: *mut AsyncEvent) {}

fn undefined_behavior() {
    let status = spawn_exec_and_wait(|| {
        // test::coroutine::run.
        super::run(|| {
            // SAFETY: swoole_fork in a coroutine, the behaviour under test.
            let _ = unsafe { swoole_fork(0) };
        });
    });
    assert_eq!(1, wexitstatus(status));

    let status = spawn_exec_and_wait(|| {
        let handle_count = AtomicI32::new(0);
        let event = AsyncEvent::new(
            (&raw const handle_count).cast_mut().cast(),
            Some(count_handle),
            Some(ignore),
        );

        // SAFETY: the event loop, then a pool task over the live counter.
        unsafe { swoole_event_init(0) };
        // SAFETY: as above.
        let ret = unsafe { async_dispatch(&event) };
        assert!(!ret.is_null());
        // SAFETY: swoole_fork after async threads, the behaviour under test.
        let _ = unsafe { swoole_fork(0) };
    });
    assert_eq!(1, wexitstatus(status));

    // SAFETY: the precheck only.
    assert_eq!(0, unsafe { swoole_fork(SW_FORK_PRECHECK) });
}

unsafe extern "C-unwind" fn c_api_inner(_: *mut c_void) {
    // SAFETY: in a coroutine.
    unsafe { swoole_coroutine_usleep(100_000) };
}

unsafe extern "C-unwind" fn c_api_outer(arg: *mut c_void) {
    let body = || {
        // SAFETY: the case's `code`.
        assert_eq!(unsafe { *arg.cast::<c_int>() }, 0x9501);
        // SAFETY: a routine that takes no argument.
        unsafe { swoole_coroutine_create(c_api_inner, std::ptr::null_mut()) };
    };
    if catch_unwind(AssertUnwindSafe(body)).is_err() {
        // SAFETY: ends the process mid-coroutine.
        unsafe { _exit(1) };
    }
}

fn c_api() {
    let mut code: c_int = 0x9501;
    // SAFETY: the routine reads the live `code`.
    let c1 = unsafe { swoole_coroutine_create(c_api_outer, (&raw mut code).cast()) };

    assert!(c1 >= 1);
}

/// `coroutine_async_file.async_file`.
fn async_file() {
    co_run(|| {
        // SAFETY: an `AsyncFile`, its inline members' calls over the hooked file
        // functions, freed (after its close) at the end.
        unsafe {
            let file = swr_v1_os_async_file_new();
            swr_v1_os_async_file_open(
                file,
                c"/tmp/file.txt".as_ptr(),
                O_CREAT | O_RDWR,
                0o666,
                swoole_coroutine_open,
            );
            // ready()
            assert_ne!(swr_v1_os_async_file_fd(file), -1);
            assert!(swr_v1_os_async_file_truncate(
                file,
                0,
                swoole_coroutine_ftruncate
            ));
            assert_eq!(
                swr_v1_os_async_file_offset(file, 0, SEEK_SET, swoole_coroutine_lseek),
                0
            );
            assert_eq!(
                swr_v1_os_async_file_offset(file, 0, SEEK_CUR, swoole_coroutine_lseek),
                0
            );

            let data = b"Hello World!";
            let length = data.len();
            assert_eq!(
                swr_v1_os_async_file_write(
                    file,
                    data.as_ptr().cast(),
                    length,
                    swoole_coroutine_write
                ),
                length as isize
            );

            assert!(swr_v1_os_async_file_sync(file, swoole_coroutine_fsync));
            assert_eq!(
                swr_v1_os_async_file_offset(file, 0, SEEK_SET, swoole_coroutine_lseek),
                0
            );

            let mut buf = [0u8; 1024];
            assert_eq!(
                swr_v1_os_async_file_read(
                    file,
                    buf.as_mut_ptr().cast(),
                    1024,
                    swoole_coroutine_read
                ),
                length as isize
            );
            assert_eq!(&buf[..length], data);

            let mut statbuf: Stat = [0; 32];
            assert!(swr_v1_os_async_file_stat(
                file,
                &mut statbuf,
                swoole_coroutine_fstat
            ));
            assert!(statbuf[ST_SIZE] > 0);

            // ~AsyncFile: close, then the owner's free.
            let fd = swr_v1_os_async_file_fd(file);
            if fd != -1 {
                swoole_coroutine_close(fd);
            }
            swr_v1_os_async_file_free(file);
        }
    });
}

/// `coroutine_base.<name>`, in upstream's order.
pub(super) const CASES: super::Cases = &[
    ("create", create_),
    ("get_current", get_current_),
    ("get_init_msec", get_init_msec),
    ("yield_resume", yield_resume),
    ("get_cid", get_cid),
    ("get_origin", get_origin),
    ("get_origin_cid", get_origin_cid),
    ("expired_origin", expired_origin),
    ("is_end", is_end),
    ("set_task", set_task),
    ("get_current_task", get_current_task_),
    ("get_current_cid", get_current_cid_),
    ("get_by_cid", get_by_cid_),
    ("get_task_by_cid", get_task_by_cid),
    ("get_last_cid", get_last_cid),
    ("count", count_),
    ("get_peak_num", get_peak_num),
    ("get_elapsed", get_elapsed),
    ("run", run),
    ("cancel", cancel_),
    ("noncancelable", noncancelable),
    ("timeout", timeout),
    ("gdb", gdb),
    ("bailout", bailout_),
    ("undefined_behavior", undefined_behavior),
    ("c_api", c_api),
];

/// `coroutine_async_file.<name>`.
pub(super) const ASYNC_FILE_CASES: super::Cases = &[("async_file", async_file)];
