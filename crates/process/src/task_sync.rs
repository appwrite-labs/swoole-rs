//! `Server::task` and the blocking `Server::task_sync` pair (one task, or a
//! `MultiTask` gathered through a temporary file): the task counters, dispatch
//! and rollback, waiting on the worker's notify pipe for the matching result or
//! for every result, and walking the result file. The server lends its task pool,
//! notify pipe and lock through [`TaskHost`], and `taskWaitMulti` its tasks through
//! [`MultiTask`]. The dispatch and the pipe's waits can reach PHP (a blocking
//! write or wait dispatches signals), so they run inside the bailout trap
//! ([`crate::pool`]); a bailout returns [`Bailed`] for the caller to re-raise.
use std::ffi::{CStr, c_int};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use swoole_rs_buffer::string::CppString;
use swoole_rs_corebase::log::{self, set_last_error};
use swoole_rs_corebase::task_tmpfile;
use swoole_rs_ossupport::file_get_contents;

pub use crate::pool::Bailed;
use crate::pool::trapped;
use crate::process_pool::SW_IPC_MAX_SIZE;
use crate::task_file::{Packed, TASK_TMP_PATH_SIZE, TaskBuffer, errno, make_tmpfile};

/// `swoole::DataHead`: `fd` (the task id) at 0, `len` at 16.
const HEAD: usize = 40;
const HEAD_LEN: usize = 16;
/// `sizeof(EventData)`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "SW_IPC_MAX_SIZE is 2048 or 8192"
)]
const EVENT_SIZE: usize = SW_IPC_MAX_SIZE as usize;
/// `SW_TASK_WAITALL`.
const TASK_WAITALL: u16 = 1 << 4;
/// `SW_ERROR_TASK_DISPATCH_FAIL`.
const ERROR_TASK_DISPATCH_FAIL: c_int = 2002;

/// What a task call borrows of the server.
pub trait TaskHost {
    /// `get_task_worker_pool()->dispatch_sync` (`blocking`) or `dispatch` of the
    /// packed `task` to `worker_id`: true when sent.
    fn dispatch(&mut self, task: Packed<'_>, worker_id: &mut c_int, blocking: bool) -> bool;
    /// This worker's notify pipe: `clean()`.
    fn pipe_clean(&mut self);
    /// This worker's notify pipe: `set_timeout(timeout)`.
    fn set_timeout(&mut self, timeout: f64);
    /// This worker's notify pipe: `read` of one notification.
    fn read_notify(&mut self) -> isize;
    /// This worker's lock: `lock()` (`on`) or `unlock()`.
    fn lock(&mut self, on: bool);
}

/// `Server::MultiTask`: tasks packed, and their results unpacked, by index.
pub trait MultiTask {
    fn count(&self) -> u16;
    /// Packs task `index` into `event`: its id, negative when it failed.
    ///
    /// # Errors
    /// PHP bailed out (a user `__serialize`).
    fn pack(&mut self, index: u16, event: &mut TaskBuffer) -> Result<i64, Bailed>;
    /// Task `index`'s result, a packed `EventData`.
    ///
    /// # Errors
    /// PHP bailed out (a user `__unserialize`).
    fn unpack(&mut self, index: u16, result: Packed<'_>) -> Result<(), Bailed>;
    /// Task `index` was not dispatched.
    fn fail(&mut self, index: u16);
}

/// The server's task state.
pub struct Tasks<'a, H> {
    pub host: H,
    /// `gs->tasking_num` and `gs->task_count`, shared with every process.
    pub tasking: &'a AtomicU32,
    pub count: &'a AtomicU64,
}

/// A worker's `task_results` slot: an `EventData` in the server's shared memory,
/// which task workers write a result or a finish count into. Read and written a
/// word at a time, as shared memory is; the words are the finish count's size.
#[derive(Clone, Copy)]
pub struct ResultSlot<'a>(&'a [AtomicU32; EVENT_SIZE / 4]);

impl<'a> ResultSlot<'a> {
    /// The slot at `slot`.
    ///
    /// # Safety
    /// `slot` is a worker's `task_results` slot: `SW_IPC_MAX_SIZE` bytes, aligned as
    /// an `EventData`, live for `'a`, and written meanwhile only by the server's
    /// task workers.
    #[must_use]
    pub const unsafe fn new(slot: std::ptr::NonNull<u8>) -> Self {
        // SAFETY: as the caller promises; any bytes are a word.
        Self(unsafe { slot.cast().as_ref() })
    }

    /// The slot made of `words`, the slot's whole `EventData`.
    #[must_use]
    pub const fn of(words: &'a [AtomicU32; EVENT_SIZE / 4]) -> Self {
        Self(words)
    }

    /// Copies `bytes`, a packed result, over the start of the slot; the bytes after
    /// it stay.
    pub fn store(self, bytes: &[u8]) {
        copy_words(self.0, bytes);
    }

    /// The temporary file a waitall slot names after its finish count, up to its
    /// terminator; empty without one.
    #[must_use]
    pub fn result_path(self) -> std::ffi::CString {
        let bytes = self
            .data()
            .iter()
            .skip(1)
            .flat_map(|word| word.load(Ordering::Relaxed).to_ne_bytes())
            .take_while(|&byte| byte != 0)
            .collect::<Vec<u8>>();
        std::ffi::CString::new(bytes).unwrap_or_default()
    }

    /// One more task finished, of those a waitall slot counts.
    pub fn add_finished(self) {
        if let Some(count) = self.data().first() {
            count.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// The slot's `EventData`, for a result a task worker has finished writing.
    #[must_use]
    pub const fn as_ptr(self) -> *const u8 {
        self.0.as_ptr().cast()
    }

    /// Zeroes the slot's head. Upstream zeroes the whole slot; its head is what is
    /// read before a result lands (the task id), and the data is read only up to
    /// the head's length.
    fn clear(self) {
        for word in self.0.iter().take(HEAD / 4) {
            word.store(0, Ordering::Relaxed);
        }
    }

    /// `info.fd`, the id of the task whose result is in the slot.
    fn task_id(self) -> i64 {
        let [first, second, ..] = self.0;
        let [a, b, c, d] = first.load(Ordering::Relaxed).to_ne_bytes();
        let [e, f, g, h] = second.load(Ordering::Relaxed).to_ne_bytes();
        i64::from_ne_bytes([a, b, c, d, e, f, g, h])
    }

    /// The slot's data, which follows its head.
    fn data(self) -> &'a [AtomicU32] {
        self.0.get(HEAD / 4..).unwrap_or_default()
    }

    /// The `int` that heads the data of a slot naming a result file: how many
    /// tasks have finished, which task workers add to.
    fn finish_count(self) -> u32 {
        self.data()
            .first()
            .map_or(0, |count| count.load(Ordering::SeqCst))
    }

    /// Copies `data` over the start of the slot's data; the bytes after it stay.
    fn write_data(self, data: &[u8]) {
        copy_words(self.data(), data);
    }
}

/// Copies `data` over the start of `words`, a word at a time; the bytes after it stay.
fn copy_words(words: &[AtomicU32], data: &[u8]) {
    let (whole, tail) = data.as_chunks::<4>();
    let mut words = words.iter();
    // `whole` leads the zip, so its end takes no word from `words`.
    for (&bytes, word) in whole.iter().zip(words.by_ref()) {
        word.store(u32::from_ne_bytes(bytes), Ordering::Relaxed);
    }
    if let Some(word) = words.next().filter(|_| !tail.is_empty()) {
        let mut bytes = word.load(Ordering::Relaxed).to_ne_bytes();
        for (to, from) in bytes.iter_mut().zip(tail) {
            *to = *from;
        }
        word.store(u32::from_ne_bytes(bytes), Ordering::Relaxed);
    }
}

/// [`Tasks`] with this worker's `task_results` slot, which the `task_sync`s wait on.
pub struct Waiting<'a, H> {
    pub tasks: Tasks<'a, H>,
    result: ResultSlot<'a>,
}

/// `Server::task`'s counting around `dispatch`: one more in flight, counted when
/// sent and taken back when not.
///
/// # Errors
/// The dispatch bailed out; the task stays in flight, as upstream.
pub fn count_task(
    tasking: &AtomicU32,
    count: &AtomicU64,
    dispatch: impl FnOnce() -> Result<bool, Bailed>,
) -> Result<bool, Bailed> {
    tasking.fetch_add(1, Ordering::SeqCst);
    if dispatch()? {
        count.fetch_add(1, Ordering::SeqCst);
        return Ok(true);
    }
    tasking.fetch_sub(1, Ordering::SeqCst);
    Ok(false)
}

impl<'a, H: TaskHost> Tasks<'a, H> {
    /// `Server::task(task, dst_worker_id, blocking)`.
    ///
    /// # Errors
    /// The dispatch bailed out.
    pub fn task(
        &mut self,
        task: Packed<'_>,
        worker_id: &mut c_int,
        blocking: bool,
    ) -> Result<bool, Bailed> {
        let host = &mut self.host;
        count_task(self.tasking, self.count, || {
            trapped(|| host.dispatch(task, worker_id, blocking))
        })
    }

    fn pipe_clean(&mut self) -> Result<(), Bailed> {
        trapped(|| self.host.pipe_clean())
    }

    fn set_timeout(&mut self, timeout: f64) -> Result<(), Bailed> {
        trapped(|| self.host.set_timeout(timeout))
    }

    fn read_notify(&mut self) -> Result<isize, Bailed> {
        trapped(|| self.host.read_notify())
    }

    /// The state of a call that waits for results in `result`, this worker's slot.
    pub fn waiting(self, result: ResultSlot<'a>) -> Waiting<'a, H> {
        Waiting {
            tasks: self,
            result,
        }
    }
}

impl<'a, H: TaskHost> Waiting<'a, H> {
    /// `serv->get_task_result()`: this worker's slot.
    pub const fn result(&self) -> ResultSlot<'a> {
        self.result
    }

    /// `Server::task_sync(task, dst_worker_id, timeout)`: dispatch one task and
    /// wait for its own result in this worker's slot.
    ///
    /// # Errors
    /// The dispatch or a wait bailed out.
    pub fn task_sync(
        &mut self,
        task: Packed<'_>,
        worker_id: &mut c_int,
        timeout: f64,
    ) -> Result<bool, Bailed> {
        let (tasks, result) = (&mut self.tasks, self.result);
        result.clear();
        let task_id = task.head().fd;
        // One trap for the whole wait: the body owns nothing a bailout skips, and
        // a bailout at any step ends the call, as a trap per step did.
        trapped(|| {
            tasks.host.pipe_clean();
            tasks.host.set_timeout(timeout);
            let host = &mut tasks.host;
            let sent = count_task(tasks.tasking, tasks.count, || {
                Ok(host.dispatch(task, worker_id, true))
            });
            if !matches!(sent, Ok(true)) {
                return false;
            }
            while tasks.host.read_notify() > 0 {
                if result.task_id() == task_id {
                    return true;
                }
            }
            false
        })
    }

    /// `Server::task_sync(mtask, timeout)`: the result slot cleared and pointed at
    /// a new temporary file, then every task dispatched and its results unpacked.
    ///
    /// # Errors
    /// A dispatch, a wait, or the `MultiTask` bailed out.
    pub fn task_sync_many(
        &mut self,
        multi: &mut impl MultiTask,
        timeout: f64,
    ) -> Result<bool, Bailed> {
        self.result.clear();
        let mut path = [0; TASK_TMP_PATH_SIZE];
        // Upstream keeps only the path, so the file closes at once.
        if make_tmpfile(&task_tmpfile(), &mut path).is_none() {
            set_last_error(errno());
            return Ok(false);
        }
        let Ok(path) = CStr::from_bytes_until_nul(&path) else {
            return Ok(false);
        };
        self.name_result_file(path);
        let Some(done) = self.sync_many(multi, timeout, path)? else {
            set_last_error(ERROR_TASK_DISPATCH_FAIL);
            return Ok(false);
        };
        Ok(done)
    }

    /// Names the result file in this worker's slot under its lock: no results yet,
    /// then `path` (`swoole_strlcpy`'d).
    fn name_result_file(&mut self, path: &CStr) {
        let path = path.to_bytes_with_nul();
        let path = path.get(..TASK_TMP_PATH_SIZE).unwrap_or(path);
        // A zero count, then the path, cut to fit and still NUL-terminated.
        let mut data = vec![0; size_of::<c_int>()];
        data.extend_from_slice(path.split_last().map_or(path, |(_, name)| name));
        data.push(0);
        self.tasks.host.lock(true);
        // An `EventData`'s data holds the count and a path of `SW_TASK_TMP_PATH_SIZE` bytes.
        self.result.write_data(&data);
        self.tasks.host.lock(false);
    }

    /// Dispatch each task (WAITALL) and wait until every result is in the file at
    /// `path`, already named in this worker's result slot, then unpack each by its
    /// task id: whether the results were read, or `None` when no task was dispatched.
    fn sync_many(
        &mut self,
        multi: &mut impl MultiTask,
        timeout: f64,
        path: &CStr,
    ) -> Result<Option<bool>, Bailed> {
        let (tasks, result) = (&mut self.tasks, self.result);
        tasks.pipe_clean()?;

        let count = multi.count();
        let mut pending = i64::from(count);
        let mut indexes = Vec::with_capacity(usize::from(count));
        let mut event = TaskBuffer::default();
        for index in 0..count {
            let mut task_id = multi.pack(index, &mut event)?;
            let mut failed = task_id < 0;
            if failed {
                log::warning(c"task_sync", format_args!("task pack failed"));
            } else {
                event.add_flags(TASK_WAITALL);
                tasks.tasking.fetch_add(1, Ordering::SeqCst);
                let mut worker_id: c_int = -1;
                if tasks.task(event.packed(), &mut worker_id, true)? {
                    tasks.tasking.fetch_sub(1, Ordering::SeqCst);
                } else {
                    log::warning(c"task_sync", format_args!("failed to dispatch task"));
                    task_id = -1;
                    failed = true;
                }
            }
            if failed {
                multi.fail(index);
                pending -= 1;
            }
            indexes.push((task_id, index));
        }
        if pending == 0 {
            return Ok(None);
        }
        if timeout > 0.0 {
            tasks.set_timeout(timeout)?;
        }
        let started = now();
        while i64::from(result.finish_count()) < pending {
            if tasks.read_notify()? <= 0 || (timeout > 0.0 && now() - started > timeout) {
                break;
            }
        }
        // As the map upstream fills: a later id replaces an earlier one's index.
        indexes.reverse();
        indexes.sort_by_key(|&(task_id, _)| task_id);
        indexes.dedup_by_key(|&mut (task_id, _)| task_id);

        tasks.host.lock(true);
        let mut content = CppString::EMPTY;
        let read = file_get_contents(path, &mut content);
        tasks.host.lock(false);
        let unpacked = if read {
            unpack_all(multi, &indexes, &content)
        } else {
            Ok(())
        };
        content.dispose();
        unpacked?;
        if !read {
            return Ok(Some(false));
        }
        swoole_rs_corebase::sys::unlink(path);
        Ok(Some(true))
    }
}

/// Unpacks each record of the result file `content` whose task id is in
/// `indexes` (sorted by id).
fn unpack_all(
    multi: &mut impl MultiTask,
    indexes: &[(i64, u16)],
    content: &CppString,
) -> Result<(), Bailed> {
    let bytes = content.view();
    let mut offset = 0;
    while let Some(record) = bytes.get(offset..).filter(|record| record.len() >= HEAD) {
        let task_id = i64::from_ne_bytes(record.first_chunk().copied().unwrap_or_default());
        let len = u32::from_ne_bytes(
            record
                .get(HEAD_LEN..)
                .and_then(<[u8]>::first_chunk)
                .copied()
                .unwrap_or_default(),
        );
        if let Ok(found) = indexes.binary_search_by_key(&task_id, |&(id, _)| id)
            && let Some(&(_, index)) = indexes.get(found)
            && let Some(record) = Packed::from_bytes(record)
        {
            multi.unpack(index, record)?;
        }
        offset += HEAD + len as usize;
    }
    Ok(())
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_secs_f64())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_words_keeps_the_tail_in_place() {
        let words: Vec<AtomicU32> = (0..20).map(|_| AtomicU32::new(u32::MAX)).collect();
        let data: Vec<u8> = (0..59).collect();
        copy_words(&words, &data);
        let read: Vec<u8> = words
            .iter()
            .flat_map(|word| word.load(Ordering::Relaxed).to_ne_bytes())
            .collect();
        assert_eq!(read[..59], data[..]);
        assert!(read[59..].iter().all(|&byte| byte == 0xff));
    }

    /// A scripted server: dispatch results, notifications.
    #[derive(Default)]
    struct Script {
        dispatched: Vec<bool>,
        notify: Vec<isize>,
        locks: Vec<bool>,
    }

    impl TaskHost for Script {
        fn dispatch(&mut self, _: Packed<'_>, _: &mut c_int, _: bool) -> bool {
            self.dispatched.remove(0)
        }
        fn pipe_clean(&mut self) {}
        fn set_timeout(&mut self, _: f64) {}
        fn read_notify(&mut self) -> isize {
            if self.notify.is_empty() {
                -1
            } else {
                self.notify.remove(0)
            }
        }
        fn lock(&mut self, on: bool) {
            self.locks.push(on);
        }
    }

    /// Scripted tasks: their pack results, and what was unpacked and failed.
    #[derive(Default)]
    struct Multi {
        packs: Vec<i64>,
        unpacked: Vec<(u16, i64)>,
        failed: Vec<u16>,
    }

    impl MultiTask for Multi {
        fn count(&self) -> u16 {
            u16::try_from(self.packs.len()).unwrap()
        }
        fn pack(&mut self, index: u16, event: &mut TaskBuffer) -> Result<i64, Bailed> {
            let id = self.packs[usize::from(index)];
            event.set_fd(id);
            Ok(id)
        }
        fn unpack(&mut self, index: u16, result: Packed<'_>) -> Result<(), Bailed> {
            let id = result.head().fd;
            self.unpacked.push((index, id));
            Ok(())
        }
        fn fail(&mut self, index: u16) {
            self.failed.push(index);
        }
    }

    fn record(task_id: i64, payload: &[u8]) -> Vec<u8> {
        let mut record = vec![0u8; HEAD];
        record[..8].copy_from_slice(&task_id.to_ne_bytes());
        record[HEAD_LEN..HEAD_LEN + 4]
            .copy_from_slice(&u32::try_from(payload.len()).unwrap().to_ne_bytes());
        record.extend_from_slice(payload);
        record
    }

    fn tasks(script: Script, counters: &(AtomicU32, AtomicU64)) -> Tasks<'_, Script> {
        Tasks {
            host: script,
            tasking: &counters.0,
            count: &counters.1,
        }
    }

    /// `tasks` waiting on `result`, a whole slot.
    fn waiting<'a>(
        script: Script,
        counters: &'a (AtomicU32, AtomicU64),
        result: &'a mut [u64],
    ) -> Waiting<'a, Script> {
        assert_eq!(result.len(), EVENT_SIZE / 8);
        let slot = std::ptr::NonNull::from(result).cast();
        tasks(script, counters).waiting(unsafe { ResultSlot::new(slot) })
    }

    #[test]
    fn a_task_counts_and_rolls_back() {
        let counters = (AtomicU32::new(0), AtomicU64::new(0));
        let script = Script {
            dispatched: vec![true, false],
            ..Script::default()
        };
        let mut tasks = tasks(script, &counters);
        let task = TaskBuffer::default();
        let mut worker_id = -1;
        for expected in [true, false] {
            let sent = tasks.task(task.packed(), &mut worker_id, false);
            assert_eq!(sent.ok(), Some(expected));
        }
        assert_eq!((counters.0.into_inner(), counters.1.into_inner()), (1, 1));
    }

    #[test]
    fn a_sync_task_waits_for_its_own_result() {
        let counters = (AtomicU32::new(0), AtomicU64::new(0));
        let script = Script {
            dispatched: vec![true],
            notify: vec![8, 8],
            ..Script::default()
        };
        let mut result = vec![0u64; EVENT_SIZE / 8];
        let mut tasks = waiting(script, &counters, &mut result);
        let mut task = TaskBuffer::default();
        task.set_fd(7);
        let mut worker_id = -1;
        // Another task's result arrives first; this one never does.
        let done = tasks.task_sync(task.packed(), &mut worker_id, 1.0);
        assert_eq!(done.ok(), Some(false));
        assert_eq!(tasks.tasks.host.notify, []);
    }

    #[test]
    fn many_tasks_unpack_each_result_by_id() {
        swoole_rs_corebase::swr_v1_core_set_max_file_content(1 << 20);
        let mut file = record(11, b"ab");
        file.extend(record(99, b""));
        file.extend(record(12, b"c"));
        let path = std::env::temp_dir().join(format!("swr-task-sync-{}", std::process::id()));
        std::fs::write(&path, file).unwrap();
        let path = std::ffi::CString::new(path.into_os_string().into_encoded_bytes()).unwrap();
        // The finish count (the result's data) already says both finished.
        let mut result = vec![0u64; EVENT_SIZE / 8];
        result[HEAD / 8] = 2;
        let counters = (AtomicU32::new(0), AtomicU64::new(0));
        let script = Script {
            dispatched: vec![true, true, false],
            ..Script::default()
        };
        let mut tasks = waiting(script, &counters, &mut result);
        let mut multi = Multi {
            packs: vec![11, 12, 13, -1],
            ..Multi::default()
        };
        let done = tasks.sync_many(&mut multi, -1.0, &path);
        assert_eq!(done.ok(), Some(Some(true)));
        assert_eq!(multi.unpacked, vec![(0, 11), (1, 12)]);
        assert_eq!(multi.failed, vec![2, 3]);
        assert_eq!(tasks.tasks.host.locks, vec![true, false]);
        assert!(
            std::fs::metadata(path.to_str().unwrap()).is_err(),
            "unlinked"
        );
        // One in flight per dispatched task (its task worker takes it off), and the
        // failed dispatch keeps its extra count, as upstream.
        assert_eq!((counters.0.into_inner(), counters.1.into_inner()), (3, 2));
    }

    #[test]
    fn the_result_file_is_named_under_the_lock() {
        // A slot whose count and path bytes are dirty.
        let mut result = vec![u64::MAX; EVENT_SIZE / 8];
        let counters = (AtomicU32::new(0), AtomicU64::new(0));
        let mut tasks = waiting(Script::default(), &counters, &mut result);
        let path = c"/tmp/swoole.task.abc123";
        tasks.name_result_file(path);
        assert_eq!(tasks.tasks.host.locks, vec![true, false]);
        let data = result
            .iter()
            .flat_map(|word| word.to_ne_bytes())
            .skip(HEAD)
            .collect::<Vec<_>>();
        assert_eq!(data[..4], [0; 4], "no results yet");
        let named = &data[4..4 + path.to_bytes_with_nul().len()];
        assert_eq!(named, path.to_bytes_with_nul());
    }

    #[test]
    fn nothing_dispatched_is_reported() {
        let mut result = vec![0u64; EVENT_SIZE / 8];
        let counters = (AtomicU32::new(0), AtomicU64::new(0));
        let mut tasks = waiting(Script::default(), &counters, &mut result);
        let mut multi = Multi {
            packs: vec![-1],
            ..Multi::default()
        };
        let done = tasks.sync_many(&mut multi, -1.0, c"/x");
        assert_eq!(done.ok(), Some(None));
        assert_eq!(multi.failed, vec![0]);
    }
}
