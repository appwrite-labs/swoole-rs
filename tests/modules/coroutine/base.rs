//! Upstream's `core-tests/src/coroutine/base.cpp` (`coroutine_base`) and
//! `coroutine/file.cpp` (`coroutine_async_file`), one function per TEST with the same
//! inputs and expected values.
//! Built into the hooks case binary (tests/modules/hooks/system), which links
//! the crates.
//!
//! A `swoole::Coroutine` is crates/corebase/src/coroutine's record (`swr_v1_coroutine`, then its
//! `swr_v1_context`; swoole_rs_corebase::coroutine.h): its getters are field reads here, its
//! operations crates/corebase/src/coroutine's `suspend` and `resume`, the `swr_v1_coroutine_*`
//! exports and upstream symbols.
//! `Coroutine::create` is crates/corebase/src/coroutine's `spawn` of a closure over
//! the case's boxed body; `coroutine::run` and `Coroutine::bailout` take a function and
//! its data here (`swoole_coroutine_create` and `swr_v1_coroutine_bail`).

use swoole_rs_corebase::swoole_fork;

use super::hook::{
    ST_SIZE, Stat, swoole_coroutine_fstat, swoole_coroutine_fsync, swoole_coroutine_ftruncate,
    swoole_coroutine_lseek, swoole_coroutine_usleep,
};
use super::os::{AsyncEvent, count_handle, dispatch, new_event};
use super::{
    _exit, co_sleep, coroutine_cancel, fork, last_error, swoole_coroutine_close,
    swoole_coroutine_create, swoole_coroutine_get_id, swoole_coroutine_open, swoole_coroutine_read,
    swoole_coroutine_write, trampoline,
};
use libc::{CLOCK_MONOTONIC, O_CREAT, O_RDWR, SEEK_CUR, SEEK_SET};
use std::cell::Cell;
use std::ffi::{c_int, c_long, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::AtomicI32;
use swoole_rs_corebase::coroutine::{
    Coroutine, Resume, coroutine_count as swoole_coroutine_count,
    coroutine_get as swoole_coroutine_get, counters, current_cid as get_current_cid, print_list,
    swoole_coroutine_iterator_each, swoole_coroutine_iterator_reset,
};
use swoole_rs_ffi::swoole_event_init;

const SW_ERROR_CO_CANNOT_CANCEL: c_int = 10015;
const SW_FORK_PRECHECK: c_int = 8;
const SWR_CO_ON_YIELD: u32 = 0;
const SWR_CO_ON_RESUME: u32 = 1;
const SWR_CO_ON_CLOSE: u32 = 2;

type SwapCallback = swoole_rs_corebase::coroutine::SwapCallback;

unsafe extern "C" {
    fn clock_gettime(clock: c_int, ts: *mut [i64; 2]) -> c_int;
    fn usleep(usec: u32) -> c_int;
    fn waitpid(pid: c_int, status: *mut c_int, options: c_int) -> c_int;
}

/// `Coroutine::create(fn)`: runs `body` in a new coroutine until it first yields
/// or ends; its id.
fn create<'a>(body: impl FnOnce() + 'a) -> c_long {
    let body: Box<dyn FnOnce() + 'a> = Box::new(body);
    let arg = Box::into_raw(Box::new(body)).cast::<c_void>();
    // SAFETY: `trampoline` takes the box `arg` owns; the borrows outlive the
    // coroutine, as the C++ lambdas' captures do.
    swoole_rs_corebase::coroutine::spawn(move || unsafe { trampoline(arg) })
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
fn get_current() -> *mut Coroutine {
    swoole_rs_corebase::coroutine::current()
}

/// `Coroutine::get_current_task()`.
fn get_current_task() -> *mut c_void {
    // SAFETY: the running coroutine is live throughout this field read.
    unsafe { swoole_rs_corebase::coroutine::current().as_ref() }
        .and_then(|co| co.task)
        .map_or(std::ptr::null_mut(), |task| task.as_ptr().as_ptr())
}

/// `Coroutine::get_by_cid(cid)`.
fn get_by_cid(cid: c_long) -> *mut Coroutine {
    swoole_rs_corebase::coroutine::coroutine_get(cid).cast()
}

/// `Coroutine::count()`.
fn count() -> usize {
    swoole_rs_corebase::coroutine::coroutine_count()
}

/// `Timer::get_absolute_msec()`: the steady clock in milliseconds.
fn absolute_msec() -> c_long {
    let mut ts = [0i64; 2];
    // SAFETY: a writable timespec.
    unsafe { clock_gettime(CLOCK_MONOTONIC, &mut ts) };
    ts[0] * 1000 + ts[1] / 1_000_000
}

fn set_callback(slot: u32, function: SwapCallback) {
    swoole_rs_corebase::coroutine::set_callback(slot, function);
}

/// A live coroutine's facade, read and driven as its C++ members do.
struct View(*mut Coroutine);

impl View {
    fn current() -> Self {
        let co = get_current();
        assert!(!co.is_null());
        Self(co)
    }
    fn by_cid(cid: c_long) -> Self {
        Self(get_by_cid(cid))
    }
    fn co(&self) -> &Coroutine {
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
        self.co()
            .task
            .map_or(std::ptr::null_mut(), |task| task.as_ptr().as_ptr())
    }
    fn set_task(&self, task: *mut c_void) {
        assert_eq!(self.0, get_current());
        let task = std::ptr::NonNull::new(task).unwrap();
        // SAFETY: each case supplies its running coroutine's task for its callbacks.
        unsafe { swoole_rs_corebase::coroutine::set_current_task(task) };
    }
    fn get_origin(&self) -> *mut Coroutine {
        self.co().origin
    }
    fn get_origin_cid(&self) -> c_long {
        // SAFETY: a non-null origin is live.
        unsafe { self.co().origin.as_ref() }.map_or(-1, |co| co.cid)
    }
    fn is_end(&self) -> bool {
        // SAFETY: the coroutine's record, a `Native`, is live while the case holds its view.
        unsafe {
            &*self
                .0
                .cast::<swoole_rs_corebase::coroutine::context::Native>()
        }
        .ctx
        .ended()
    }
    fn is_canceled(&self) -> bool {
        self.co().resume_code == Resume::Canceled
    }
    fn is_timedout(&self) -> bool {
        self.co().resume_code == Resume::TimedOut
    }
    fn yield_(&self) {
        swoole_rs_corebase::coroutine::suspend(self.0.cast());
    }
    fn yield_ex(&self, timeout: f64) -> bool {
        swoole_rs_corebase::coroutine::yield_resumed(swoole_rs_corebase::coroutine::yield_timed(
            self.0.cast(),
            timeout,
        ))
    }
    fn resume(&self) {
        // SAFETY: the case's own yield (a user yield), which any resume may end.
        unsafe { swoole_rs_corebase::coroutine::resume(self.get_cid()) };
    }
    fn cancel(&self) -> bool {
        coroutine_cancel(self.0.cast())
    }
}

/// [`call`] of the `&dyn Fn()` at its pointer, as a deferred task.
struct DeferredCall(*mut c_void);

impl swoole_rs_ffi::event_api::Deferred for DeferredCall {
    fn run(self) {
        // The deferrer's closure, live until the loop runs this.
        call(self.0);
    }

    fn dispose(self) {}
}

/// Calls the `&dyn Fn()` at `data`, which [`bailout`] passes: the one data its
/// callback is given.
extern "C" fn call(data: *mut c_void) {
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
            swoole_rs_corebase::coroutine::swr_v1_coroutine_bail(
                Some(call),
                (&raw const body).cast_mut().cast(),
            )
        },
        // SAFETY: no callback.
        None => unsafe {
            swoole_rs_corebase::coroutine::swr_v1_coroutine_bail(None, std::ptr::null_mut())
        },
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

extern "C" fn yield_resume_on_yield(_: swoole_rs_corebase::coroutine::CoroutineTask) {
    let task = get_current_task().cast::<c_long>();
    assert!(!task.is_null());
    // SAFETY: the task is the case's live `cid2`.
    assert_eq!(unsafe { *task }, get_current_cid());
}

extern "C" fn yield_resume_on_resume(_: swoole_rs_corebase::coroutine::CoroutineTask) {
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

    assert_eq!(counters().last_cid, cid);
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
            assert!(counters().peak_num >= 2);
        });
    });
}

fn get_elapsed() {
    use swoole_rs_corebase::coroutine::{current_execute_time, elapsed, steady_usec};

    assert_eq!(elapsed(0), -1);
    assert_eq!(current_execute_time(), -1);
    let cid = create(|| {
        let co = View::current();
        let start = co.get_init_msec();
        // SAFETY: usleep(3).
        unsafe { usleep(2000) };
        for cid in [0, co.get_cid()] {
            let earliest = absolute_msec() - start;
            let measured = elapsed(cid);
            let latest = absolute_msec() - start;
            assert!((earliest..=latest).contains(&measured));
            assert!(measured >= 2);
        }
        assert_eq!(elapsed(c_long::MAX), -1);
        let earliest = co.co().execute_usec(steady_usec());
        let measured = current_execute_time();
        let latest = co.co().execute_usec(steady_usec());
        assert!((earliest..=latest).contains(&measured));
    });
    assert_eq!(elapsed(cid), -1);
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
            co_sleep(0.002);
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
        // The gdb helpers, in a coroutine.
        assert_eq!(swoole_coroutine_count(), 1);
        assert_eq!(swoole_coroutine_get(cid), current.cast());
        assert!(swoole_coroutine_get(999_999).is_null());

        swoole_coroutine_iterator_reset();
        assert_eq!(swoole_coroutine_iterator_each(), current.cast());
        assert!(swoole_coroutine_iterator_each().is_null());

        swoole_coroutine_iterator_reset();
        assert_eq!(swoole_coroutine_iterator_each(), current.cast());
        print_list();
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
            // The loop runs the deferred call before `co_run` returns, while `deferred`
            // is live.
            let task = DeferredCall(data);
            assert!(swoole_rs_ffi::event_api::swoole_event_defer_value(task).is_ok());
            exit.set(true);
        });

        assert!(exit.get());
        assert!(end.get());
    });
    assert_eq!(wexitstatus(status), 0);
}

extern "C" fn ignore(_: &AsyncEvent) {}

fn undefined_behavior() {
    let status = spawn_exec_and_wait(|| {
        // test::coroutine::run.
        super::run(|| {
            let _ = swoole_fork(0);
        });
    });
    assert_eq!(1, wexitstatus(status));

    let status = spawn_exec_and_wait(|| {
        let handle_count = AtomicI32::new(0);
        // The handler counts into `handle_count`, which outlives the process.
        let event = new_event(
            (&raw const handle_count).cast_mut().cast(),
            Some(count_handle),
            Some(ignore),
        );

        _ = swoole_event_init(0);
        let ret = dispatch(&event);
        assert!(!ret.is_null());
        let _ = swoole_fork(0);
    });
    assert_eq!(1, wexitstatus(status));

    assert_eq!(0, swoole_fork(SW_FORK_PRECHECK));
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

/// `coroutine_async_file.async_file`. `AsyncFile` left the build with its last
/// caller; each member was one hooked call on its descriptor, so the case makes
/// those calls, in the TEST's order.
fn async_file() {
    co_run(|| {
        // SAFETY: the hooked file functions over a descriptor this case opens and
        // closes, and buffers that outlive each call.
        unsafe {
            let fd = swoole_coroutine_open(c"/tmp/file.txt".as_ptr(), O_CREAT | O_RDWR, 0o666);
            // ready()
            assert_ne!(fd, -1);
            assert_eq!(swoole_coroutine_ftruncate(fd, 0), 0);
            assert_eq!(swoole_coroutine_lseek(fd, 0, SEEK_SET), 0);
            assert_eq!(swoole_coroutine_lseek(fd, 0, SEEK_CUR), 0);

            let data = b"Hello World!";
            let length = data.len();
            assert_eq!(
                swoole_coroutine_write(fd, data.as_ptr().cast(), length),
                length as isize
            );

            assert_eq!(swoole_coroutine_fsync(fd), 0);
            assert_eq!(swoole_coroutine_lseek(fd, 0, SEEK_SET), 0);

            let mut buf = [0u8; 1024];
            assert_eq!(
                swoole_coroutine_read(fd, buf.as_mut_ptr().cast(), 1024),
                length as isize
            );
            assert_eq!(&buf[..length], data);

            let mut statbuf: Stat = [0; 32];
            assert_eq!(swoole_coroutine_fstat(fd, &mut statbuf), 0);
            assert!(statbuf[ST_SIZE] > 0);

            // ~AsyncFile
            swoole_coroutine_close(fd);
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
