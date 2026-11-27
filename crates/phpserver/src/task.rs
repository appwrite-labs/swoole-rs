//! `Swoole\Server`'s task family: `task`, `taskwait`, `taskWaitMulti`,
//! `taskCo`, `Swoole\Server\Task::pack`/`unpack`, and the `onTask`/`onFinish`
//! events. Rust owns the callbacks of pending tasks and the coroutines that
//! wait for results (upstream's `ServerProperty::task_callbacks` and
//! `task_coroutine_map`), and the task encoding (a string, or PHP's serialize
//! with `SW_TASK_SERIALIZE`), packed, sent and waited for by `corebase::process`'s
//! `Server` methods over this worker's task pool, notify pipe and lock ([`Host`]).
use crate::callable::Callable;
use crate::dispatch::{handler_error, invoke, set_last_error, used};
use crate::lifecycle::Props;
use crate::server::{DATA, TASK, fatal, running};
use std::{
    cell::Cell,
    collections::HashMap,
    ffi::{CStr, c_int},
    mem::ManuallyDrop,
    rc::{Rc, Weak},
    sync::atomic::Ordering,
};
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::CellExt;
use swoole_rs_corebase::buffer::string::StringBuffer;
use swoole_rs_corebase::constants::TaskFlag;
use swoole_rs_corebase::constants::{SW_MAX_CONCURRENT_TASK, SW_TASKWAIT_TIMEOUT};
use swoole_rs_corebase::locks::SharedMutex;
use swoole_rs_corebase::log::LogLevel::SW_LOG_WARNING;
use swoole_rs_corebase::process::{
    methods::{self, swoole_get_worker_id},
    process_pool::PoolRef,
    task_file::{Packed, TaskBuffer},
    task_sync::{Bailed, MultiTask, TaskHost, Tasks, Waiting},
};
use swoole_rs_corebase::timer::Deadline;
use swoole_rs_corebase::tls::socket_pair::{self, SocketPair};
use swoole_rs_corebase::{DataHead, log};
use swoole_rs_phpcoroutines::{Coroutine, Waiter};
use swoole_rs_phpswoole::unserialize_from;
use swoole_rs_server::{
    task_worker::{native::finish, result_slot},
    view,
};
use swoole_rs_zend::{
    Array, ArrayMut, Call, E_WARNING, Owned, OwnedArray, Value, ZString, bailout::Bailout, message,
    report,
};

/// `SW_TASK_*` flags.
const SERIALIZE: TaskFlag = TaskFlag::SW_TASK_SERIALIZE;
const NONBLOCK: TaskFlag = TaskFlag::SW_TASK_NONBLOCK;
const CALLBACK: TaskFlag = TaskFlag::SW_TASK_CALLBACK;
const COROUTINE: TaskFlag = TaskFlag::SW_TASK_COROUTINE;
const NOREPLY: TaskFlag = TaskFlag::SW_TASK_NOREPLY;
/// `SW_ERROR_PHP_FATAL_ERROR`, `SW_ERROR_TASK_DISPATCH_FAIL`.
const SW_ERROR_PHP_FATAL_ERROR: c_int =
    swoole_rs_error::ErrorCode::SW_ERROR_PHP_FATAL_ERROR as c_int;
const SW_ERROR_TASK_DISPATCH_FAIL: c_int =
    swoole_rs_error::ErrorCode::SW_ERROR_TASK_DISPATCH_FAIL as c_int;
/// `SW_ERROR_TASK_TIMEOUT`, `SW_LOG_WARNING`.
const SW_ERROR_TASK_TIMEOUT: c_int = swoole_rs_error::ErrorCode::SW_ERROR_TASK_TIMEOUT as c_int;
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
    pipe: Option<&'a SocketPair>,
    lock: Option<&'static SharedMutex>,
}

impl TaskHost for Host<'_> {
    fn dispatch(&mut self, task: Packed<'_>, worker: Option<u32>, blocking: bool) -> bool {
        methods::dispatch(
            swoole_rs_server::process_pool_native(),
            self.pool,
            task,
            worker,
            blocking,
        )
    }

    fn pipe_clean(&mut self) {
        if let Some(pipe) = self.pipe {
            socket_pair::clean(pipe);
        }
    }

    fn set_timeout(&mut self, timeout: f64) {
        if let Some(pipe) = self.pipe {
            socket_pair::set_timeout(pipe, timeout);
        }
    }

    fn read_notify(&mut self) -> Option<usize> {
        let pipe = self.pipe?;
        let mut notify = [std::mem::MaybeUninit::uninit(); size_of::<u64>()];
        socket_pair::read_into(pipe, &mut notify)
    }

    fn lock(&mut self, on: bool) {
        if let Some(lock) = self.lock {
            _ = if on { lock.lock() } else { lock.unlock() };
        }
    }
}

/// `serv`'s task state.
fn task_state(serv: view::Server<'_>) -> Tasks<'_, Host<'_>> {
    let counters = serv.counters();
    Tasks {
        host: Host {
            pool: serv.task_pool_ref(),
            pipe: None,
            lock: None,
        },
        tasking: &counters.tasking_num,
        count: &counters.task_count,
    }
}

/// `serv`'s task state for a call that waits, with this worker's notify pipe, lock
/// and `task_results` slot; `None` before the server has its results.
fn wait_state(serv: view::Server<'_>) -> Option<Waiting<'_, Host<'_>>> {
    let mut tasks = task_state(serv);
    let id = swoole_get_worker_id().c_cast::<u16>();
    tasks.host.pipe = serv.task_notify_pipe(id);
    tasks.host.lock = serv
        .worker_record(id)
        .and_then(swoole_rs_corebase::process::worker_record::Worker::lock);
    let slot = result_slot(serv, id)?;
    Some(tasks.waiting(slot))
}

/// `taskWaitMulti`'s `Server::MultiTask`: the array of tasks, and the array of
/// results by their index.
struct Multi<'a> {
    tasks: Vec<&'a Value>,
    out: &'a mut ArrayMut,
    count: u16,
}

impl MultiTask for Multi<'_> {
    fn count(&self) -> u16 {
        self.count
    }

    fn pack(&mut self, index: u16, event: &mut TaskBuffer) -> Result<Option<i64>, Bailed> {
        let Some(data) = self.tasks.get(usize::from(index)) else {
            return Ok(None);
        };
        encode(data, event).map_err(|Bailout| Bailed)
    }

    fn unpack(&mut self, index: u16, result: Packed<'_>) -> Result<(), Bailed> {
        if let Some((data, true)) = decode(result).map_err(|Bailout| Bailed)? {
            self.out.update_index(u64::from(index), data);
        }
        Ok(())
    }

    fn fail(&mut self, index: u16) {
        self.out.update_index(u64::from(index), Owned::bool(false));
    }
}

/// Upstream's `TaskCo`: a coroutine waiting in `taskwait` or
/// `taskCo`/`taskWaitMulti` for results written into `result`, which becomes
/// the call's return value. The waiting call owns it; the pending table refers to it.
struct TaskCo {
    /// The waiting call's wait.
    waiter: Waiter,
    /// Taken out and put back with nothing in between that yields or runs PHP
    /// code.
    result: Cell<Owned>,
    /// Number of positional results, including failed dispatches.
    count: u32,
}

/// One server's pending tasks, held on its object beside its `Property`, as
/// upstream's `ServerProperty` holds them.
#[derive(Default)]
pub(crate) struct Pending {
    /// Never freed with the table: a thread exits after Zend shut down.
    callbacks: HashMap<i64, ManuallyDrop<Callable>>,
    /// The waiting call owns each.
    coroutines: HashMap<i64, (Weak<TaskCo>, Option<u64>)>,
}

fn facts(server: view::Server<'_>) -> Facts {
    let config = server.config();
    let worker_type = crate::server::worker_type();
    Facts {
        worker: worker_type == Some(crate::server::WorkerType::Event),
        task_worker: worker_type == Some(crate::server::WorkerType::Task),
        task_object: config.task_enable_coroutine.get() || config.task_object.get(),
        task_coroutine: config.task_enable_coroutine.get(),
        event_object: config.event_object.get(),
        enable_coroutine: config.enable_coroutine.get(),
        task_worker_num: i64::from(config.task_worker_num.get()),
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
fn encode(data: &Value, buf: &mut TaskBuffer) -> Result<Option<i64>, Bailout> {
    let packed = encoded(data, |payload, serialized| {
        let id = buf.pack(payload).then(|| buf.head().fd);
        (id, serialized)
    })?;
    let Some((id, serialized)) = packed else {
        return Ok(None);
    };
    if id.is_none() {
        event_warning(c"large task pack failed")?;
    }
    if serialized {
        flag(buf, SERIALIZE);
    }
    Ok(id)
}

/// `php_swoole_server_task_unpack`: `task`'s value, and whether it unserialized
/// whole (the part it did is still returned, as upstream leaves it in its zval);
/// `None` when its temporary file can't be read. A value a bailout interrupted
/// is leaked.
pub(crate) fn decode(task: Packed<'_>) -> Result<Option<(Owned, bool)>, Bailout> {
    // Upstream reads a temporary file into `sw_tg_buffer()`; this reads it into its
    // own buffer, which unserialize's PHP code can't reach.
    let mut buffer = StringBuffer::EMPTY;
    let decoded = decode_into(task, &mut buffer);
    buffer.dispose();
    decoded
}

/// [`decode`] with `buffer` for a temporary file's bytes.
fn decode_into(
    task: Packed<'_>,
    buffer: &mut StringBuffer,
) -> Result<Option<(Owned, bool)>, Bailout> {
    let Some((data, flags)) = task.unpack(buffer) else {
        return Ok(None);
    };
    if !TaskFlag::from_bits(flags).contains(SERIALIZE) {
        return Ok(Some((Owned::string(ZString::new(data)), true)));
    }
    let (value, done, bailed) = unserialize_from(c"php_swoole_server_task_unpack", data);
    if let Err(bailout) = bailed {
        #[expect(
            clippy::mem_forget,
            reason = "a value a bailout interrupted is leaked, as upstream's"
        )]
        std::mem::forget(value);
        return Err(bailout);
    }
    Ok(Some((value, done)))
}

/// A handler's `decode` into its return value: whether it unserialized whole.
fn return_decoded(call: &Call, decoded: Result<Option<(Owned, bool)>, Bailout>) -> bool {
    match decoded {
        Ok(Some((value, done))) => {
            call.set_owned(value);
            done
        }
        Ok(None) => false,
        Err(bailout) => {
            bailout.defer();
            false
        }
    }
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
            i32::from(SERIALIZE.bits())
        } else {
            0
        };
        finish(server, payload, flags, req)
    })?;
    Ok(finished.unwrap_or(false))
}

/// `php_swoole_server_task_pack`: the task id, or `None`; a trapped bailout waits
/// for the handler's end.
pub(crate) fn pack(data: &Value, buf: &mut TaskBuffer) -> Option<i64> {
    encode(data, buf).unwrap_or_else(|bailout| {
        bailout.defer();
        None
    })
}

/// `buf->info.ext_flags |= flags`.
fn flag(buf: &mut TaskBuffer, flags: TaskFlag) {
    buf.add_flags(flags.bits());
}

/// `serv->task(buf, dst_worker_id)`: to `dst`, else a worker the pool schedules.
fn dispatch(server: view::Server<'_>, buf: &mut TaskBuffer, dst: Option<u32>) -> bool {
    task_state(server)
        .task(buf.packed(), dst, false)
        .unwrap_or_else(|Bailed| {
            Bailout.defer();
            false
        })
}

/// `php_swoole_server_task_check_param`.
/// A `dst_worker_id`: negative (-1, any) leaves the pool to schedule one.
fn target(dst_worker_id: i64) -> Option<u32> {
    u32::try_from(dst_worker_id.c_cast::<c_int>()).ok()
}

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

/// Release outside the table borrow so the callable's destructor can submit another task.
fn release_callback(id: i64) {
    drop(crate::lifecycle::with_tasks(|p| p.callbacks.remove(&id)).map(ManuallyDrop::into_inner));
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
    let mut buf = TaskBuffer::default();
    let Some(id) = pack(data, &mut buf) else {
        return call.set_bool(false);
    };
    let callback = call.value(3).filter(|zfn| zfn.truthy());
    if !facts.worker {
        flag(&mut buf, NOREPLY);
    } else if let Some(callback) = callback {
        flag(&mut buf, CALLBACK);
        let Some(cb) = crate::callable::create(callback) else {
            return call.set_bool(false);
        };
        // A replaced callable is freed once the table is no longer borrowed.
        let old = crate::lifecycle::with_tasks(|p| p.callbacks.insert(id, ManuallyDrop::new(cb)));
        drop(old.map(ManuallyDrop::into_inner));
    }
    flag(&mut buf, NONBLOCK);
    if dispatch(serv, &mut buf, target(dst)) {
        call.set_long(id);
    } else {
        if TaskFlag::from_bits(buf.packed().head().ext_flags).contains(CALLBACK) {
            release_callback(id);
        }
        call.set_bool(false);
    }
}

fn taskwait(call: &Call) {
    let Some((serv, facts)) = worker(call, c"taskwait") else {
        return;
    };
    let (Ok(()), Ok(timeout), Ok(dst)) = (
        call.arity(1, 3),
        call.double(2, SW_TASKWAIT_TIMEOUT),
        call.long(3, -1),
    ) else {
        return call.set_bool(false);
    };
    let Some(data) = call.value(1).filter(|_| check(&facts, dst)) else {
        return call.set_bool(false);
    };
    let mut buf = TaskBuffer::default();
    let Some(id) = pack(data, &mut buf) else {
        return call.set_bool(false);
    };
    let dst = target(dst);
    if Coroutine::current().is_none() {
        let Some(mut tasks) = wait_state(serv) else {
            return call.set_bool(false);
        };
        let done = match tasks.task_sync(buf.packed(), dst, timeout) {
            // `serv->get_task_result()`: this worker's slot of `task_results`.
            Ok(true) => tasks.result().read(|result| {
                Packed::from_bytes(result)
                    .is_some_and(|result| return_decoded(call, decode(result)))
            }),
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
    let task_co = Rc::new(TaskCo {
        waiter: Waiter::default(),
        result: Cell::default(),
        count: 1,
    });
    if !dispatch(serv, &mut buf, dst) {
        return call.set_bool(false);
    }
    crate::lifecycle::with_tasks(|p| p.coroutines.insert(id, (Rc::downgrade(&task_co), None)));
    // onFinish resumes it.
    #[expect(clippy::disallowed_methods, reason = "taskwait's $timeout, from PHP")]
    let done = task_co.waiter.wait_for(Deadline::from_timeout(timeout));
    crate::lifecycle::with_tasks(|p| p.coroutines.remove(&id));
    if done {
        call.set_owned(task_co.result.take());
    } else {
        call.set_bool(false);
    }
}

/// Hold the array across serialization, which can run PHP and replace the caller's array.
fn task_arguments(call: &Call) -> Option<(Owned, f64)> {
    call.arity(1, 2).ok()?;
    let array = call.array(1).ok()?.copy();
    Some((array, call.double(2, SW_TASKWAIT_TIMEOUT).ok()?))
}

fn task_wait_multi(call: &Call) {
    task_batch(call, c"taskWaitMulti");
}

fn task_co(call: &Call) {
    task_batch(call, c"taskCo");
}

fn task_batch(call: &Call, method: &CStr) {
    let Some((serv, facts)) = worker(call, method) else {
        return;
    };
    let Some((snapshot, timeout)) = task_arguments(call) else {
        return call.set_bool(false);
    };
    if !check(&facts, -1) {
        return call.set_bool(false);
    }
    let Some(tasks) = snapshot.array() else {
        return call.set_bool(false);
    };
    let n_task = tasks.count();
    if n_task >= SW_MAX_CONCURRENT_TASK {
        fatal(E_WARNING, c"too many concurrent tasks");
        return call.set_bool(false);
    }
    if n_task == 0 {
        set_last_error(SW_ERROR_TASK_DISPATCH_FAIL);
        return call.set_bool(false);
    }
    set_last_error(0);
    if swoole_rs_phpcoroutines::in_coroutine() {
        return task_batch_coroutine(call, serv, tasks, timeout);
    }
    let values = tasks.entries().map(|(_, value)| value).collect();
    // The results, which become the return value only once every wait is over.
    let mut out = OwnedArray::default();
    let mut multi = Multi {
        tasks: values,
        out: &mut out,
        count: u16::try_from(n_task).unwrap_or_default(),
    };
    let done = wait_state(serv).is_some_and(|mut tasks| {
        tasks
            .task_sync_many(&mut multi, timeout)
            .unwrap_or_else(|Bailed| {
                Bailout.defer();
                false
            })
    });
    if done || log::last_error() == swoole_rs_error::ErrorCode::SW_ERROR_FILE_EMPTY as c_int {
        if out.array().is_some_and(|results| results.count() < n_task) {
            set_last_error(SW_ERROR_TASK_TIMEOUT);
        }
        call.set_owned(batch_result(tasks, out.into_owned()));
    } else {
        // `zval_ptr_dtor(return_value)`, then `RETURN_FALSE`.
        drop(out);
        call.set_bool(false);
    }
}

/// Restore input order and keys once no more results can arrive.
fn batch_result(tasks: &Array, positional: Owned) -> Owned {
    let results = positional.array();
    if tasks.is_list() && results.is_some_and(|r| r.is_list() && r.count() == tasks.count()) {
        return positional;
    }
    let mut out = OwnedArray::default();
    let mut position = 0;
    tasks.for_each(|key, _| {
        let value = results
            .and_then(|r| r.index(position))
            .map_or_else(|| Owned::bool(false), Value::copy);
        match key {
            swoole_rs_zend::Key::Name(name) => out.insert(name, value),
            swoole_rs_zend::Key::Index(index) => out.update_index(index, value),
        }
        position += 1;
    });
    out.into_owned()
}

fn task_batch_coroutine(call: &Call, serv: view::Server<'_>, tasks: &Array, timeout: f64) {
    let n_task = tasks.count();
    let task_co = Rc::new(TaskCo {
        waiter: Waiter::default(),
        result: Cell::new(OwnedArray::default().into_owned()),
        count: n_task,
    });
    let mut list = Vec::with_capacity(n_task as usize);
    for (_, ztask) in tasks.entries() {
        let index = list.len() as u64;
        let mut buf = TaskBuffer::default();
        // The task's id once it is dispatched.
        let id = if let Some(id) = pack(ztask, &mut buf) {
            flag(&mut buf, NONBLOCK | COROUTINE);
            dispatch(serv, &mut buf, None).then_some(id)
        } else {
            fatal(E_WARNING, c"failed to pack task");
            None
        };
        if let Some(id) = id {
            crate::lifecycle::with_tasks(|p| {
                p.coroutines
                    .insert(id, (Rc::downgrade(&task_co), Some(index)))
            });
        } else {
            task_co.result.with_taken(|result| {
                if let Some(array) = result.array_mut() {
                    array.update_index(index, Owned::bool(false));
                }
            });
        }
        list.push(id);
    }
    if list.iter().all(Option::is_none) {
        set_last_error(SW_ERROR_TASK_DISPATCH_FAIL);
        return call.set_bool(false);
    }
    // onFinish resumes it.
    let complete = task_co
        .result
        .with_taken(|result| result.array().is_some_and(|array| array.count() == n_task));
    #[expect(clippy::disallowed_methods, reason = "taskCo's $timeout, from PHP")]
    let deadline = Deadline::from_timeout(timeout);
    if complete || task_co.waiter.wait_for(deadline) {
        // A late result for a previous wait can set the worker error while parked.
        set_last_error(0);
    } else {
        if log::last_error() == swoole_rs_error::ErrorCode::SW_ERROR_CO_TIMEDOUT as c_int {
            set_last_error(SW_ERROR_TASK_TIMEOUT);
        }
        crate::lifecycle::with_tasks(|p| {
            for id in list.into_iter().flatten() {
                p.coroutines.remove(&id);
            }
        });
    }
    call.set_owned(batch_result(tasks, task_co.result.take()));
}

fn pack_method(call: &Call) {
    if call.arity(1, 1).is_err() {
        return call.set_bool(false);
    }
    let Some(data) = call.value(1) else {
        return call.set_bool(false);
    };
    let mut buf = TaskBuffer::default();
    if pack(data, &mut buf).is_none() {
        return call.set_bool(false);
    }
    flag(&mut buf, NONBLOCK | NOREPLY);
    // RETURN_STRINGL of `EventData::size()` bytes, copied.
    call.set_owned(Owned::string(ZString::new(buf.packed().bytes())));
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
    let done = task.is_some_and(|task| return_decoded(call, decode(task)));
    if !done {
        call.set_bool(false);
    }
}

swoole_rs_zend::handlers! {
    zim_swoole_server_task => task;
    zim_swoole_server_taskwait => taskwait;
    zim_swoole_server_taskWaitMulti => task_wait_multi;
    zim_swoole_server_taskCo => task_co;
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
    Ok(decode(task)?.and_then(|(data, done)| done.then_some(data)))
}

/// `php_swoole_server_onTask`, for a live server and the task it received.
pub(crate) fn on_task(server: view::Server<'_>, task: Packed<'_>) -> Result<bool, Bailout> {
    let counters = server.counters();
    counters.tasking_num.fetch_sub(1, Ordering::SeqCst);
    let Some(data) = unpack(task)? else {
        return Ok(false);
    };
    let facts = facts(server);
    let info = info(task);
    // `data` is the last argument, or the task's property.
    let mut args = [Owned::default(), Owned::default(), data];
    let argc = if facts.task_object {
        let [event, _, data] = &mut args;
        let props = Props::init(event, &TASK);
        props.long(c"worker_id", info.worker_id);
        props.long(c"id", info.id);
        props.set(c"data", data);
        props.double(c"dispatch_time", info.time);
        props.long(c"flags", info.flags);
        crate::server::task_bind(event, &task.head());
        1
    } else {
        args[0] = Owned::long(info.id);
        args[1] = Owned::long(info.worker_id);
        3
    };
    let retval = Owned::default();
    let called = crate::lifecycle::callback(crate::lifecycle::ServerCallback::Task)
        .map_or(Ok(false), |cb| {
            invoke(&cb, used(&args, argc), Some(&retval), facts.task_coroutine)
        });
    let outcome = match called {
        Ok(false) => handler_error("onTask"),
        called => called.map(drop),
    };
    let [event, _, data] = args;
    drop(event);
    let outcome = outcome.and_then(|()| {
        if retval.kind() > swoole_rs_zend::sys::IS_NULL {
            finish_task(server, &retval, Some(&task.head()))?;
            drop(retval);
            swoole_rs_zend::host::uncaught_exception()?;
        }
        Ok(())
    });
    drop(data);
    outcome.map(|()| true)
}

/// `php_swoole_server_onFinish`, for a live server and the result it received.
pub(crate) fn on_finish(server: view::Server<'_>, task: Packed<'_>) -> Result<bool, Bailout> {
    let info = info(task);
    let id = info.id;
    let flags = TaskFlag::from_bits(task.head().ext_flags);
    let Some(data) = unpack(task)? else {
        return Ok(false);
    };
    if flags.contains(COROUTINE) {
        return resume(id, data).map(|()| true);
    }
    let callback = flags.contains(CALLBACK);
    let cb = if callback {
        // Its own reference: the callback may replace the pending entry.
        crate::lifecycle::with_tasks(|p| p.callbacks.get(&id).map(|cb| Callable::clone(cb)))
    } else {
        crate::lifecycle::callback(crate::lifecycle::ServerCallback::Finish)
    };
    let Some(cb) = cb else {
        if callback {
            expired(id);
            return Ok(true);
        }
        let outcome = event_warning(c"require 'onFinish' callback");
        drop(data);
        return outcome.map(|()| false);
    };
    let facts = facts(server);
    // `data` is the last argument, or the event's property.
    let mut args = [Owned::default(), data];
    let argc = if facts.event_object {
        let [event, data] = &mut args;
        let props = Props::init(event, &DATA[TASK_RESULT]);
        props.long(c"task_id", id);
        props.long(c"task_worker_id", info.worker_id);
        props.double(c"dispatch_time", info.time);
        props.set(c"data", data);
        1
    } else {
        args[0] = Owned::long(id);
        2
    };
    let outcome = match invoke(&cb, used(&args, argc), None, facts.enable_coroutine) {
        Ok(false) => handler_error("onFinish"),
        called => called.map(drop),
    };
    if callback {
        release_callback(id);
    }
    drop(args);
    outcome.map(|()| true)
}

fn expired(id: i64) {
    log::error_log(
        SW_LOG_WARNING,
        SW_ERROR_TASK_TIMEOUT,
        c"php_swoole_server_onFinish",
        format_args!("task[{id}] has expired"),
    );
}

/// A coroutine task's result: into `taskwait`'s return value, or at its index
/// in `taskCo`'s array; the waiting coroutine resumes once all arrived.
fn resume(id: i64, data: Owned) -> Result<(), Bailout> {
    let Some((task_co, index)) = crate::lifecycle::with_tasks(|p| {
        let (task, index) = p.coroutines.remove(&id)?;
        Some((task.upgrade()?, index))
    }) else {
        expired(id);
        return Ok(());
    };
    if let Some(index) = index {
        // Removing the pending entry lets each id arrive once, so its index
        // is empty; a result that has no array keeps `data` for the caller.
        let (arrived, unplaced) = task_co
            .result
            .with_taken(|result| match result.array_mut() {
                Some(array) => {
                    array.update_index(index, data);
                    (array.array().map_or(0, Array::count), None)
                }
                None => (0, Some(data)),
            });
        drop(unplaced);
        if arrived != task_co.count {
            return Ok(());
        }
    } else {
        drop(task_co.result.replace(data));
    }
    task_co.waiter.wake()
}
