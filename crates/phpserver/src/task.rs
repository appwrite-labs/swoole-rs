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
    cell::{Cell, RefCell},
    collections::HashMap,
    ffi::{CStr, c_int},
    mem::ManuallyDrop,
    rc::{Rc, Weak},
    sync::atomic::Ordering,
};
use swoole_rs_buffer::string::CppString;
use swoole_rs_corebase::CCast;
use swoole_rs_corebase::{DataHead, log};
use swoole_rs_locks::SharedMutex;
use swoole_rs_phpcoroutines::Coroutine;
use swoole_rs_phpswoole::unserialize_from;
use swoole_rs_process::{
    methods::{self, swoole_get_worker_id},
    process_pool::PoolRef,
    task_file::{Packed, TaskBuffer},
    task_sync::{Bailed, MultiTask, TaskHost, Tasks, Waiting},
};
use swoole_rs_server::{
    task_worker::{native::finish, result_slot},
    view,
};
use swoole_rs_tls::socket_pair::{self, SocketPair};
use swoole_rs_zend::{
    Array, ArrayMut, Call, E_WARNING, Owned, OwnedArray, Value, ZString, bailout::Bailout,
    long_zval, message, report,
};

/// `SW_TASK_*` flags.
const SERIALIZE: u32 = 1 << 1;
const NONBLOCK: u32 = 1 << 2;
const CALLBACK: u32 = 1 << 3;
const COROUTINE: u32 = 1 << 5;
const NOREPLY: u32 = 1 << 7;
/// `SW_MAX_CONCURRENT_TASK`, `SW_TASKWAIT_TIMEOUT`.
const MAX_CONCURRENT_TASK: u32 = 1024;
const TASKWAIT_TIMEOUT: f64 = 0.5;
/// `SW_ERROR_PHP_FATAL_ERROR`, `SW_ERROR_TASK_DISPATCH_FAIL`.
const SW_ERROR_PHP_FATAL_ERROR: c_int = 503;
const SW_ERROR_TASK_DISPATCH_FAIL: c_int = 2002;
/// `SW_ERROR_TASK_TIMEOUT`, `SW_LOG_WARNING`.
const SW_ERROR_TASK_TIMEOUT: c_int = 2003;
const SW_LOG_WARNING: c_int = swoole_rs_corebase::log::LogLevel::SW_LOG_WARNING as c_int;
const SW_OK: c_int = swoole_rs_corebase::constants::ResultCode::SW_OK as c_int;
const SW_ERR: c_int = swoole_rs_corebase::constants::ResultCode::SW_ERR as c_int;
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
    fn dispatch(&mut self, task: Packed<'_>, worker_id: &mut c_int, blocking: bool) -> bool {
        methods::dispatch(self.pool, task, worker_id, blocking)
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

    fn read_notify(&mut self) -> Option<usize> {
        let pipe = self.pipe.as_ref()?;
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
    // SAFETY: this worker's live notify pipe, which only this call uses meanwhile.
    tasks.host.pipe = unsafe { serv.task_notify_pipe(id).as_mut() };
    tasks.host.lock = serv
        .worker_record(id)
        .and_then(swoole_rs_process::worker_record::Worker::lock);
    let slot = result_slot(serv, id)?;
    Some(tasks.waiting(slot))
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

    fn pack(&mut self, index: u16, event: &mut TaskBuffer) -> Result<Option<i64>, Bailed> {
        let Some(data) = self.tasks.and_then(|tasks| tasks.index(u64::from(index))) else {
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

/// Upstream's `TaskCo`: a coroutine waiting in `taskwait` (`list` empty) or
/// `taskCo`/`taskWaitMulti` for results written into `result`, which becomes
/// the call's return value. The waiting call owns it; the pending table refers to it.
struct TaskCo {
    co: Coroutine,
    result: RefCell<Owned>,
    /// Set once every task is sent, as upstream.
    list: RefCell<Vec<Option<i64>>>,
    count: Cell<u32>,
}

/// One server's pending tasks.
#[derive(Default)]
struct Pending {
    /// Never freed with the table: a thread exits after Zend shut down.
    callbacks: HashMap<i64, ManuallyDrop<Callable>>,
    /// The waiting call owns each.
    coroutines: HashMap<i64, Weak<TaskCo>>,
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
    let mut buffer = CppString::EMPTY;
    let decoded = decode_into(task, &mut buffer);
    buffer.dispose();
    decoded
}

/// [`decode`] with `buffer` for a temporary file's bytes.
fn decode_into(task: Packed<'_>, buffer: &mut CppString) -> Result<Option<(Owned, bool)>, Bailout> {
    let Some((data, flags)) = task.unpack(buffer) else {
        return Ok(None);
    };
    if u32::from(flags) & SERIALIZE == 0 {
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
            SERIALIZE.cast_signed()
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
fn flag(buf: &mut TaskBuffer, flags: u32) {
    let flags = flags.c_cast::<u16>();
    buf.add_flags(flags);
}

/// `serv->task(buf, dst_worker_id)`.
fn dispatch(server: view::Server<'_>, buf: &mut TaskBuffer, dst: &mut c_int) -> bool {
    task_state(server)
        .task(buf.packed(), dst, false)
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
        let old = pending(serv, |p| p.callbacks.insert(id, ManuallyDrop::new(cb)));
        drop(old.map(ManuallyDrop::into_inner));
    }
    flag(&mut buf, NONBLOCK);
    let mut dst = dst.c_cast::<c_int>();
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
    let mut buf = TaskBuffer::default();
    let Some(id) = pack(data, &mut buf) else {
        return call.set_bool(false);
    };
    let mut dst = dst.c_cast::<c_int>();
    let Some(co) = Coroutine::current() else {
        let Some(mut tasks) = wait_state(serv) else {
            return call.set_bool(false);
        };
        let done = match tasks.task_sync(buf.packed(), &mut dst, timeout) {
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
    };
    flag(&mut buf, NONBLOCK | COROUTINE);
    let task_co = Rc::new(TaskCo {
        co,
        result: RefCell::default(),
        list: RefCell::default(),
        count: Cell::new(1),
    });
    if !dispatch(serv, &mut buf, &mut dst) {
        return call.set_bool(false);
    }
    pending(serv, |p| p.coroutines.insert(id, Rc::downgrade(&task_co)));
    // onFinish resumes it.
    let done = swoole_rs_phpcoroutines::suspend_for(timeout);
    pending(serv, |p| p.coroutines.remove(&id));
    if done {
        call.set_owned(task_co.result.take());
    } else {
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
    let done = wait_state(serv).is_some_and(|mut tasks| {
        tasks
            .task_sync_many(&mut multi, timeout)
            .unwrap_or_else(|Bailed| {
                Bailout.defer();
                false
            })
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
    let task_co = Rc::new(TaskCo {
        co,
        result: RefCell::new(OwnedArray::default().into_owned()),
        list: RefCell::default(),
        count: Cell::new(0),
    });
    let mut list = Vec::with_capacity(n_task as usize);
    for (_, ztask) in tasks.into_iter().flat_map(Array::entries) {
        let index = list.len() as u64;
        let mut buf = TaskBuffer::default();
        // The task's id once it is dispatched.
        let id = if let Some(id) = pack(ztask, &mut buf) {
            flag(&mut buf, NONBLOCK | COROUTINE);
            let mut dst: c_int = -1;
            dispatch(serv, &mut buf, &mut dst).then_some(id)
        } else {
            fatal(E_WARNING, c"failed to pack task");
            None
        };
        if let Some(id) = id {
            pending(serv, |p| p.coroutines.insert(id, Rc::downgrade(&task_co)));
        } else {
            if let Some(array) = task_co.result.borrow_mut().array_mut() {
                array.update_index(index, Owned::bool(false));
            }
            n_task -= 1;
        }
        list.push(id);
    }
    if n_task == 0 {
        set_last_error(SW_ERROR_TASK_DISPATCH_FAIL);
        return call.set_bool(false);
    }
    task_co.list.replace(list);
    task_co.count.set(n_task);
    // onFinish resumes it.
    if !swoole_rs_phpcoroutines::suspend_for(timeout)
        && let Some(array) = task_co.result.borrow_mut().array_mut()
    {
        for j in 0..n_task as usize {
            if array.array().and_then(|a| a.index(j as u64)).is_none() {
                if fill_false {
                    array.update_index(j as u64, Owned::bool(false));
                }
                if let Some(&Some(id)) = task_co.list.borrow().get(j) {
                    pending(serv, |p| p.coroutines.remove(&id));
                }
            }
        }
    }
    call.set_owned(task_co.result.take());
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
    call.set_owned(Owned::string(ZString::new(buf.bytes())));
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
    Ok(decode(task)?.and_then(|(data, done)| done.then_some(data)))
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
        crate::server::task_bind(Value::of(&mut args[1]), &task.head());
        2
    } else {
        args[1] = long_zval(info.id);
        args[2] = long_zval(info.worker_id);
        args[3] = data;
        4
    };
    let retval = Owned::default();
    let called = crate::lifecycle::callback(crate::lifecycle::ServerCallback::Task).map_or(
        Ok(false),
        |cb| {
            invoke(
                &cb,
                args.get_mut(..argc).unwrap_or_default(),
                Some(&retval),
                facts.task_coroutine,
            )
        },
    );
    let outcome = match called {
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
    let mut flags = info.flags.c_cast::<u32>();
    if flags & COROUTINE != 0 {
        return resume(server, id, data).map(|()| SW_OK);
    }
    let mut data = data.into_zval();
    let cb = if flags & CALLBACK != 0 {
        // Its own reference: the callback may replace the pending entry.
        let cb = pending(server, |p| {
            p.callbacks.get(&id).map(|cb| Callable::clone(cb))
        });
        if cb.is_none() {
            flags &= !CALLBACK;
        }
        cb
    } else {
        crate::lifecycle::callback(crate::lifecycle::ServerCallback::Finish)
    };
    let Some(cb) = cb else {
        let outcome = event_warning(c"require 'onFinish' callback");
        dtor(&mut data);
        return outcome.map(|()| SW_ERR);
    };
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
        &cb,
        args.get_mut(..argc).unwrap_or_default(),
        None,
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
    let Some(task_co) = pending(server, |p| p.coroutines.get(&id).and_then(Weak::upgrade)) else {
        log::error_log(
            SW_LOG_WARNING,
            SW_ERROR_TASK_TIMEOUT,
            c"php_swoole_server_onFinish",
            format_args!("task[{id}] has expired"),
        );
        return Ok(());
    };
    let (index, single) = {
        let list = task_co.list.borrow();
        (list.iter().position(|&t| t == Some(id)), list.is_empty())
    };
    if single {
        // ZVAL_COPY_VALUE: the result takes the value.
        drop(task_co.result.replace(data));
    } else {
        let Some(index) = index else {
            let text = message!("task[{id}] is invalid");
            return event_warning(text.as_cstr());
        };
        if let Some(array) = task_co.result.borrow_mut().array_mut() {
            array.update_index(index as u64, data);
        }
        pending(server, |p| p.coroutines.remove(&id));
        let arrived = task_co.result.borrow().array().map_or(0, Array::count);
        if arrived != task_co.count.get() {
            return Ok(());
        }
    }
    let co = task_co.co;
    drop(task_co);
    // SAFETY: the waiting coroutine; this frame owns nothing that needs a drop.
    unsafe { co.wake(false) };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_tasks_are_per_server() {
        let (a, b) = (0x1000, 0x2000);
        // An all-zero callable (an undefined zval, an empty cache), never called
        // or dropped.
        let callable: Callable = unsafe { std::rc::Rc::new_zeroed().assume_init() };
        let callable = ManuallyDrop::new(callable);
        let raw = std::rc::Rc::as_ptr(&callable);
        pending_at(a, |p| p.callbacks.insert(7, callable));
        assert_eq!(
            pending_at(b, |p| p.callbacks.get(&7).map(|c| std::rc::Rc::as_ptr(c))),
            None
        );
        let removed = pending_at(a, |p| p.callbacks.remove(&7));
        assert_eq!(removed.map(|c| std::rc::Rc::as_ptr(&c)), Some(raw));
    }
}
