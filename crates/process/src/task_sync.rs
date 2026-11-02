//! `Server::task` and the blocking `Server::task_sync` pair (one task, or a
//! `MultiTask` gathered through a temporary file): the task counters, dispatch
//! and rollback, waiting on the worker's notify pipe for the matching result or
//! for every result, and walking the result file. Native code lends its objects
//! through [`TaskHooks`]: the task pool's dispatch, the notify pipe, the
//! worker's lock, the temporary file, and the `MultiTask`'s PHP pack/unpack.
//! Hooks that can reach PHP run inside the bailout trap ([`crate::pool`]); a
//! bailout returns [`crate::pool::BAILED`] for the native caller to re-raise.
use std::ffi::{c_int, c_void};
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::pool::{BAILED, Bailed, DONE, PoolCall, Step, run};

/// `swoole::DataHead`: `fd` (the task id) at 0, `len` at 16, `ext_flags` at 26.
const HEAD: usize = 40;
const HEAD_LEN: usize = 16;
const HEAD_EXT_FLAGS: usize = 26;
/// `SW_TASK_WAITALL`.
const TASK_WAITALL: u16 = 1 << 4;

/// Warnings native code formats.
pub const WARN_PACK: c_int = 1;
pub const WARN_DISPATCH: c_int = 2;
/// `swr_v1_process_task_sync_many`'s `*ok` when no task could be dispatched
/// (`SW_ERROR_TASK_DISPATCH_FAIL`).
pub const NONE_DISPATCHED: c_int = 2;

/// The server's task objects, one function per operation. A [`PoolCall`]'s
/// `pool` is the server, `data`/`length` the operation's buffer, `flags` its
/// index or option, `result` its return value.
#[repr(C)]
pub struct TaskHooks {
    /// `get_task_worker_pool()->dispatch_sync` (`flags` 1) or `dispatch` (0) of
    /// the `EventData` at `data`, to the worker at `socket` (an `int*`): 1 when sent.
    pub dispatch: Step,
    /// The notify pipe: `clean()`.
    pub pipe_clean: Step,
    /// The notify pipe: `set_timeout(timeout)`, the `f64` at `data`.
    pub pipe_timeout: Step,
    /// The notify pipe: `read(data, length)`.
    pub pipe_read: Step,
    /// The worker's lock: `lock()` (`flags` 1) or `unlock()` (0).
    pub lock: Step,
    /// `mtask.pack(flags, data)`: the task id, negative when it failed.
    pub pack: Step,
    /// `mtask.unpack(flags, data)`.
    pub unpack: Step,
    /// `mtask.fail(flags)`.
    pub fail: Step,
    /// `file_get_contents(path)` under the lock: the file's bytes at `data`, its
    /// length in `length` (null when it could not be read), freed by `release`.
    pub contents: Step,
    pub release: Step,
    /// Report a `WARN_*`.
    pub warn: unsafe extern "C" fn(kind: c_int),
}

/// The server state a task call works on.
#[repr(C)]
pub struct TaskServer {
    pub hooks: *const TaskHooks,
    pub server: *mut c_void,
    /// The `MultiTask` being gathered (each call's `worker`), or null.
    pub mtask: *mut c_void,
    /// `gs->tasking_num` and `gs->task_count`, shared with every process.
    pub tasking_num: *mut u32,
    pub task_count: *mut i64,
    /// This worker's `task_results` slot (an `EventData`).
    pub result: *mut u8,
    pub result_size: usize,
}

struct Server<'a> {
    hooks: &'a TaskHooks,
    server: *mut c_void,
    mtask: *mut c_void,
    tasking: &'a AtomicU32,
    count: &'a AtomicI64,
    result: *mut u8,
}

impl Server<'_> {
    fn call(
        &self,
        step: Step,
        data: *mut u8,
        length: usize,
        flags: c_int,
    ) -> Result<PoolCall, Bailed> {
        let mut call = PoolCall::new(self.server, self.mtask);
        call.data = data.cast();
        call.length = length;
        call.flags = flags;
        run(step, &mut call)?;
        Ok(call)
    }

    fn result_task_id(&self) -> i64 {
        // SAFETY: `info.fd` heads this worker's result slot.
        unsafe { self.result.cast::<i64>().read_unaligned() }
    }

    /// `Server::task`: count the task, dispatch it, and roll the count back on failure.
    fn task(&self, task: *mut u8, worker_id: *mut c_int, blocking: bool) -> Result<bool, Bailed> {
        self.tasking.fetch_add(1, Ordering::SeqCst);
        let mut call = PoolCall::new(self.server, std::ptr::null_mut());
        call.socket = worker_id.cast();
        call.data = task.cast();
        call.flags = c_int::from(blocking);
        run(self.hooks.dispatch, &mut call)?;
        if call.result != 0 {
            self.count.fetch_add(1, Ordering::SeqCst);
            return Ok(true);
        }
        self.tasking.fetch_sub(1, Ordering::SeqCst);
        Ok(false)
    }

    fn set_timeout(&self, mut timeout: f64) -> Result<(), Bailed> {
        self.call(self.hooks.pipe_timeout, (&raw mut timeout).cast(), 0, 0)
            .map(drop)
    }

    fn read_notify(&self) -> Result<isize, Bailed> {
        let mut notify = 0u64;
        Ok(self
            .call(self.hooks.pipe_read, (&raw mut notify).cast(), 8, 0)?
            .result)
    }

    fn lock(&self, on: bool) -> Result<(), Bailed> {
        self.call(self.hooks.lock, std::ptr::null_mut(), 0, c_int::from(on))
            .map(drop)
    }

    /// One task: dispatch it and wait for its own result.
    fn sync_one(
        &self,
        task: *mut u8,
        worker_id: *mut c_int,
        timeout: f64,
        result_size: usize,
    ) -> Result<bool, Bailed> {
        // SAFETY: this worker's result slot.
        unsafe { self.result.write_bytes(0, result_size) };
        // SAFETY: `info.fd` heads the task.
        let task_id = unsafe { task.cast::<i64>().read_unaligned() };
        self.call(self.hooks.pipe_clean, std::ptr::null_mut(), 0, 0)?;
        self.set_timeout(timeout)?;
        if !self.task(task, worker_id, true)? {
            return Ok(false);
        }
        while self.read_notify()? > 0 {
            if self.result_task_id() == task_id {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Several tasks: dispatch each (WAITALL) and wait until every result is in
    /// the file, then unpack each by its task id. `path` is the result file,
    /// already named in this worker's result slot; `event_size` is `sizeof(EventData)`.
    fn sync_many(
        &self,
        count: u16,
        timeout: f64,
        path: &std::ffi::CStr,
        event_size: usize,
    ) -> Result<c_int, Bailed> {
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "the result slot is an EventData, 8-aligned; its count is the first word"
        )]
        let finish_count = self.result.wrapping_add(HEAD).cast::<AtomicU32>();
        self.call(self.hooks.pipe_clean, std::ptr::null_mut(), 0, 0)?;

        let mut pending = i64::from(count);
        let mut indexes = std::collections::HashMap::new();
        let mut event = vec![0u64; event_size.div_ceil(8)];
        for index in 0..count {
            let mut task_id = self
                .call(
                    self.hooks.pack,
                    event.as_mut_ptr().cast(),
                    event_size,
                    c_int::from(index),
                )?
                .result as i64;
            let mut failed = task_id < 0;
            if failed {
                // SAFETY: native's warning.
                unsafe { (self.hooks.warn)(WARN_PACK) };
            } else {
                // `info.ext_flags`: bytes 26..28, in the event's fourth word.
                event[HEAD_EXT_FLAGS / 8] |= u64::from(TASK_WAITALL) << (8 * (HEAD_EXT_FLAGS % 8));
                self.tasking.fetch_add(1, Ordering::SeqCst);
                let mut worker_id: c_int = -1;
                if self.task(event.as_mut_ptr().cast(), &raw mut worker_id, true)? {
                    self.tasking.fetch_sub(1, Ordering::SeqCst);
                } else {
                    // SAFETY: native's warning.
                    unsafe { (self.hooks.warn)(WARN_DISPATCH) };
                    task_id = -1;
                    failed = true;
                }
            }
            if failed {
                self.call(self.hooks.fail, std::ptr::null_mut(), 0, c_int::from(index))?;
                pending -= 1;
            }
            indexes.insert(task_id, index);
        }
        if pending == 0 {
            return Ok(NONE_DISPATCHED);
        }
        if timeout > 0.0 {
            self.set_timeout(timeout)?;
        }
        let started = now();
        // SAFETY: the finish count heads the result slot; task workers add to it.
        while i64::from(unsafe { &*finish_count }.load(Ordering::SeqCst)) < pending {
            if self.read_notify()? <= 0 || (timeout > 0.0 && now() - started > timeout) {
                break;
            }
        }

        self.lock(true)?;
        let contents = self.call(self.hooks.contents, path.as_ptr().cast_mut().cast(), 0, 0);
        self.lock(false)?;
        let contents = contents?;
        if contents.data.is_null() {
            return Ok(0);
        }
        // SAFETY: the file's bytes native read, until `release`.
        let bytes =
            unsafe { std::slice::from_raw_parts(contents.data.cast::<u8>(), contents.length) };
        let mut offset = 0;
        let mut unpacked = Ok(());
        while offset + HEAD <= bytes.len() {
            let record = &bytes[offset..];
            let task_id = i64::from_ne_bytes(record[..8].try_into().unwrap_or_default());
            let len = u32::from_ne_bytes(
                record[HEAD_LEN..HEAD_LEN + 4]
                    .try_into()
                    .unwrap_or_default(),
            );
            if let Some(&index) = indexes.get(&task_id)
                && let Err(bailed) = self.call(
                    self.hooks.unpack,
                    record.as_ptr().cast_mut(),
                    record.len(),
                    c_int::from(index),
                )
            {
                unpacked = Err(bailed);
                break;
            }
            offset += HEAD + len as usize;
        }
        let mut release = PoolCall::new(self.server, std::ptr::null_mut());
        release.data = contents.data;
        release.socket = contents.socket;
        // SAFETY: native frees what `contents` returned; this cannot reach PHP.
        unsafe { (self.hooks.release)((&raw mut release).cast()) };
        unpacked?;
        // SAFETY: the result file's NUL-terminated path.
        unsafe { libc::unlink(path.as_ptr()) };
        Ok(1)
    }
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_secs_f64())
}

fn server(task_server: &TaskServer) -> Server<'_> {
    Server {
        // SAFETY: the caller's hooks.
        hooks: unsafe { &*task_server.hooks },
        server: task_server.server,
        mtask: task_server.mtask,
        // SAFETY: the shared counters, updated atomically by every process.
        tasking: unsafe { AtomicU32::from_ptr(task_server.tasking_num) },
        // SAFETY: as above.
        count: unsafe { AtomicI64::from_ptr(task_server.task_count) },
        result: task_server.result,
    }
}

fn outcome<T>(result: Result<T, Bailed>, ok: *mut T) -> c_int {
    match result {
        Ok(done) => {
            // SAFETY: the caller's out-value.
            unsafe { ok.write(done) };
            DONE
        }
        Err(Bailed) => BAILED,
    }
}

/// `Server::task`.
///
/// # Safety
/// `task_server` is live; `task` an `EventData`; `worker_id` writable; `ok` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_process_task(
    task_server: *const TaskServer,
    task: *mut u8,
    worker_id: *mut c_int,
    blocking: bool,
    ok: *mut bool,
) -> c_int {
    // SAFETY: the caller's state.
    let server = server(unsafe { &*task_server });
    outcome(server.task(task, worker_id, blocking), ok)
}

/// `Server::task_sync` of one task: [`DONE`] with `*ok`, or [`BAILED`].
///
/// # Safety
/// As [`swr_v1_process_task`]; the result slot holds `result_size` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_process_task_sync(
    task_server: *const TaskServer,
    task: *mut u8,
    worker_id: *mut c_int,
    timeout: f64,
    ok: *mut bool,
) -> c_int {
    // SAFETY: the caller's state.
    let task_server = unsafe { &*task_server };
    let server = server(task_server);
    outcome(
        server.sync_one(task, worker_id, timeout, task_server.result_size),
        ok,
    )
}

/// `Server::task_sync` of a `MultiTask` of `count` tasks, gathered in the file
/// at `path` (already named in the result slot, its count zeroed): `*ok` is 1
/// when the results were unpacked, 0 when they could not be read, or
/// [`NONE_DISPATCHED`].
///
/// # Safety
/// As [`swr_v1_process_task`]; `path` is NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_process_task_sync_many(
    task_server: *const TaskServer,
    count: u16,
    timeout: f64,
    path: *const std::ffi::c_char,
    event_size: usize,
    ok: *mut c_int,
) -> c_int {
    // SAFETY: the caller's state and path.
    let (server, path) = (server(unsafe { &*task_server }), unsafe {
        std::ffi::CStr::from_ptr(path)
    });
    outcome(server.sync_many(count, timeout, path, event_size), ok)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A scripted server: dispatch results, notifications, the result file.
    #[derive(Default)]
    struct Script {
        dispatched: Vec<bool>,
        packs: Vec<isize>,
        notify: Vec<isize>,
        file: Vec<u8>,
        result: Vec<u64>,
        unpacked: Vec<(c_int, i64)>,
        failed: Vec<c_int>,
        warnings: Vec<c_int>,
        locks: Vec<c_int>,
    }

    thread_local! {
        static SCRIPT: RefCell<Script> = RefCell::default();
    }

    fn with<R>(f: impl FnOnce(&mut Script) -> R) -> R {
        SCRIPT.with(|script| f(&mut script.borrow_mut()))
    }

    fn call(arg: *mut c_void) -> &'static mut PoolCall {
        // SAFETY: the call under test.
        unsafe { &mut *arg.cast::<PoolCall>() }
    }

    unsafe extern "C" fn dispatch(arg: *mut c_void) {
        call(arg).result = isize::from(with(|s| s.dispatched.remove(0)));
    }
    unsafe extern "C" fn nothing(_: *mut c_void) {}
    unsafe extern "C" fn pipe_read(arg: *mut c_void) {
        call(arg).result = with(|s| {
            if s.notify.is_empty() {
                -1
            } else {
                s.notify.remove(0)
            }
        });
    }
    unsafe extern "C" fn lock(arg: *mut c_void) {
        with(|s| s.locks.push(call(arg).flags));
    }
    unsafe extern "C" fn pack(arg: *mut c_void) {
        let c = call(arg);
        let id = with(|s| s.packs.remove(0));
        // SAFETY: the event buffer under test.
        unsafe { c.data.cast::<i64>().write(id as i64) };
        c.result = id;
    }
    unsafe extern "C" fn unpack(arg: *mut c_void) {
        let c = call(arg);
        // SAFETY: a record of the file under test.
        let id = unsafe { c.data.cast::<i64>().read_unaligned() };
        with(|s| s.unpacked.push((c.flags, id)));
    }
    unsafe extern "C" fn fail(arg: *mut c_void) {
        with(|s| s.failed.push(call(arg).flags));
    }
    unsafe extern "C" fn contents(arg: *mut c_void) {
        let c = call(arg);
        with(|s| {
            c.data = s.file.as_mut_ptr().cast();
            c.length = s.file.len();
        });
    }
    unsafe extern "C" fn warn(kind: c_int) {
        with(|s| s.warnings.push(kind));
    }

    static HOOKS: TaskHooks = TaskHooks {
        dispatch,
        pipe_clean: nothing,
        pipe_timeout: nothing,
        pipe_read,
        lock,
        pack,
        unpack,
        fail,
        contents,
        release: nothing,
        warn,
    };

    fn record(task_id: i64, payload: &[u8]) -> Vec<u8> {
        let mut record = vec![0u8; HEAD];
        record[..8].copy_from_slice(&task_id.to_ne_bytes());
        record[HEAD_LEN..HEAD_LEN + 4]
            .copy_from_slice(&u32::try_from(payload.len()).unwrap().to_ne_bytes());
        record.extend_from_slice(payload);
        record
    }

    fn state(tasking: &mut u32, count: &mut i64) -> TaskServer {
        TaskServer {
            hooks: &raw const HOOKS,
            server: std::ptr::null_mut(),
            mtask: std::ptr::null_mut(),
            tasking_num: tasking,
            task_count: count,
            result: with(|s| s.result.as_mut_ptr().cast()),
            result_size: 64,
        }
    }

    #[test]
    fn a_task_counts_and_rolls_back() {
        with(|s| {
            *s = Script {
                dispatched: vec![true, false],
                ..Script::default()
            }
        });
        let (mut tasking, mut count) = (0, 0);
        let state = state(&mut tasking, &mut count);
        let mut task = [0u64; 8];
        let mut worker_id = -1;
        let mut ok = false;
        for expected in [true, false] {
            // SAFETY: the scripted state.
            let done = unsafe {
                swr_v1_process_task(
                    &raw const state,
                    task.as_mut_ptr().cast(),
                    &raw mut worker_id,
                    false,
                    &raw mut ok,
                )
            };
            assert_eq!((done, ok), (DONE, expected));
        }
        assert_eq!((tasking, count), (1, 1));
    }

    #[test]
    fn a_sync_task_waits_for_its_own_result() {
        with(|s| {
            *s = Script {
                dispatched: vec![true],
                notify: vec![8, 8],
                result: vec![0; 8],
                ..Script::default()
            }
        });
        let (mut tasking, mut count) = (0, 0);
        let state = state(&mut tasking, &mut count);
        let mut task = [7u64, 0, 0, 0, 0, 0, 0, 0];
        let mut worker_id = -1;
        let mut ok = false;
        // Another task's result arrives first; this one never does.
        // SAFETY: the scripted state.
        let done = unsafe {
            swr_v1_process_task_sync(
                &raw const state,
                task.as_mut_ptr().cast(),
                &raw mut worker_id,
                1.0,
                &raw mut ok,
            )
        };
        assert_eq!((done, ok), (DONE, false));
        assert_eq!(with(|s| s.notify.len()), 0);
    }

    #[test]
    fn many_tasks_unpack_each_result_by_id() {
        let mut file = record(11, b"ab");
        file.extend(record(99, b""));
        file.extend(record(12, b"c"));
        // The finish count (the result's data) already says both finished.
        let mut result = vec![0u64; 16];
        result[HEAD / 8] = 2;
        with(|s| {
            *s = Script {
                dispatched: vec![true, true, false],
                packs: vec![11, 12, 13, -1],
                file,
                result,
                ..Script::default()
            };
        });
        let (mut tasking, mut count) = (0, 0);
        let state = state(&mut tasking, &mut count);
        let mut ok = 0;
        let path = c"/nonexistent/swr-task-sync-test";
        // SAFETY: the scripted state.
        let done = unsafe {
            swr_v1_process_task_sync_many(&raw const state, 4, -1.0, path.as_ptr(), 64, &raw mut ok)
        };
        assert_eq!((done, ok), (DONE, 1));
        let script = with(std::mem::take);
        assert_eq!(script.unpacked, vec![(0, 11), (1, 12)]);
        assert_eq!(script.failed, vec![2, 3]);
        assert_eq!(script.warnings, vec![WARN_DISPATCH, WARN_PACK]);
        assert_eq!(script.locks, vec![1, 0]);
        // One in flight per dispatched task (its task worker takes it off), and the
        // failed dispatch keeps its extra count, as upstream.
        assert_eq!((tasking, count), (3, 2));
    }

    #[test]
    fn nothing_dispatched_is_reported() {
        with(|s| {
            *s = Script {
                packs: vec![-1],
                result: vec![0; 16],
                ..Script::default()
            }
        });
        let (mut tasking, mut count) = (0, 0);
        let state = state(&mut tasking, &mut count);
        let mut ok = 0;
        // SAFETY: the scripted state.
        let done = unsafe {
            swr_v1_process_task_sync_many(
                &raw const state,
                1,
                -1.0,
                c"/x".as_ptr(),
                64,
                &raw mut ok,
            )
        };
        assert_eq!((done, ok), (DONE, NONE_DISPATCHED));
    }
}
