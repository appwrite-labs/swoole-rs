//! The hooks async gate's 426 scenarios, ported from `tests/modules/hooks/async_gate.cc`.
//! Upstream's `coroutine::async`, `async(fn)`, `System::sleep`, `gethostbyname_impl_with_async`
//! and `System::getaddrinfo` run in `swoole_rs_oracle::hooks_async` over a recording facade;
//! here the hybrid's offload (`swr_v1_hook_async`, `swr_v1_hook_async_job`,
//! `sleep_timeout`, `swr_v1_core_resolver_finish`), composed as system.cc's
//! glue composes it, runs over the same facade, and every scenario's effects must match.
use super::*;
use crate::sleep_timeout;
use std::cell::{Cell, RefCell, UnsafeCell};
use std::rc::Rc;
use swoole_rs_corebase::swr_v1_core_resolver_finish;

const AIO_TIMEOUT: c_int = 1779;
const RESOLVE_TIMEOUT: c_int = 1780;
/// `AsyncEvent{}`.
const EMPTY: AsyncEvent =
    AsyncEvent::request(core::ptr::null_mut(), None, None, core::ptr::null_mut());

/// The fake state, reset per scenario as the gate's `CASES` macro reset it.
#[derive(Clone, Copy)]
struct Knobs {
    dispatch_ok: bool,
    yield_ok: bool,
    context_canceled: bool,
    worker_canceled: bool,
    worker_error: c_int,
    request_error: c_int,
    worker_result: isize,
}

/// Each scenario starts here.
const DEFAULTS: Knobs = Knobs {
    dispatch_ok: true,
    yield_ok: true,
    context_canceled: false,
    worker_canceled: false,
    worker_error: libc::EIO,
    request_error: 0,
    worker_result: 1,
};

/// `GethostbynameRequest` and `GetaddrinfoRequest` in one: the shared request an
/// event's `data` owns; its last owner records the delete.
#[derive(Default)]
struct Request {
    addr: RefCell<String>,
    error: Cell<c_int>,
    ready: Cell<bool>,
}

impl Drop for Request {
    fn drop(&mut self) {
        record("request:delete".into());
    }
}

thread_local! {
    static TRACE: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static KNOBS: Cell<Knobs> = const { Cell::new(DEFAULTS) };
    static QUEUED: Cell<bool> = const { Cell::new(false) };
    static LAST_ERROR: Cell<c_int> = const { Cell::new(0) };
    /// The pool's copy of the event, and the request its `data` shares.
    static WORKER: UnsafeCell<AsyncEvent> = const { UnsafeCell::new(EMPTY) };
    static WORKER_REQUEST: RefCell<Option<Rc<Request>>> = const { RefCell::new(None) };
    /// The request the caller's event holds while it dispatches.
    static CALLER_REQUEST: RefCell<Option<Rc<Request>>> = const { RefCell::new(None) };
}

fn record(line: String) {
    TRACE.with_borrow_mut(|trace| trace.push(line));
}
fn knobs() -> Knobs {
    KNOBS.get()
}
fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}
fn flag(value: bool) -> u8 {
    u8::from(value)
}
fn worker() -> *mut AsyncEvent {
    WORKER.with(UnsafeCell::get)
}
fn reset_worker() {
    // SAFETY: no callback holds the worker between scenarios.
    unsafe { *worker() = EMPTY };
    drop(WORKER_REQUEST.take());
}

fn complete_worker() {
    if !QUEUED.replace(false) {
        return;
    }
    let event = worker();
    // SAFETY: the dispatched copy, live for the thread; the handlers run as the pool would.
    unsafe {
        if let Some(handler) = (*event).handler
            && (*event).canceled == 0
        {
            handler(event);
        }
        if let Some(callback) = (*event).callback {
            callback(event);
        }
    }
}

extern "C" fn current() -> *mut c_void {
    record("current".into());
    core::ptr::dangling_mut()
}
unsafe extern "C" fn dispatch(event: *const AsyncEvent) -> *mut AsyncEvent {
    // SAFETY: the offload passes its complete event.
    let event = unsafe { &*event };
    record(format!(
        "dispatch:{}:{}",
        flag(event.handler.is_some()),
        flag(event.callback.is_some())
    ));
    let k = knobs();
    if !k.dispatch_ok {
        return core::ptr::null_mut();
    }
    let copy = worker();
    // SAFETY: plain copy into the thread's worker slot, as `worker = *event` does.
    unsafe {
        *copy = AsyncEvent::request(
            event.object,
            event.handler,
            event.callback,
            event.pipe_socket,
        );
        (*copy).canceled = flag(k.worker_canceled);
        (*copy).error = k.worker_error;
        (*copy).retval = k.worker_result;
    }
    // The copy shares the caller's request, releasing the one it replaced.
    let shared = CALLER_REQUEST.with_borrow(Clone::clone);
    drop(WORKER_REQUEST.replace(shared));
    QUEUED.set(true);
    copy
}
unsafe extern "C" fn park(_: *mut c_void) {
    record("yield".into());
    complete_worker();
}
unsafe extern "C" fn park_for(_: *mut c_void, seconds: f64) -> bool {
    record(format!("yield-ex:{}", seconds.to_bits()));
    let ok = knobs().yield_ok;
    if ok {
        complete_worker();
    }
    ok
}
unsafe extern "C" fn resume(_: *mut c_void) {
    record("resume".into());
}
extern "C" fn last_error() -> c_int {
    let value = LAST_ERROR.get();
    record(format!("last:get:{value}"));
    value
}
fn set_last_error(value: c_int) {
    LAST_ERROR.set(value);
    record(format!("last:set:{value}"));
}
fn is_canceled() -> bool {
    let canceled = knobs().context_canceled;
    record(format!("canceled:{}", flag(canceled)));
    canceled
}

const OPS: AsyncOps = AsyncOps {
    current,
    dispatch,
    park,
    park_for,
    resume,
    last_error,
};

unsafe extern "C" fn typed_handler(event: *mut AsyncEvent) {
    record("handler:typed".into());
    let k = knobs();
    // SAFETY: the dispatched copy.
    unsafe {
        (*event).error = k.worker_error;
        (*event).retval = k.worker_result;
    }
}
unsafe extern "C" fn handler_gethostbyname(event: *mut AsyncEvent) {
    record("handler:host".into());
    WORKER_REQUEST.with_borrow(|request| {
        if let Some(request) = request {
            *request.addr.borrow_mut() = "127.0.0.1".into();
        }
    });
    let k = knobs();
    // SAFETY: the dispatched copy.
    unsafe {
        (*event).retval = k.worker_result;
        (*event).error = k.worker_error;
    }
}
unsafe extern "C" fn handler_getaddrinfo(event: *mut AsyncEvent) {
    record("handler:addr".into());
    let k = knobs();
    WORKER_REQUEST.with_borrow(|request| {
        if let Some(request) = request {
            request.ready.set(true);
            request.error.set(k.request_error);
        }
    });
    // SAFETY: the dispatched copy.
    unsafe {
        (*event).retval = k.worker_result;
        (*event).error = k.worker_error;
    }
}
unsafe extern "C" fn lambda(_: *mut c_void) {
    record("lambda:execute".into());
    set_errno(knobs().worker_error);
}

/// `coroutine::async(handler, event, timeout)` as system.cc wraps it.
fn async_event(
    handler: Handler,
    event: &mut AsyncEvent,
    request: Option<&Rc<Request>>,
    timeout: f64,
) -> bool {
    CALLER_REQUEST.set(request.cloned());
    // SAFETY: a valid event and ops table, inside the fake coroutine.
    let result = unsafe { swr_v1_hook_async(event, handler, timeout, &OPS) };
    drop(CALLER_REQUEST.take());
    result
}

/// `System::sleep` as the hybrid's system.cc composes it.
fn sleep(seconds: f64) -> c_int {
    let co = current();
    let seconds = sleep_timeout(seconds, 0.001);
    // SAFETY: fake.
    unsafe { park_for(co, seconds) };
    if is_canceled() { -1 } else { 0 }
}

/// `gethostbyname_impl_with_async` as the hybrid's system.cc composes it.
fn gethostbyname(name: &str, domain: c_int, timeout: f64) -> String {
    let mut event = EMPTY;
    record(format!("host-request:{name}:{domain}"));
    let request = Rc::new(Request::default());
    event.retval = 1;
    async_event(handler_gethostbyname, &mut event, Some(&request), timeout);
    // SAFETY: no request error slot.
    let completion = unsafe {
        swr_v1_core_resolver_finish(
            event.retval,
            event.error,
            core::ptr::null(),
            AIO_TIMEOUT,
            RESOLVE_TIMEOUT,
        )
    };
    let result = if completion.failed {
        set_last_error(completion.error);
        String::new()
    } else {
        request.addr.borrow().clone()
    };
    drop(request);
    result
}

/// `System::getaddrinfo` as the hybrid's system.cc composes it.
fn getaddrinfo(
    name: &str,
    family: c_int,
    kind: c_int,
    protocol: c_int,
    service: &str,
    timeout: f64,
) -> Vec<String> {
    let mut event = EMPTY;
    record(format!(
        "addr-request:{name}:{family}:{kind}:{protocol}:{service}"
    ));
    let request = Rc::new(Request::default());
    async_event(handler_getaddrinfo, &mut event, Some(&request), timeout);
    let mut values = Vec::new();
    let error = request.error.get();
    // SAFETY: the request's error slot, settled once the wait succeeded.
    let completion = unsafe {
        swr_v1_core_resolver_finish(
            event.retval,
            event.error,
            &raw const error,
            AIO_TIMEOUT,
            RESOLVE_TIMEOUT,
        )
    };
    if completion.failed {
        set_last_error(completion.error);
    } else {
        record("addr:parse".into());
        if request.ready.get() {
            values.push("native-result".to_string());
        }
    }
    drop(request);
    values
}

fn late_completion() {
    if QUEUED.get() {
        record("late-completion".into());
        complete_worker();
    }
}

/// One gate scenario through the hybrid's offload, as `CASES(selected)` ran it.
fn run(scenario: i32) -> Vec<String> {
    TRACE.with_borrow_mut(Vec::clear);
    set_errno(libc::E2BIG);
    let mut k = DEFAULTS;
    QUEUED.set(false);
    LAST_ERROR.set(libc::ECANCELED);
    reset_worker();
    let (inet, inet6) = (libc::AF_INET, libc::AF_INET6);
    let index = |value: i32| usize::try_from(value).unwrap_or_default();
    if scenario < 96 {
        let n = scenario;
        k.dispatch_ok = n & 1 != 0;
        k.yield_ok = n & 2 != 0;
        k.worker_canceled = n & 4 != 0;
        k.worker_error = [0, libc::ETIMEDOUT, libc::EIO][index((n / 8) % 3)];
        k.worker_result = [-1, 0, 1, 123][index(n / 24)];
        KNOBS.set(k);
        let mut event = EMPTY;
        let result = async_event(typed_handler, &mut event, None, 0.25);
        record(format!("result:{}", flag(result)));
        record(format!("caller-canceled:{}", event.canceled));
        record(format!("caller-retval:{}", event.retval));
        record(format!("caller-error:{}", event.error));
        // SAFETY: the worker slot, read between callbacks.
        record(format!("worker-canceled:{}", unsafe {
            (*worker()).canceled
        }));
        record(format!("errno:{}", errno()));
        late_completion();
    } else if scenario < 102 {
        let n = scenario - 96;
        k.dispatch_ok = n & 1 != 0;
        k.worker_error = [0, libc::EIO, libc::EINVAL][index((n / 2) % 3)];
        KNOBS.set(k);
        // SAFETY: `lambda` runs anywhere; inside the fake coroutine.
        let result = unsafe { swr_v1_hook_async_job(lambda, core::ptr::null_mut(), &OPS) };
        record(format!("result:{}", flag(result)));
        record(format!("errno:{}", errno()));
    } else if scenario < 138 {
        let n = scenario - 102;
        let seconds = [
            f64::NEG_INFINITY,
            -1.0,
            -0.0,
            0.0,
            0.0001,
            0.001,
            1.0,
            f64::NAN,
            f64::INFINITY,
        ];
        k.context_canceled = (n / 9) % 2 != 0;
        k.yield_ok = n / 18 != 0;
        KNOBS.set(k);
        let slept = sleep(seconds[index(n % 9)]);
        record(format!("sleep:{slept}"));
        record(format!("errno:{}", errno()));
    } else {
        let addr = scenario >= 234;
        let n = if addr { scenario - 234 } else { scenario - 138 };
        k.worker_result = [-1, 0, 1][index(n % 3)];
        k.worker_error = [0, AIO_TIMEOUT, libc::EIO, libc::EINVAL][index((n / 3) % 4)];
        k.dispatch_ok = (n / 12) % 2 != 0;
        k.yield_ok = (n / 24) % 2 != 0;
        if addr {
            k.request_error = (n / 48) % 2;
            KNOBS.set(k);
            let family = if n / 96 != 0 { inet6 } else { inet };
            let values = getaddrinfo("private.test", family, libc::SOCK_STREAM, 0, "443", 0.5);
            record(format!("addresses:{}", values.len()));
            if let Some(first) = values.first() {
                record(first.clone());
            }
        } else {
            KNOBS.set(k);
            let domain = if n / 48 != 0 { inet6 } else { inet };
            let host = gethostbyname("private.test", domain, 0.5);
            record(format!("host:{host}"));
        }
        record(format!("errno:{}", errno()));
        record(format!("last:{}", LAST_ERROR.get()));
        late_completion();
    }
    reset_worker();
    TRACE.take()
}

unsafe extern "C" fn collect(context: *mut c_void, text: *const u8, length: usize) {
    // SAFETY: the oracle passes `length` readable bytes and our Vec as context.
    let line = unsafe { core::slice::from_raw_parts(text, length) };
    // SAFETY: context is the caller's Vec.
    unsafe { &mut *context.cast::<Vec<String>>() }.push(String::from_utf8_lossy(line).into_owned());
}

fn upstream(scenario: i32) -> Vec<String> {
    let mut lines = Vec::new();
    // SAFETY: a scenario in range and a context `collect` understands.
    unsafe { swoole_rs_oracle::hooks_async::trace(scenario, collect, (&raw mut lines).cast()) };
    lines
}

#[test]
fn ported_async_gate() {
    for scenario in 0..426 {
        assert_eq!(
            run(scenario),
            upstream(scenario),
            "async scenario {scenario}"
        );
    }
}
