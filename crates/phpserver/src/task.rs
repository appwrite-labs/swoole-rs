//! `Swoole\Server`'s task family: `task`, `taskwait`, `taskWaitMulti`,
//! `taskCo`, `Swoole\Server\Task::pack`/`unpack`, and the `onTask`/`onFinish`
//! events. Rust owns the callbacks of pending tasks and the coroutines that
//! wait for results (upstream's `ServerProperty::task_callbacks` and
//! `task_coroutine_map`), and the task encoding (a string, or PHP's serialize
//! with `SW_TASK_SERIALIZE`); C++ keeps the Server, its `task_pack`,
//! `task_unpack` and `finish`, and the coroutine
//! (`native/process/swoole_rs_phpserver_task.h`).
use crate::dispatch::{handler_error, server_callback, server_zval, set_last_error};
use crate::server::{DATA, TASK, fatal, running};
use std::{
    cell::RefCell,
    collections::HashMap,
    ffi::{CStr, c_char, c_int, c_void},
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
};
use swoole_rs_zend::{
    Call, E_WARNING,
    bailout::Bailout,
    each_value, init_object, long_zval, message, object_of, php_error,
    string::bytes,
    sys::{
        IS_ARRAY, IS_NULL, add_index_bool, swr_zend_serialize, swr_zend_string_release,
        swr_zend_unserialize, swr_zval_is_true, swr_zval_stringl, zend_class_entry,
        zend_hash_index_find, zend_hash_index_update, zend_object, zend_update_property,
        zend_update_property_double, zend_update_property_long, zval, zval_ptr_dtor,
    },
    type_of,
    value::{array_init, count, table},
};

/// `SW_TASK_*` flags.
const SERIALIZE: u32 = 1 << 1;
const NONBLOCK: u32 = 1 << 2;
const CALLBACK: u32 = 1 << 3;
const COROUTINE: u32 = 1 << 5;
const NOREPLY: u32 = 1 << 7;
/// `SW_SERVER_CB_onTask`, `SW_SERVER_CB_onFinish`.
const ON_TASK: c_int = 7;
const ON_FINISH: c_int = 8;
/// `SW_MAX_CONCURRENT_TASK`, `SW_TASKWAIT_TIMEOUT`.
const MAX_CONCURRENT_TASK: u32 = 1024;
const TASKWAIT_TIMEOUT: f64 = 0.5;
/// `SW_ERROR_PHP_FATAL_ERROR`, `SW_ERROR_TASK_DISPATCH_FAIL`.
const SW_ERROR_PHP_FATAL_ERROR: c_int = 503;
const SW_ERROR_TASK_DISPATCH_FAIL: c_int = 2002;
const SW_OK: c_int = 0;
const SW_ERR: c_int = -1;
/// `DATA`'s `Swoole\Server\TaskResult`.
const TASK_RESULT: usize = 4;

/// `swr_phpserver_task_facts`.
#[repr(C)]
#[derive(Default)]
struct Facts {
    started: bool,
    worker: bool,
    task_worker: bool,
    task_object: bool,
    task_coroutine: bool,
    event_object: bool,
    enable_coroutine: bool,
    task_worker_num: i64,
}

/// `swr_phpserver_task_info`.
#[repr(C)]
#[derive(Default)]
struct Info {
    id: i64,
    worker_id: i64,
    time: f64,
    flags: i64,
}

/// `swr_v1_phpserver_task_host`.
#[repr(C)]
pub struct TaskHost {
    facts: unsafe extern "C" fn(*mut c_void, *mut Facts),
    pack: unsafe extern "C" fn(*mut c_void, *const c_char, usize) -> i64,
    unpack: unsafe extern "C" fn(*mut c_void, *mut *const c_char, *mut usize, *mut u32) -> bool,
    log_unserialize: unsafe extern "C" fn(i64, usize),
    flag: unsafe extern "C" fn(*mut c_void, u32),
    size: unsafe extern "C" fn(*mut c_void) -> usize,
    info: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut Info),
    dispatch: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_int) -> bool,
    dispatch_wait: unsafe extern "C" fn(
        *mut c_void,
        *mut c_void,
        *mut c_int,
        f64,
        *mut zval,
        *mut bool,
    ) -> bool,
    dispatch_multi: unsafe extern "C" fn(*mut c_void, *mut zval, f64, *mut zval, *mut bool) -> bool,
    tasking_done: unsafe extern "C" fn(*mut c_void),
    bind_task: unsafe extern "C" fn(*mut zend_object, *mut c_void, *mut c_void),
    finish: unsafe extern "C" fn(*mut c_void, *const c_char, usize, c_int, *mut c_void) -> bool,
    callable_create: unsafe extern "C" fn(*mut zval) -> *mut c_void,
    callable_free: unsafe extern "C" fn(*mut c_void),
    call: unsafe extern "C" fn(*mut c_void, u32, *mut zval, *mut zval, bool) -> c_int,
    co_current: unsafe extern "C" fn(*mut bool) -> *mut c_void,
    co_in: unsafe extern "C" fn() -> bool,
    co_yield: unsafe extern "C" fn(*mut c_void, f64) -> bool,
    co_resume: unsafe extern "C" fn(*mut c_void),
    log_expired: unsafe extern "C" fn(i64),
}

static HOST: AtomicPtr<TaskHost> = AtomicPtr::new(ptr::null_mut());

#[inline]
fn host() -> &'static TaskHost {
    // SAFETY: MINIT installs C++'s static table before any method or event runs.
    unsafe { &*HOST.load(Ordering::Acquire) }
}

/// MINIT: the table the task family calls.
/// # Safety
/// `table` lives for the process.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpserver_task_init(table: *const TaskHost) {
    HOST.store(table.cast_mut(), Ordering::Release);
}

/// An `EventData`, which C++ asserts fits.
#[repr(C, align(8))]
pub(crate) struct EventData([u8; 8192]);

impl EventData {
    pub(crate) fn new() -> Self {
        Self([0; 8192])
    }

    pub(crate) fn ptr(&mut self) -> *mut c_void {
        self.0.as_mut_ptr().cast()
    }
}

/// Upstream's `TaskCo`: a coroutine waiting in `taskwait` (`list` empty) or
/// `taskCo`/`taskWaitMulti` for results written into `result`.
struct TaskCo {
    co: *mut c_void,
    result: *mut zval,
    list: Vec<i64>,
    count: u32,
    /// `taskCo` fills timed-out results with false; `taskWaitMulti` leaves them unset.
    fill_false: bool,
}

/// One server's pending tasks.
#[derive(Default)]
struct Pending {
    callbacks: HashMap<i64, *mut c_void>,
    coroutines: HashMap<i64, *mut TaskCo>,
}

thread_local! {
    /// Per Server (by address), as upstream's `ServerProperty` is per object.
    static PENDING: RefCell<HashMap<usize, Pending>> = RefCell::new(HashMap::new());
}

fn pending<R>(server: *mut c_void, f: impl FnOnce(&mut Pending) -> R) -> R {
    PENDING.with_borrow_mut(|all| f(all.entry(server as usize).or_default()))
}

fn facts(server: *mut c_void) -> Facts {
    let mut facts = Facts::default();
    // SAFETY: a live server and a writable view.
    unsafe { (host().facts)(server, &raw mut facts) };
    facts
}

fn info(server: *mut c_void, req: *mut c_void) -> Info {
    let mut info = Info::default();
    // SAFETY: a live server and its task.
    unsafe { (host().info)(server, req, &raw mut info) };
    info
}

fn undef() -> zval {
    // SAFETY: a zval is plain data; all-zero bytes are `IS_UNDEF`.
    unsafe { std::mem::zeroed() }
}

fn dtor(value: &mut zval) {
    // SAFETY: a value this module owns.
    unsafe { zval_ptr_dtor(value) };
}

/// The bytes a task carries: a string as it is, anything else serialized
/// (`None`: serialize wrote nothing). `each` sees the bytes and whether they
/// were serialized.
fn encoded<R>(data: *mut zval, each: impl FnOnce(&[u8], bool) -> R) -> Result<Option<R>, Bailout> {
    // SAFETY: a live value.
    if let Some(text) = unsafe { swoole_rs_zend::string_of(data) } {
        return Ok(Some(each(text, false)));
    }
    let mut bailed = false;
    // SAFETY: a live value and a writable flag; the string is ours.
    let serialized = unsafe { swr_zend_serialize(data, &raw mut bailed) };
    if bailed {
        return Err(Bailout);
    }
    if serialized.is_null() {
        return Ok(None);
    }
    // SAFETY: the string serialize made.
    let result = each(unsafe { bytes(serialized) }, true);
    // SAFETY: as above, released once.
    unsafe { swr_zend_string_release(serialized) };
    Ok(Some(result))
}

/// `php_swoole_server_task_pack`: pack `data` into `buf`; the task id, or -1.
fn encode(data: *mut zval, buf: *mut c_void) -> Result<i64, Bailout> {
    let host = host();
    let packed = encoded(data, |payload, serialized| {
        // SAFETY: a writable `EventData` and the payload's bytes.
        let id = unsafe { (host.pack)(buf, payload.as_ptr().cast(), payload.len()) };
        (id, serialized)
    })?;
    let Some((id, serialized)) = packed else {
        return Ok(-1);
    };
    if id < 0 {
        event_warning(c"large task pack failed")?;
    }
    if serialized {
        // SAFETY: the `EventData` packed above.
        unsafe { (host.flag)(buf, SERIALIZE) };
    }
    Ok(id)
}

/// `php_swoole_server_task_unpack`: the task in `buf` into `out`.
fn decode(buf: *mut c_void, out: *mut zval) -> Result<bool, Bailout> {
    let host = host();
    let (mut data, mut length, mut flags) = (ptr::null(), 0, 0);
    // SAFETY: a packed task and writable views of its payload.
    if !unsafe { (host.unpack)(buf, &raw mut data, &raw mut length, &raw mut flags) } {
        return Ok(false);
    }
    if flags & SERIALIZE == 0 {
        // SAFETY: a writable zval and the payload's bytes.
        unsafe { swr_zval_stringl(out, data, length) };
        return Ok(true);
    }
    let (mut offset, mut bailed) = (0, false);
    // SAFETY: as above.
    let done = unsafe { swr_zend_unserialize(out, data, length, &raw mut offset, &raw mut bailed) };
    if bailed {
        return Err(Bailout);
    }
    if !done {
        // SAFETY: a log line C++ formats.
        unsafe { (host.log_unserialize)(i64::try_from(offset).unwrap_or(i64::MAX), length) };
    }
    Ok(done)
}

/// `php_swoole_server_task_finish`: send `data` as the result of `req` (null:
/// this worker's current task).
pub(crate) fn finish_task(
    server: *mut c_void,
    data: *mut zval,
    req: *mut c_void,
) -> Result<bool, Bailout> {
    let finished = encoded(data, |payload, serialized| {
        let flags = if serialized {
            SERIALIZE.cast_signed()
        } else {
            0
        };
        // SAFETY: a live server, the payload's bytes and the task, or null.
        unsafe { (host().finish)(server, payload.as_ptr().cast(), payload.len(), flags, req) }
    })?;
    Ok(finished.unwrap_or(false))
}

/// `encode` for C++ (`sendMessage`, `taskWaitMulti`'s tasks); `*bailed` when
/// it bailed out.
/// # Safety
/// `data` is a live value, `buf` a writable `EventData` and `bailed` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpserver_task_pack(
    data: *mut zval,
    buf: *mut c_void,
    bailed: *mut bool,
) -> i64 {
    encode(data, buf).unwrap_or_else(|Bailout| {
        // SAFETY: per the caller.
        unsafe { bailed.write(true) };
        -1
    })
}

/// `decode` for C++ (`onPipeMessage`, `taskwait`'s and `taskWaitMulti`'s results).
/// # Safety
/// `buf` is a packed task, `out` a writable zval and `bailed` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpserver_task_unpack(
    buf: *mut c_void,
    out: *mut zval,
    bailed: *mut bool,
) -> bool {
    decode(buf, out).unwrap_or_else(|Bailout| {
        // SAFETY: per the caller.
        unsafe { bailed.write(true) };
        false
    })
}

/// `php_swoole_server_task_pack`: the task id, or -1; a trapped bailout waits
/// for the handler's end.
pub(crate) fn pack(data: *mut zval, buf: &mut EventData) -> i64 {
    encode(data, buf.ptr()).unwrap_or_else(|bailout| {
        bailout.defer();
        -1
    })
}

fn flag(buf: &mut EventData, flags: u32) {
    // SAFETY: a packed `EventData`.
    unsafe { (host().flag)(buf.ptr(), flags) };
}

fn dispatch(server: *mut c_void, buf: &mut EventData, dst: &mut c_int) -> bool {
    // SAFETY: a live server and a packed task.
    unsafe { (host().dispatch)(server, buf.ptr(), dst) }
}

/// `php_swoole_server_task_check_param`.
fn check(facts: &Facts, dst_worker_id: i64) -> bool {
    if facts.task_worker_num == 0 {
        fatal(
            E_WARNING,
            c"task method can't be executed without task worker",
        );
        return false;
    }
    if dst_worker_id > 0 && dst_worker_id >= facts.task_worker_num {
        let text = message!(
            "worker_id must be less than task_worker_num[{}]",
            facts.task_worker_num
        );
        fatal(E_WARNING, text.as_cstr());
        return false;
    }
    if facts.task_worker {
        fatal(E_WARNING, c"Server->task() cannot use in the task-worker");
        return false;
    }
    true
}

/// `running(call)` and a worker process, or a warning naming `method`.
fn worker(call: &Call, method: &CStr) -> Option<(*mut c_void, Facts)> {
    let serv = running(call)?;
    let facts = facts(serv);
    if !facts.worker {
        let text = message!(
            "{} method can only be used in the worker process",
            method.to_string_lossy()
        );
        fatal(E_WARNING, text.as_cstr());
        call.set_bool(false);
        return None;
    }
    Some((serv, facts))
}

fn task(call: &Call) {
    let Some(serv) = running(call) else { return };
    let (Ok(()), Ok(dst)) = (call.arity(1, 3), call.long(2, -1)) else {
        return call.set_bool(false);
    };
    let callback = (call.num_args() >= 3).then(|| call.zval(3));
    let facts = facts(serv);
    if !check(&facts, dst) {
        return call.set_bool(false);
    }
    let mut buf = EventData::new();
    let id = pack(call.zval(1), &mut buf);
    if id < 0 {
        return call.set_bool(false);
    }
    // SAFETY: a live argument.
    let callback = callback.filter(|&zfn| unsafe { swr_zval_is_true(zfn) });
    if !facts.worker {
        flag(&mut buf, NOREPLY);
    } else if let Some(callback) = callback {
        flag(&mut buf, CALLBACK);
        // SAFETY: a live argument; C++ warns when it is not callable.
        let cb = unsafe { (host().callable_create)(callback) };
        if cb.is_null() {
            return call.set_bool(false);
        }
        if let Some(old) = pending(serv, |p| p.callbacks.insert(id, cb)) {
            // SAFETY: a callable this module created.
            unsafe { (host().callable_free)(old) };
        }
    }
    flag(&mut buf, NONBLOCK);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream passes (int *) &dst_worker_id"
    )]
    let mut dst = dst as c_int;
    if dispatch(serv, &mut buf, &mut dst) {
        call.set_long(id);
    } else {
        call.set_bool(false);
    }
}

fn taskwait(call: &Call) {
    let Some((serv, facts)) = worker(call, c"taskwait") else {
        return;
    };
    let (Ok(()), Ok(timeout), Ok(dst)) = (
        call.arity(1, 3),
        call.double(2, TASKWAIT_TIMEOUT),
        call.long(3, -1),
    ) else {
        return call.set_bool(false);
    };
    if !check(&facts, dst) {
        return call.set_bool(false);
    }
    let mut buf = EventData::new();
    let id = pack(call.zval(1), &mut buf);
    if id < 0 {
        return call.set_bool(false);
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream passes (int *) &dst_worker_id"
    )]
    let mut dst = dst as c_int;
    let host = host();
    // SAFETY: plain state.
    if !unsafe { (host.co_in)() } {
        let mut bailed = false;
        // SAFETY: a live server, a packed task and the return slot.
        let done = unsafe {
            (host.dispatch_wait)(
                serv,
                buf.ptr(),
                &raw mut dst,
                timeout,
                call.return_value(),
                &raw mut bailed,
            )
        };
        if bailed {
            Bailout.defer();
        }
        if !done {
            call.set_bool(false);
        }
        return;
    }
    flag(&mut buf, NONBLOCK | COROUTINE);
    let mut bailed = false;
    // SAFETY: inside a coroutine.
    let co = unsafe { (host.co_current)(&raw mut bailed) };
    let mut task_co = TaskCo {
        co,
        result: call.return_value(),
        list: Vec::new(),
        count: 1,
        fill_false: false,
    };
    if !dispatch(serv, &mut buf, &mut dst) {
        return call.set_bool(false);
    }
    pending(serv, |p| p.coroutines.insert(id, &raw mut task_co));
    // SAFETY: the current coroutine; onFinish resumes it.
    let done = unsafe { (host.co_yield)(task_co.co, timeout) };
    pending(serv, |p| p.coroutines.remove(&id));
    if !done {
        call.set_bool(false);
    }
}

fn task_wait_multi(call: &Call) {
    // SAFETY: plain state.
    if unsafe { (host().co_in)() } {
        return task_co(call, false);
    }
    let Some((serv, facts)) = worker(call, c"taskWaitMulti") else {
        return;
    };
    let (Ok(()), Ok(timeout)) = (call.arity(1, 2), call.double(2, TASKWAIT_TIMEOUT)) else {
        return call.set_bool(false);
    };
    if !check(&facts, -1) {
        return call.set_bool(false);
    }
    let tasks = call.zval(1);
    // SAFETY: the return slot.
    unsafe { array_init(call.return_value()) };
    if length(tasks) >= MAX_CONCURRENT_TASK {
        fatal(E_WARNING, c"too many concurrent tasks");
        return call.set_bool(false);
    }
    let mut bailed = false;
    // SAFETY: a live server, the tasks and the result array.
    let done = unsafe {
        (host().dispatch_multi)(serv, tasks, timeout, call.return_value(), &raw mut bailed)
    };
    if bailed {
        Bailout.defer();
    }
    if !done {
        // SAFETY: the array this call made.
        unsafe { zval_ptr_dtor(call.return_value()) };
        call.set_bool(false);
    }
}

/// `php_swoole_array_length`, as 0 for what is not an array.
fn length(array: *mut zval) -> u32 {
    // SAFETY: a live argument.
    if unsafe { type_of(array) } != IS_ARRAY {
        return 0;
    }
    // SAFETY: an array.
    unsafe { count(array) }
}

fn task_co_method(call: &Call) {
    task_co(call, true);
}

/// `taskCo`, and `taskWaitMulti` inside a coroutine (`fill_false`: false).
fn task_co(call: &Call, fill_false: bool) {
    let Some((serv, facts)) = worker(call, c"taskCo") else {
        return;
    };
    let (Ok(()), Ok(timeout)) = (call.arity(1, 2), call.double(2, TASKWAIT_TIMEOUT)) else {
        return call.set_bool(false);
    };
    let tasks = call.zval(1);
    let mut n_task = length(tasks);
    if n_task >= MAX_CONCURRENT_TASK {
        fatal(E_WARNING, c"too many concurrent tasks");
        return call.set_bool(false);
    }
    if !check(&facts, -1) {
        return call.set_bool(false);
    }
    let host = host();
    let mut bailed = false;
    // SAFETY: `Coroutine::get_current_safe()`, its fatal error trapped.
    let co = unsafe { (host.co_current)(&raw mut bailed) };
    if bailed {
        return Bailout.defer();
    }
    let result = call.return_value();
    // SAFETY: the return slot.
    unsafe { array_init(result) };
    let mut task_co = TaskCo {
        co,
        result,
        list: Vec::new(),
        count: 0,
        fill_false,
    };
    let mut list = Vec::with_capacity(n_task as usize);
    if n_task > 0 {
        // SAFETY: an array argument.
        let tasks = unsafe { table(tasks) };
        let each = |ztask: *mut zval| {
            let index = list.len() as u64;
            let mut buf = EventData::new();
            let mut id = pack(ztask, &mut buf);
            let mut dispatched = false;
            if id < 0 {
                fatal(E_WARNING, c"failed to pack task");
            } else {
                flag(&mut buf, NONBLOCK | COROUTINE);
                let mut dst: c_int = -1;
                if dispatch(serv, &mut buf, &mut dst) {
                    dispatched = true;
                } else {
                    id = -1;
                }
            }
            if dispatched {
                pending(serv, |p| p.coroutines.insert(id, &raw mut task_co));
            } else {
                // SAFETY: the result array.
                unsafe { add_index_bool(result, index, false) };
                n_task -= 1;
            }
            list.push(id);
            true
        };
        // SAFETY: a live array, not modified by the loop.
        unsafe { each_value(tasks, each) };
    }
    if n_task == 0 {
        set_last_error(SW_ERROR_TASK_DISPATCH_FAIL);
        // SAFETY: the array this call made.
        unsafe { zval_ptr_dtor(result) };
        return call.set_bool(false);
    }
    task_co.list = list;
    task_co.count = n_task;
    // SAFETY: the current coroutine; onFinish resumes it.
    if unsafe { (host.co_yield)(co, timeout) } {
        return;
    }
    for j in 0..n_task as usize {
        // SAFETY: the result array.
        let array = unsafe { table(result) };
        // SAFETY: a live array.
        if unsafe { zend_hash_index_find(array, j as u64) }.is_null() {
            if task_co.fill_false {
                // SAFETY: as above.
                unsafe { add_index_bool(result, j as u64, false) };
            }
            let id = task_co.list[j];
            pending(serv, |p| p.coroutines.remove(&id));
        }
    }
}

fn pack_method(call: &Call) {
    if call.arity(1, 1).is_err() {
        return call.set_bool(false);
    }
    let mut buf = EventData::new();
    if pack(call.zval(1), &mut buf) < 0 {
        return call.set_bool(false);
    }
    flag(&mut buf, NONBLOCK | NOREPLY);
    // SAFETY: the packed task.
    let size = unsafe { (host().size)(buf.ptr()) };
    // RETURN_STRINGL: `size` bytes of the buffer, copied.
    // SAFETY: the return slot and the buffer's bytes.
    unsafe {
        swoole_rs_zend::sys::swr_zval_stringl(call.return_value(), buf.0.as_ptr().cast(), size);
    };
}

fn unpack_method(call: &Call) {
    if call.arity(1, 1).is_err() {
        return call.set_bool(false);
    }
    let data = call.zval(1);
    // Upstream reads the argument's bytes as an EventData without a type check.
    // SAFETY: a string argument (as upstream assumes).
    let buf = unsafe { swoole_rs_zend::value::str_of(data) };
    // SAFETY: the string's bytes.
    let bytes = unsafe { (*buf).val.as_mut_ptr() };
    // As upstream: the bytes hold a packed task.
    let done = decode(bytes.cast(), call.return_value()).unwrap_or_else(|bailout| {
        bailout.defer();
        false
    });
    if !done {
        call.set_bool(false);
    }
}

swoole_rs_zend::handlers! {
    zim_swoole_server_task => task;
    zim_swoole_server_taskwait => taskwait;
    zim_swoole_server_taskWaitMulti => task_wait_multi;
    zim_swoole_server_taskCo => task_co_method;
    zim_swoole_server_task_pack => pack_method;
    zim_swoole_server_task_unpack => unpack_method;
}

/// `php_swoole_fatal_error(E_WARNING, ...)` from an event.
fn event_warning(text: &CStr) -> Result<(), Bailout> {
    set_last_error(SW_ERROR_PHP_FATAL_ERROR);
    // SAFETY: a warning, from an event Zend runs.
    unsafe { php_error(E_WARNING, text) }
}

fn set_long(ce: *mut zend_class_entry, object: *mut zend_object, name: &CStr, value: i64) {
    // SAFETY: a live object of `ce`; Zend copies the name.
    unsafe { zend_update_property_long(ce, object, name.as_ptr(), name.count_bytes(), value) };
}

fn set_double(ce: *mut zend_class_entry, object: *mut zend_object, name: &CStr, value: f64) {
    // SAFETY: as above.
    unsafe { zend_update_property_double(ce, object, name.as_ptr(), name.count_bytes(), value) };
}

fn set_value(ce: *mut zend_class_entry, object: *mut zend_object, name: &CStr, value: *mut zval) {
    // SAFETY: as above; the property takes its own reference.
    unsafe { zend_update_property(ce, object, name.as_ptr(), name.count_bytes(), value) };
}

/// Unpack `req` into a value, or `None` (`Err` for a trapped bailout).
fn unpack(req: *mut c_void) -> Result<Option<zval>, Bailout> {
    let mut data = undef();
    Ok(decode(req, &raw mut data)?.then_some(data))
}

/// `php_swoole_server_onTask`; true when C++ must re-raise a bailout.
/// # Safety
/// A live server and the task it received; `result` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpserver_on_task(
    server: *mut c_void,
    req: *mut c_void,
    result: *mut c_int,
) -> bool {
    let outcome = on_task(server, req);
    // SAFETY: per the caller.
    unsafe { result.write(*outcome.as_ref().unwrap_or(&SW_OK)) };
    outcome.is_err()
}

fn on_task(server: *mut c_void, req: *mut c_void) -> Result<c_int, Bailout> {
    let host = host();
    // SAFETY: a live server.
    unsafe { (host.tasking_done)(server) };
    let zserv = server_zval(server);
    let Some(mut data) = unpack(req)? else {
        return Ok(SW_ERR);
    };
    let facts = facts(server);
    let info = info(server, req);
    let mut args = [zserv, undef(), undef(), undef()];
    let argc = if facts.task_object {
        let ce = TASK.entry();
        // SAFETY: a writable zval and the registered Task class.
        unsafe { init_object(&raw mut args[1], ce) };
        // SAFETY: an object zval.
        let object = unsafe { object_of(&raw const args[1]) };
        // SAFETY: a new Task object, the server and its task.
        unsafe { (host.bind_task)(object, server, req) };
        set_long(ce, object, c"worker_id", info.worker_id);
        set_long(ce, object, c"id", info.id);
        set_value(ce, object, c"data", &raw mut data);
        set_double(ce, object, c"dispatch_time", info.time);
        set_long(ce, object, c"flags", info.flags);
        2
    } else {
        args[1] = long_zval(info.id);
        args[2] = long_zval(info.worker_id);
        args[3] = data;
        4
    };
    let mut retval = undef();
    let cb = server_callback(server, ON_TASK);
    // SAFETY: the server's onTask callable and `argc` initialised arguments.
    let called = unsafe {
        (host.call)(
            cb,
            argc,
            args.as_mut_ptr(),
            &raw mut retval,
            facts.task_coroutine,
        )
    };
    let outcome = match called {
        2 => Err(Bailout),
        0 => handler_error(&zserv, "onTask"),
        _ => Ok(()),
    };
    if argc == 2 {
        dtor(&mut args[1]);
    }
    let outcome = outcome.and_then(|()| {
        // SAFETY: the callback's result, or undef.
        if unsafe { type_of(&raw const retval) } != IS_NULL {
            let finished = finish_task(server, &raw mut retval, req);
            dtor(&mut retval);
            finished?;
        }
        Ok(())
    });
    dtor(&mut data);
    outcome.map(|()| SW_OK)
}

/// `php_swoole_server_onFinish`; true when C++ must re-raise a bailout.
/// # Safety
/// A live server and the result it received; `result` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpserver_on_finish(
    server: *mut c_void,
    req: *mut c_void,
    result: *mut c_int,
) -> bool {
    let outcome = on_finish(server, req);
    // SAFETY: per the caller.
    unsafe { result.write(*outcome.as_ref().unwrap_or(&SW_OK)) };
    outcome.is_err()
}

fn on_finish(server: *mut c_void, req: *mut c_void) -> Result<c_int, Bailout> {
    let host = host();
    let zserv = server_zval(server);
    let info = info(server, req);
    let id = info.id;
    let Some(mut data) = unpack(req)? else {
        return Ok(SW_ERR);
    };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "ext_flags is a uint16_t"
    )]
    let mut flags = info.flags as u32;
    if flags & COROUTINE != 0 {
        return resume(server, id, data).map(|()| SW_OK);
    }
    let cb = if flags & CALLBACK != 0 {
        let cb = pending(server, |p| p.callbacks.get(&id).copied());
        if cb.is_none() {
            flags &= !CALLBACK;
        }
        cb.unwrap_or(ptr::null_mut())
    } else {
        server_callback(server, ON_FINISH)
    };
    if cb.is_null() {
        let outcome = event_warning(c"require 'onFinish' callback");
        dtor(&mut data);
        return outcome.map(|()| SW_ERR);
    }
    let facts = facts(server);
    let mut args = [zserv, undef(), undef()];
    let argc = if facts.event_object {
        let ce = DATA[TASK_RESULT].entry();
        // SAFETY: a writable zval and the registered TaskResult class.
        unsafe { init_object(&raw mut args[1], ce) };
        // SAFETY: an object zval.
        let object = unsafe { object_of(&raw const args[1]) };
        set_long(ce, object, c"task_id", id);
        set_long(ce, object, c"task_worker_id", info.worker_id);
        set_double(ce, object, c"dispatch_time", info.time);
        set_value(ce, object, c"data", &raw mut data);
        2
    } else {
        args[1] = long_zval(id);
        args[2] = data;
        3
    };
    // SAFETY: a live callable and `argc` initialised arguments.
    let called = unsafe {
        (host.call)(
            cb,
            argc,
            args.as_mut_ptr(),
            ptr::null_mut(),
            facts.enable_coroutine,
        )
    };
    let outcome = match called {
        2 => Err(Bailout),
        0 => handler_error(&zserv, "onFinish"),
        _ => Ok(()),
    };
    if flags & CALLBACK != 0
        && let Some(cb) = pending(server, |p| p.callbacks.remove(&id))
    {
        // SAFETY: a callable `task()` created.
        unsafe { (host.callable_free)(cb) };
    }
    if facts.event_object {
        dtor(&mut args[1]);
    }
    dtor(&mut data);
    outcome.map(|()| SW_OK)
}

/// A coroutine task's result: into `taskwait`'s return value, or at its index
/// in `taskCo`'s array; the waiting coroutine resumes once all arrived.
fn resume(server: *mut c_void, id: i64, mut data: zval) -> Result<(), Bailout> {
    let host = host();
    let Some(task_co) = pending(server, |p| p.coroutines.get(&id).copied()) else {
        // SAFETY: a log line.
        unsafe { (host.log_expired)(id) };
        dtor(&mut data);
        return Ok(());
    };
    // SAFETY: the waiting coroutine's live TaskCo, unregistered before it ends.
    let (co, result, index, total) = unsafe {
        let task_co = &*task_co;
        (
            task_co.co,
            task_co.result,
            task_co.list.iter().position(|&t| t == id),
            task_co.count,
        )
    };
    // SAFETY: as above.
    if unsafe { (*task_co).list.is_empty() } {
        // ZVAL_COPY_VALUE: the result takes the value.
        // SAFETY: the waiting call's return slot.
        unsafe { result.write(data) };
        // SAFETY: the waiting coroutine.
        unsafe { (host.co_resume)(co) };
        return Ok(());
    }
    let Some(index) = index else {
        let text = message!("task[{id}] is invalid");
        let outcome = event_warning(text.as_cstr());
        dtor(&mut data);
        return outcome;
    };
    // SAFETY: the result array.
    let array = unsafe { table(result) };
    // SAFETY: a live array; it takes the value.
    unsafe { zend_hash_index_update(array, index as u64, &raw mut data) };
    pending(server, |p| p.coroutines.remove(&id));
    // SAFETY: the result array.
    if unsafe { count(result) } == total {
        // SAFETY: the waiting coroutine.
        unsafe { (host.co_resume)(co) };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_data_matches_the_header() {
        // SWR_PHPSERVER_EVENT_DATA_SIZE and _ALIGN, which C++ asserts EventData fits.
        assert_eq!(std::mem::size_of::<EventData>(), 8192);
        assert_eq!(std::mem::align_of::<EventData>(), 8);
    }

    #[test]
    fn pending_tasks_are_per_server() {
        let (mut x, mut y) = (0u8, 0u8);
        let (a, b): (*mut c_void, *mut c_void) = ((&raw mut x).cast(), (&raw mut y).cast());
        pending(a, |p| p.callbacks.insert(7, a));
        assert_eq!(pending(b, |p| p.callbacks.get(&7).copied()), None);
        assert_eq!(pending(a, |p| p.callbacks.remove(&7)), Some(a));
    }
}
