//! `Swoole\Server`'s task family: `task`, `taskwait`, `taskWaitMulti`,
//! `taskCo`, `Swoole\Server\Task::pack`/`unpack`, and the `onTask`/`onFinish`
//! events. Rust owns the callbacks of pending tasks and the coroutines that
//! wait for results (upstream's `ServerProperty::task_callbacks` and
//! `task_coroutine_map`), and the task encoding (a string, or PHP's serialize
//! with `SW_TASK_SERIALIZE`), packed, sent and waited for by crates/process's
//! `Server` methods over this worker's task pool, notify pipe and lock ([`Host`]).
use crate::callable::Callable;
use crate::dispatch::{dtor, handler_error, invoke, set_last_error, undef, zserv};
use crate::lifecycle::Props;
use crate::server::{DATA, TASK, fatal, running};
use std::{
    cell::RefCell,
    collections::HashMap,
    ffi::{CStr, c_int, c_void},
    mem::{ManuallyDrop, MaybeUninit},
    ptr,
    sync::atomic::Ordering,
};
use swoole_rs_buffer::string::CppString;
use swoole_rs_corebase::{DataHead, log};
use swoole_rs_locks::SharedMutex;
use swoole_rs_phpcoroutines::Coroutine;
use swoole_rs_process::{
    methods::{self, swoole_get_worker_id},
    process_pool::{PoolRef, SW_IPC_MAX_SIZE},
    task_file::{self, Packed},
    task_sync::{Bailed, MultiTask, TaskHost, Tasks},
};
use swoole_rs_server::{task_worker_ops::finish, view};
use swoole_rs_tls::socket_pair::{self, SocketPair};
use swoole_rs_zend::{
    Array, ArrayMut, Call, E_WARNING, Owned, OwnedArray, Value,
    bailout::Bailout,
    long_zval, message, report,
    sys::{swr_zend_unserialize, swr_zval_stringl, zval, zval_ptr_dtor},
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

/// The server's task pool, and this worker's notify pipe and lock.
struct Host<'a> {
    pool: PoolRef<'a>,
    pipe: Option<&'a mut SocketPair>,
    lock: Option<&'static SharedMutex>,
}

impl TaskHost for Host<'_> {
    fn dispatch(&mut self, task: *mut u8, worker_id: &mut c_int, blocking: bool) -> bool {
        // SAFETY: a packed task, which nothing writes while it is sent.
        let task = unsafe { Packed::from_raw(task.cast()) };
        methods::dispatch(self.pool, task, worker_id, blocking) == SW_OK
    }

    fn pipe_clean(&mut self) {
        if let Some(pipe) = &self.pipe {
            socket_pair::clean(pipe);
        }
    }

    fn set_timeout(&mut self, timeout: f64) {
        if let Some(pipe) = &mut self.pipe {
            socket_pair::set_timeout(pipe, timeout);
        }
    }

    fn read_notify(&mut self) -> isize {
        let Some(pipe) = &self.pipe else { return -1 };
        let mut notify = 0u64;
        // SAFETY: a local of that size.
        unsafe { socket_pair::read(pipe, (&raw mut notify).cast(), size_of::<u64>()) }
    }

    fn lock(&mut self, on: bool) {
        if let Some(lock) = self.lock {
            _ = if on { lock.lock() } else { lock.unlock() };
        }
    }
}

/// `serv`'s task state; for a call that waits (`sync`), with this worker's
/// notify pipe, lock and `task_results` slot.
fn task_state(serv: view::Server<'_>, sync: bool) -> Tasks<'_, Host<'_>> {
    let counters = serv.counters();
    let mut host = Host {
        pool: serv.task_pool_ref(),
        pipe: None,
        lock: None,
    };
    let mut result = ptr::null_mut();
    if sync {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "upstream's uint16_t worker id"
        )]
        let id = swoole_get_worker_id() as u16;
        // SAFETY: this worker's live notify pipe, which only this call uses meanwhile.
        host.pipe = unsafe { serv.task_notify_pipe(id).as_mut() };
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
struct Multi<'a> {
    tasks: Option<&'a Array>,
    out: &'a mut ArrayMut,
    count: u16,
}

impl MultiTask for Multi<'_> {
    fn count(&self) -> u16 {
        self.count
    }

    fn pack(&mut self, index: u16, event: *mut u8) -> Result<i64, Bailed> {
        let Some(data) = self.tasks.and_then(|tasks| tasks.index(u64::from(index))) else {
            return Ok(-1);
        };
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "task_sync's event is a zeroed Vec<u64> of SW_IPC_MAX_SIZE bytes"
        )]
        let event = event.cast::<EventData>();
        // SAFETY: as above, an initialized, aligned `EventData` it lends for the call.
        let event = unsafe { &mut *event };
        encode(data, event).map_err(|Bailout| Bailed)
    }

    fn unpack(&mut self, index: u16, result: Packed<'_>) -> Result<(), Bailed> {
        let data = Owned::default();
        if decode(result, data.as_ptr()).map_err(|Bailout| Bailed)? {
            self.out.update_index(u64::from(index), data);
        }
        Ok(())
    }

    fn fail(&mut self, index: u16) {
        self.out.update_index(u64::from(index), Owned::bool(false));
    }
}

/// A `task_file::EventData` to pack into. Its payload is uninitialized, as
/// upstream's stack `EventData`: `task_pack` writes the head and the payload its
/// length counts.
#[repr(C)]
pub(crate) struct EventData {
    head: DataHead,
    _payload: MaybeUninit<[u8; size_of::<task_file::EventData>() - size_of::<DataHead>()]>,
}

impl EventData {
    pub(crate) fn new() -> Self {
        Self {
            head: DataHead::default(),
            _payload: MaybeUninit::uninit(),
        }
    }

    pub(crate) fn ptr(&mut self) -> *mut c_void {
        ptr::from_mut(self).cast()
    }
}

/// Upstream's `TaskCo`: a coroutine waiting in `taskwait` (`list` empty) or
/// `taskCo`/`taskWaitMulti` for results written into `result`.
struct TaskCo {
    co: Coroutine,
    result: *mut zval,
    list: Vec<i64>,
    count: u32,
    /// `taskCo` fills timed-out results with false; `taskWaitMulti` leaves them unset.
    fill_false: bool,
}

/// One server's pending tasks.
#[derive(Default)]
struct Pending {
    /// Never freed with the table: a thread exits after Zend shut down.
    callbacks: HashMap<i64, ManuallyDrop<Callable>>,
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

fn info(task: Packed<'_>) -> Info {
    let head = task.head();
    Info {
        id: head.fd,
        worker_id: i64::from(head.reactor_id),
        time: head.time,
        flags: i64::from(head.ext_flags),
    }
}

/// The bytes a task carries: a string as it is, anything else serialized
/// (`None`: serialize wrote nothing). `each` sees the bytes and whether they
/// were serialized.
fn encoded<R>(data: &Value, each: impl FnOnce(&[u8], bool) -> R) -> Result<Option<R>, Bailout> {
    if let Some(text) = data.bytes() {
        return Ok(Some(each(text, false)));
    }
    Ok(data
        .serialize()?
        .map(|serialized| each(serialized.bytes(), true)))
}

/// `php_swoole_server_task_pack`: pack `data` into `buf`; the task id, or -1.
fn encode(data: &Value, buf: &mut EventData) -> Result<i64, Bailout> {
    let packed = encoded(data, |payload, serialized| {
        let task = buf.ptr().cast();
        // SAFETY: a writable `EventData` and the payload's bytes.
        if !unsafe { task_file::task_pack(task, payload.as_ptr().cast(), payload.len()) } {
            buf.head.fd = -1;
            buf.head.len = 0;
        }
        (buf.head.fd, serialized)
    })?;
    let Some((id, serialized)) = packed else {
        return Ok(-1);
    };
    if id < 0 {
        event_warning(c"large task pack failed")?;
    }
    if serialized {
        flag(buf, SERIALIZE);
    }
    Ok(id)
}

/// `php_swoole_server_task_unpack`: `task` into `out`.
pub(crate) fn decode(task: Packed<'_>, out: *mut zval) -> Result<bool, Bailout> {
    let buffer = swoole_rs_corebase::thread_buffer().cast::<CppString>();
    // SAFETY: this thread's live buffer.
    let Some((data, length, flags)) = task.unpack(unsafe { &mut *buffer }) else {
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

/// `php_swoole_server_task_finish`: send `data` as the result of `req` (none:
/// this worker's current task).
pub(crate) fn finish_task(
    server: view::Server<'_>,
    data: &Value,
    req: Option<&DataHead>,
) -> Result<bool, Bailout> {
    let finished = encoded(data, |payload, serialized| {
        let flags = if serialized {
            SERIALIZE.cast_signed()
        } else {
            0
        };
        finish(server, payload, flags, req)
    })?;
    Ok(finished.unwrap_or(false))
}

/// `php_swoole_server_task_pack`: the task id, or -1; a trapped bailout waits
/// for the handler's end.
pub(crate) fn pack(data: &Value, buf: &mut EventData) -> i64 {
    encode(data, buf).unwrap_or_else(|bailout| {
        bailout.defer();
        -1
    })
}

/// `buf->info.ext_flags |= flags`.
fn flag(buf: &mut EventData, flags: u32) {
    #[expect(clippy::cast_possible_truncation, reason = "ext_flags is a uint16_t")]
    let flags = flags as u16;
    buf.head.ext_flags |= flags;
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
    let facts = facts(serv);
    let Some(data) = call.value(1).filter(|_| check(&facts, dst)) else {
        return call.set_bool(false);
    };
    let mut buf = EventData::new();
    let id = pack(data, &mut buf);
    if id < 0 {
        return call.set_bool(false);
    }
    let callback = call.value(3).filter(|zfn| zfn.truthy());
    if !facts.worker {
        flag(&mut buf, NOREPLY);
    } else if let Some(callback) = callback {
        flag(&mut buf, CALLBACK);
        let Some(cb) = Callable::create(callback) else {
            return call.set_bool(false);
        };
        // A replaced callable is freed once the table is no longer borrowed.
        let old = pending(serv, |p| p.callbacks.insert(id, ManuallyDrop::new(cb)));
        drop(old.map(ManuallyDrop::into_inner));
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
    let Some(data) = call.value(1).filter(|_| check(&facts, dst)) else {
        return call.set_bool(false);
    };
    let mut buf = EventData::new();
    let id = pack(data, &mut buf);
    if id < 0 {
        return call.set_bool(false);
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream passes (int *) &dst_worker_id"
    )]
    let mut dst = dst as c_int;
    let Some(co) = Coroutine::current() else {
        let mut tasks = task_state(serv, true);
        let done = match tasks.task_sync(buf.ptr().cast(), &mut dst, timeout) {
            // `serv->get_task_result()`: this worker's slot of `task_results`.
            Ok(true) => {
                // SAFETY: the result the task worker packed into the slot.
                let result = unsafe { Packed::from_raw(tasks.result.cast()) };
                decode(result, call.return_value()).unwrap_or_else(|bailout| {
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
    };
    flag(&mut buf, NONBLOCK | COROUTINE);
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
    // onFinish resumes it.
    let done = swoole_rs_phpcoroutines::suspend_for(timeout);
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
    let tasks = call.value(1).and_then(Value::array);
    let n_task = tasks.map_or(0, Array::count);
    if n_task >= MAX_CONCURRENT_TASK {
        // As upstream, whose `array_init(return_value)` precedes this return.
        call.set_array();
        fatal(E_WARNING, c"too many concurrent tasks");
        return call.set_bool(false);
    }
    // The results, which become the return value only once every wait is over.
    let mut out = OwnedArray::default();
    let mut multi = Multi {
        tasks,
        out: &mut out,
        count: u16::try_from(n_task).unwrap_or_default(),
    };
    let done = task_state(serv, true)
        .task_sync_many(&mut multi, timeout)
        .unwrap_or_else(|Bailed| {
            Bailout.defer();
            false
        });
    if done {
        call.set_owned(out.into_owned());
    } else {
        // `zval_ptr_dtor(return_value)`, then `RETURN_FALSE`.
        drop(out);
        call.set_bool(false);
    }
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
    let Some(co) = Coroutine::current() else {
        return;
    };
    let result = call.return_value();
    call.set_array();
    // SAFETY: the array this call made; `onFinish` writes it only while this
    // coroutine waits, after this borrow's last use.
    let array = unsafe { ArrayMut::from_ptr(result) };
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
            array.update_index(index, Owned::bool(false));
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
    // onFinish resumes it.
    if swoole_rs_phpcoroutines::suspend_for(timeout) {
        return;
    }
    // SAFETY: the array this call made, which `onFinish` no longer writes.
    let array = unsafe { ArrayMut::from_ptr(result) };
    for j in 0..n_task as usize {
        if array.array().and_then(|a| a.index(j as u64)).is_none() {
            if task_co.fill_false {
                array.update_index(j as u64, Owned::bool(false));
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
    let Some(data) = call.value(1) else {
        return call.set_bool(false);
    };
    let mut buf = EventData::new();
    if pack(data, &mut buf) < 0 {
        return call.set_bool(false);
    }
    flag(&mut buf, NONBLOCK | NOREPLY);
    // `EventData::size()`.
    let size = size_of::<DataHead>() + buf.head.len as usize;
    // RETURN_STRINGL: `size` bytes of the buffer, copied.
    // SAFETY: the return slot and the buffer's bytes.
    unsafe {
        swoole_rs_zend::sys::swr_zval_stringl(call.return_value(), buf.ptr().cast(), size);
    };
}

fn unpack_method(call: &Call) {
    if call.arity(1, 1).is_err() {
        return call.set_bool(false);
    }
    // A packed task: a string of a head and the payload it counts.
    let task = call
        .value(1)
        .and_then(Value::bytes)
        .and_then(Packed::from_bytes);
    let done = task.is_some_and(|task| {
        decode(task, call.return_value()).unwrap_or_else(|bailout| {
            bailout.defer();
            false
        })
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
fn unpack(task: Packed<'_>) -> Result<Option<Owned>, Bailout> {
    let data = Owned::default();
    Ok(decode(task, data.as_ptr())?.then_some(data))
}

/// `php_swoole_server_onTask`, for a live server and the task it received.
pub(crate) fn on_task(server: view::Server<'_>, task: Packed<'_>) -> Result<c_int, Bailout> {
    let counters = server.counters();
    counters.tasking_num.fetch_sub(1, Ordering::SeqCst);
    let zserv = zserv();
    let Some(data) = unpack(task)? else {
        return Ok(SW_ERR);
    };
    let mut data = data.into_zval();
    let facts = facts(server);
    let info = info(task);
    let mut args = [zserv, undef(), undef(), undef()];
    let argc = if facts.task_object {
        let props = Props::init(&mut args[1], &TASK);
        props.long(c"worker_id", info.worker_id);
        props.long(c"id", info.id);
        props.value(c"data", &mut data);
        props.double(c"dispatch_time", info.time);
        props.long(c"flags", info.flags);
        crate::server::task_bind(Value::of(&mut args[1]), server.raw(), &task.head());
        2
    } else {
        args[1] = long_zval(info.id);
        args[2] = long_zval(info.worker_id);
        args[3] = data;
        4
    };
    let retval = Owned::default();
    let cb = crate::lifecycle::callback(ON_TASK);
    let outcome = match invoke(
        cb,
        args.get_mut(..argc).unwrap_or_default(),
        retval.as_ptr(),
        facts.task_coroutine,
    ) {
        Ok(false) => handler_error("onTask"),
        called => called.map(drop),
    };
    if argc == 2 {
        dtor(&mut args[1]);
    }
    let outcome = outcome.and_then(|()| {
        if !retval.is_null() {
            finish_task(server, &retval, Some(&task.head()))?;
        }
        Ok(())
    });
    drop(retval);
    dtor(&mut data);
    outcome.map(|()| SW_OK)
}

/// `php_swoole_server_onFinish`, for a live server and the result it received.
pub(crate) fn on_finish(server: view::Server<'_>, task: Packed<'_>) -> Result<c_int, Bailout> {
    let zserv = zserv();
    let info = info(task);
    let id = info.id;
    let Some(data) = unpack(task)? else {
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
    let mut data = data.into_zval();
    let cb = if flags & CALLBACK != 0 {
        let cb = pending(server, |p| p.callbacks.get(&id).map(|cb| cb.as_raw()));
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
        Ok(false) => handler_error("onFinish"),
        called => called.map(drop),
    };
    if flags & CALLBACK != 0 {
        // Freed once the table is no longer borrowed.
        drop(pending(server, |p| p.callbacks.remove(&id)).map(ManuallyDrop::into_inner));
    }
    if facts.event_object {
        dtor(&mut args[1]);
    }
    dtor(&mut data);
    outcome.map(|()| SW_OK)
}

/// A coroutine task's result: into `taskwait`'s return value, or at its index
/// in `taskCo`'s array; the waiting coroutine resumes once all arrived.
fn resume(server: view::Server<'_>, id: i64, data: Owned) -> Result<(), Bailout> {
    let Some(task_co) = pending(server, |p| p.coroutines.get(&id).copied()) else {
        log::error_log(
            SW_LOG_WARNING,
            SW_ERROR_TASK_TIMEOUT,
            c"php_swoole_server_onFinish",
            format_args!("task[{id}] has expired"),
        );
        return Ok(());
    };
    // SAFETY: the waiting coroutine's live TaskCo, unregistered before it ends.
    let task_co = unsafe { &*task_co };
    let (co, result, index, total, single) = (
        task_co.co,
        task_co.result,
        task_co.list.iter().position(|&t| t == id),
        task_co.count,
        task_co.list.is_empty(),
    );
    if single {
        // ZVAL_COPY_VALUE: the result takes the value.
        // SAFETY: the waiting call's return slot.
        unsafe { result.write(data.into_zval()) };
        // SAFETY: the waiting coroutine; this frame owns nothing that needs a drop.
        unsafe { co.wake(false) };
        return Ok(());
    }
    let Some(index) = index else {
        let text = message!("task[{id}] is invalid");
        return event_warning(text.as_cstr());
    };
    // SAFETY: the waiting call's result array, which it leaves alone while it waits.
    let array = unsafe { ArrayMut::from_ptr(result) };
    array.update_index(index as u64, data);
    pending(server, |p| p.coroutines.remove(&id));
    if array.array().map_or(0, Array::count) == total {
        // SAFETY: the waiting coroutine; this frame owns nothing that needs a drop.
        unsafe { co.wake(false) };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_data_matches_the_header() {
        // Upstream's `sizeof(EventData)`: SW_IPC_MAX_SIZE (8192, 2048 on macOS).
        assert_eq!(std::mem::size_of::<EventData>() as u64, SW_IPC_MAX_SIZE);
        assert_eq!(std::mem::align_of::<EventData>(), 8);
    }

    #[test]
    fn pending_tasks_are_per_server() {
        let (mut x, mut y) = (0u64, 0u64);
        let (a, b) = ((&raw mut x).cast::<TaskCo>(), (&raw mut y).cast::<TaskCo>());
        pending_at(a as usize, |p| p.coroutines.insert(7, a));
        assert_eq!(
            pending_at(b as usize, |p| p.coroutines.get(&7).copied()),
            None
        );
        assert_eq!(pending_at(a as usize, |p| p.coroutines.remove(&7)), Some(a));
    }
}
