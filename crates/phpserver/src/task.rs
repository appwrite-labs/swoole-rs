//! `Swoole\Server`'s task family: `task`, `taskwait`, `taskWaitMulti`,
//! `taskCo`, `Swoole\Server\Task::pack`/`unpack`, and the `onTask`/`onFinish`
//! events. Rust owns the callbacks of pending tasks and the coroutines that
//! wait for results (upstream's `ServerProperty::task_callbacks` and
//! `task_coroutine_map`), and the task encoding (a string, or PHP's serialize
//! with `SW_TASK_SERIALIZE`), packed, sent and waited for by crates/process's
//! `Server` methods over this worker's task pool, notify pipe and lock ([`Host`]).
use crate::dispatch::{handler_error, invoke, set_last_error, undef, zserv};
use crate::lifecycle::Props;
use crate::server::{DATA, TASK, fatal, running};
use std::{
    cell::RefCell,
    collections::HashMap,
    ffi::{CStr, c_int, c_void},
    mem::MaybeUninit,
    ptr,
    sync::atomic::Ordering,
};
use swoole_rs_corebase::{DataHead, log};
use swoole_rs_locks::{swr_v1_locks_lock, swr_v1_locks_unlock};
use swoole_rs_process::{
    methods::{self, swoole_get_worker_id},
    process_pool::{PoolRecord, SW_IPC_MAX_SIZE},
    task_file,
    task_sync::{Bailed, MultiTask, TaskHost, Tasks},
    workers::entry::finish,
};
use swoole_rs_server::view;
use swoole_rs_tls::socket_pair::{self, SocketPair};
use swoole_rs_zend::{
    Array, Call, E_WARNING, Value,
    bailout::Bailout,
    long_zval, message, report,
    string::bytes,
    sys::{
        IS_NULL, add_index_bool, swr_zend_serialize, swr_zend_string_release, swr_zend_unserialize,
        swr_zval_is_true, swr_zval_stringl, zend_hash_index_find, zend_hash_index_update, zval,
        zval_ptr_dtor,
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
/// `SW_ERROR_TASK_TIMEOUT`, `SW_LOG_WARNING`.
const SW_ERROR_TASK_TIMEOUT: c_int = 2003;
const SW_LOG_WARNING: c_int = 4;
const SW_OK: c_int = 0;
const SW_ERR: c_int = -1;
/// `DATA`'s `Swoole\Server\TaskResult`.
const TASK_RESULT: usize = 4;

/// What the task methods read of the Server.
struct Facts {
    worker: bool,
    task_worker: bool,
    task_object: bool,
    task_coroutine: bool,
    event_object: bool,
    enable_coroutine: bool,
    task_worker_num: i64,
}

/// A task's `get_task_id`, `get_task_src_worker_id`, `info.time` and `info.ext_flags`.
struct Info {
    id: i64,
    worker_id: i64,
    time: f64,
    flags: i64,
}

/// `Lock::MUTEX`.
const MUTEX: i32 = 3;

/// The server's task pool, and this worker's notify pipe and lock.
struct Host {
    pool: *mut PoolRecord,
    pipe: *mut SocketPair,
    lock: *mut c_void,
}

impl TaskHost for Host {
    fn dispatch(&mut self, task: *mut u8, worker_id: &mut c_int, blocking: bool) -> bool {
        // SAFETY: the live task pool and a packed task.
        unsafe { methods::dispatch(self.pool, task, worker_id, blocking) == SW_OK }
    }

    fn pipe_clean(&mut self) {
        // SAFETY: this worker's live notify pipe.
        socket_pair::clean(unsafe { &*self.pipe });
    }

    fn set_timeout(&mut self, timeout: f64) {
        // SAFETY: as above.
        socket_pair::set_timeout(unsafe { &mut *self.pipe }, timeout);
    }

    fn read_notify(&mut self) -> isize {
        let mut notify = 0u64;
        // SAFETY: as above, into a local of that size.
        unsafe { socket_pair::read(self.pipe, (&raw mut notify).cast(), size_of::<u64>()) }
    }

    fn lock(&mut self, on: bool) {
        if on {
            // SAFETY: this worker's live mutex.
            unsafe { swr_v1_locks_lock(self.lock, MUTEX, libc::LOCK_EX, -1, 0) };
        } else {
            // SAFETY: as above.
            unsafe { swr_v1_locks_unlock(self.lock, MUTEX) };
        }
    }
}

/// `serv`'s task state; for a call that waits (`sync`), with this worker's
/// notify pipe, lock and `task_results` slot.
fn task_state(serv: view::Server<'_>, sync: bool) -> Tasks<'_, Host> {
    let counters = serv.counters();
    let mut host = Host {
        pool: serv.task_pool(),
        pipe: ptr::null_mut(),
        lock: ptr::null_mut(),
    };
    let mut result = ptr::null_mut();
    if sync {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "upstream's uint16_t worker id"
        )]
        let id = swoole_get_worker_id() as u16;
        host.pipe = serv.task_notify_pipe(id);
        let worker = serv.worker(id);
        // SAFETY: this worker's live record.
        host.lock = unsafe { (*worker).lock };
        let results = serv.config().task_results.as_ptr().cast::<u8>();
        let size = usize::try_from(SW_IPC_MAX_SIZE).unwrap_or_default();
        result = results.wrapping_add(usize::from(id) * size);
    }
    Tasks {
        host,
        tasking: &counters.tasking_num,
        count: &counters.task_count,
        result,
    }
}

/// `taskWaitMulti`'s `Server::MultiTask`: the array of tasks, and the array of
/// results by their index.
struct Multi {
    tasks: *mut zval,
    out: *mut zval,
    count: u16,
}

impl MultiTask for Multi {
    fn count(&self) -> u16 {
        self.count
    }

    fn pack(&mut self, index: u16, event: *mut u8) -> Result<i64, Bailed> {
        // SAFETY: an array argument.
        let tasks = unsafe { table(self.tasks) };
        // SAFETY: a live array.
        let data = unsafe { zend_hash_index_find(tasks, u64::from(index)) };
        if data.is_null() {
            return Ok(-1);
        }
        encode(data, event.cast()).map_err(|Bailout| Bailed)
    }

    fn unpack(&mut self, index: u16, result: *mut u8) -> Result<(), Bailed> {
        let mut data = undef();
        if decode(result.cast(), &raw mut data).map_err(|Bailout| Bailed)? {
            // SAFETY: the result array.
            let out = unsafe { table(self.out) };
            // SAFETY: a live array; it takes the value.
            unsafe { zend_hash_index_update(out, u64::from(index), &raw mut data) };
        }
        Ok(())
    }

    fn fail(&mut self, index: u16) {
        // SAFETY: the result array.
        unsafe { add_index_bool(self.out, u64::from(index), false) };
    }
}

/// An `EventData`, which C++ asserts fits. Uninitialized, as upstream's stack
/// `EventData`: `task_pack` writes the head and the payload its length counts.
#[repr(C, align(8))]
pub(crate) struct EventData(MaybeUninit<[u8; 8192]>);

impl EventData {
    pub(crate) fn new() -> Self {
        Self(MaybeUninit::uninit())
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

fn pending<R>(server: view::Server<'_>, f: impl FnOnce(&mut Pending) -> R) -> R {
    pending_at(server.raw() as usize, f)
}

/// The pending table of the server at `key` (its address).
fn pending_at<R>(key: usize, f: impl FnOnce(&mut Pending) -> R) -> R {
    PENDING.with_borrow_mut(|all| f(all.entry(key).or_default()))
}

fn facts(server: view::Server<'_>) -> Facts {
    let config = server.config();
    let worker_type = crate::server::worker_type();
    Facts {
        worker: worker_type == crate::server::EVENT_WORKER,
        task_worker: worker_type == crate::server::TASK_WORKER,
        task_object: config.task_enable_coroutine || config.task_object,
        task_coroutine: config.task_enable_coroutine,
        event_object: config.event_object,
        enable_coroutine: config.enable_coroutine,
        task_worker_num: i64::from(config.task_worker_num),
    }
}

/// An `EventData`'s `DataHead`.
fn head<'a>(buf: *mut c_void) -> &'a mut DataHead {
    // SAFETY: every `EventData` starts with its `DataHead`.
    unsafe { &mut *buf.cast::<DataHead>() }
}

fn info(req: *mut c_void) -> Info {
    let head = head(req);
    Info {
        id: head.fd,
        worker_id: i64::from(head.reactor_id),
        time: head.time,
        flags: i64::from(head.ext_flags),
    }
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
    let packed = encoded(data, |payload, serialized| {
        // SAFETY: a writable `EventData` and the payload's bytes.
        if !unsafe { task_file::task_pack(buf.cast(), payload.as_ptr().cast(), payload.len()) } {
            let head = head(buf);
            head.fd = -1;
            head.len = 0;
        }
        (head(buf).fd, serialized)
    })?;
    let Some((id, serialized)) = packed else {
        return Ok(-1);
    };
    if id < 0 {
        event_warning(c"large task pack failed")?;
    }
    if serialized {
        add_flags(buf, SERIALIZE);
    }
    Ok(id)
}

/// `php_swoole_server_task_unpack`: the task in `buf` into `out`.
pub(crate) fn decode(buf: *mut c_void, out: *mut zval) -> Result<bool, Bailout> {
    let buffer = swoole_rs_corebase::thread_buffer();
    // SAFETY: a packed task and this thread's buffer.
    let Some((data, length, flags)) = (unsafe { task_file::unpack(buf, buffer) }) else {
        return Ok(false);
    };
    if u32::from(flags) & SERIALIZE == 0 {
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
        log::warning(
            c"php_swoole_server_task_unpack",
            format_args!("unserialize() failed, Error at offset {offset} of {length} bytes"),
        );
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
        unsafe { finish(server, payload.as_ptr().cast(), payload.len(), flags, req) }
    })?;
    Ok(finished.unwrap_or(false))
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
    add_flags(buf.ptr(), flags);
}

/// `buf->info.ext_flags |= flags`.
fn add_flags(buf: *mut c_void, flags: u32) {
    #[expect(clippy::cast_possible_truncation, reason = "ext_flags is a uint16_t")]
    let flags = flags as u16;
    head(buf).ext_flags |= flags;
}

/// `serv->task(buf, dst_worker_id)`.
fn dispatch(server: view::Server<'_>, buf: &mut EventData, dst: &mut c_int) -> bool {
    task_state(server, false)
        .task(buf.ptr().cast(), dst, false)
        .unwrap_or_else(|Bailed| {
            Bailout.defer();
            false
        })
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
fn worker<'a>(call: &'a Call, method: &CStr) -> Option<(view::Server<'a>, Facts)> {
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
        let cb = unsafe { swoole_rs_phpruntime::callable_create(callback) };
        if cb.is_null() {
            return call.set_bool(false);
        }
        if let Some(old) = pending(serv, |p| p.callbacks.insert(id, cb)) {
            // SAFETY: a callable this module created.
            unsafe { swoole_rs_phpruntime::callable_free(old) };
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
    if !swoole_rs_phpcoroutines::in_coroutine() {
        let mut tasks = task_state(serv, true);
        let done = match tasks.task_sync(buf.ptr().cast(), &mut dst, timeout) {
            // `serv->get_task_result()`: this worker's slot of `task_results`.
            Ok(true) => {
                decode(tasks.result.cast(), call.return_value()).unwrap_or_else(|bailout| {
                    bailout.defer();
                    false
                })
            }
            Ok(false) => false,
            Err(Bailed) => {
                Bailout.defer();
                false
            }
        };
        if !done {
            call.set_bool(false);
        }
        return;
    }
    flag(&mut buf, NONBLOCK | COROUTINE);
    let co = swoole_rs_phpcoroutines::current();
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
    let done = unsafe { swoole_rs_phpcoroutines::suspend_for(task_co.co, timeout) };
    pending(serv, |p| p.coroutines.remove(&id));
    if !done {
        call.set_bool(false);
    }
}

fn task_wait_multi(call: &Call) {
    if swoole_rs_phpcoroutines::in_coroutine() {
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
    let n_task = length(call.value(1));
    if n_task >= MAX_CONCURRENT_TASK {
        fatal(E_WARNING, c"too many concurrent tasks");
        return call.set_bool(false);
    }
    let mut multi = Multi {
        tasks,
        out: call.return_value(),
        count: u16::try_from(n_task).unwrap_or_default(),
    };
    let done = task_state(serv, true)
        .task_sync_many(&mut multi, timeout)
        .unwrap_or_else(|Bailed| {
            Bailout.defer();
            false
        });
    if !done {
        // SAFETY: the array this call made.
        unsafe { zval_ptr_dtor(call.return_value()) };
        call.set_bool(false);
    }
}

/// `php_swoole_array_length`, as 0 for what is not an array.
fn length(array: Option<&Value>) -> u32 {
    array.and_then(Value::array).map_or(0, Array::count)
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
    let tasks = call.value(1).and_then(Value::array);
    let mut n_task = tasks.map_or(0, Array::count);
    if n_task >= MAX_CONCURRENT_TASK {
        fatal(E_WARNING, c"too many concurrent tasks");
        return call.set_bool(false);
    }
    if !check(&facts, -1) {
        return call.set_bool(false);
    }
    if let Err(bailout) = swoole_rs_phpruntime::coroutine_check() {
        return bailout.defer();
    }
    let co = swoole_rs_phpcoroutines::current();
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
    for (_, ztask) in tasks.into_iter().flat_map(Array::entries) {
        let index = list.len() as u64;
        let mut buf = EventData::new();
        let mut id = pack(ztask.as_ptr(), &mut buf);
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
    if unsafe { swoole_rs_phpcoroutines::suspend_for(co, timeout) } {
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
            if let Some(&id) = task_co.list.get(j) {
                pending(serv, |p| p.coroutines.remove(&id));
            }
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
    // `EventData::size()`.
    let size = size_of::<DataHead>() + head(buf.ptr()).len as usize;
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
    report(E_WARNING, text)
}

/// Unpack `req` into a value, or `None` (`Err` for a trapped bailout).
fn unpack(req: *mut c_void) -> Result<Option<zval>, Bailout> {
    let mut data = undef();
    Ok(decode(req, &raw mut data)?.then_some(data))
}

/// `php_swoole_server_onTask`, for a live server and the task it received.
pub(crate) fn on_task(server: view::Server<'_>, req: *mut c_void) -> Result<c_int, Bailout> {
    let counters = server.counters();
    counters.tasking_num.fetch_sub(1, Ordering::SeqCst);
    let zserv = zserv();
    let Some(mut data) = unpack(req)? else {
        return Ok(SW_ERR);
    };
    let facts = facts(server);
    let info = info(req);
    let mut args = [zserv, undef(), undef(), undef()];
    let argc = if facts.task_object {
        let props = Props::init(&mut args[1], &TASK);
        crate::server::task_bind(props.object(), server.raw(), head(req));
        props.long(c"worker_id", info.worker_id);
        props.long(c"id", info.id);
        props.value(c"data", &mut data);
        props.double(c"dispatch_time", info.time);
        props.long(c"flags", info.flags);
        2
    } else {
        args[1] = long_zval(info.id);
        args[2] = long_zval(info.worker_id);
        args[3] = data;
        4
    };
    let mut retval = undef();
    let cb = crate::lifecycle::callback(ON_TASK);
    let outcome = match invoke(
        cb,
        args.get_mut(..argc).unwrap_or_default(),
        &raw mut retval,
        facts.task_coroutine,
    ) {
        Ok(false) => handler_error(&zserv, "onTask"),
        called => called.map(drop),
    };
    if argc == 2 {
        dtor(&mut args[1]);
    }
    let outcome = outcome.and_then(|()| {
        // SAFETY: the callback's result, or undef.
        if unsafe { type_of(&raw const retval) } != IS_NULL {
            let finished = finish_task(server.raw(), &raw mut retval, req);
            dtor(&mut retval);
            finished?;
        }
        Ok(())
    });
    dtor(&mut data);
    outcome.map(|()| SW_OK)
}

/// `php_swoole_server_onFinish`, for a live server and the result it received.
pub(crate) fn on_finish(server: view::Server<'_>, req: *mut c_void) -> Result<c_int, Bailout> {
    let zserv = zserv();
    let info = info(req);
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
        crate::lifecycle::callback(ON_FINISH)
    };
    if cb.is_null() {
        let outcome = event_warning(c"require 'onFinish' callback");
        dtor(&mut data);
        return outcome.map(|()| SW_ERR);
    }
    let facts = facts(server);
    let mut args = [zserv, undef(), undef()];
    let argc = if facts.event_object {
        let props = Props::init(&mut args[1], &DATA[TASK_RESULT]);
        props.long(c"task_id", id);
        props.long(c"task_worker_id", info.worker_id);
        props.double(c"dispatch_time", info.time);
        props.value(c"data", &mut data);
        2
    } else {
        args[1] = long_zval(id);
        args[2] = data;
        3
    };
    let outcome = match invoke(
        cb,
        args.get_mut(..argc).unwrap_or_default(),
        ptr::null_mut(),
        facts.enable_coroutine,
    ) {
        Ok(false) => handler_error(&zserv, "onFinish"),
        called => called.map(drop),
    };
    if flags & CALLBACK != 0
        && let Some(cb) = pending(server, |p| p.callbacks.remove(&id))
    {
        // SAFETY: a callable `task()` created.
        unsafe { swoole_rs_phpruntime::callable_free(cb) };
    }
    if facts.event_object {
        dtor(&mut args[1]);
    }
    dtor(&mut data);
    outcome.map(|()| SW_OK)
}

/// A coroutine task's result: into `taskwait`'s return value, or at its index
/// in `taskCo`'s array; the waiting coroutine resumes once all arrived.
fn resume(server: view::Server<'_>, id: i64, mut data: zval) -> Result<(), Bailout> {
    let Some(task_co) = pending(server, |p| p.coroutines.get(&id).copied()) else {
        log::error_log(
            SW_LOG_WARNING,
            SW_ERROR_TASK_TIMEOUT,
            c"php_swoole_server_onFinish",
            format_args!("task[{id}] has expired"),
        );
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
        // SAFETY: the waiting coroutine; this frame owns nothing that needs a drop.
        unsafe { swoole_rs_phpcoroutines::wake(co, false) };
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
        // SAFETY: the waiting coroutine; this frame owns nothing that needs a drop.
        unsafe { swoole_rs_phpcoroutines::wake(co, false) };
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
        pending_at(a as usize, |p| p.callbacks.insert(7, a));
        assert_eq!(
            pending_at(b as usize, |p| p.callbacks.get(&7).copied()),
            None
        );
        assert_eq!(pending_at(a as usize, |p| p.callbacks.remove(&7)), Some(a));
    }
}
