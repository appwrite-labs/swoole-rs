//! `swoole::Table`: a fixed-size hash table in shared memory, used by forked
//! workers at once. Rust owns the whole table; its two blocks come from
//! `sw_mem_pool()` (the header) and `sw_shm_malloc` (the arena), so both
//! stay shared across `fork()` exactly as upstream's are.
//!
//! The header holds what upstream's `Table` held in shared memory: sizes,
//! counters, the pool mutex and the iterator mutex. Columns and the iterator's
//! cursor are process-private, as upstream's maps and `TableIterator` were: a
//! [`Handle`] owns them beside its header, and a fork copies them. The arena is room for upstream's `size` bucket pointers
//! (unused: a bucket's row is found by its index), `size` bucket rows, then
//! room for upstream's pool header (unused too: the pool's state is in the table's
//! header) and the pool's collision rows. Each row is an 88-byte header
//! followed by the column data, rounded up to 8
//! bytes so every lock word and link is aligned.
//!
//! Row locks are upstream's spin locks, with its recovery from a dead owner
//! (`kill(pid, 0)` fails with `ESRCH`) and from an owner that holds a lock for
//! more than two seconds.
use crate::constants::SW_TABLE_CONFLICT_PROPORTION;
use crate::locks::SharedMutex;
use crate::memory::fixed::{self, Fixed, HEADER, Layout, Slice};
use crate::memory::members::Shm;
use crate::{CCast, CellExt};
use std::cell::Cell;
use std::ffi::c_int;
use std::mem::{MaybeUninit, offset_of};
use std::sync::atomic::{
    AtomicI32, AtomicI64, AtomicPtr, AtomicU8, AtomicU32, AtomicUsize, Ordering,
};
use std::time::Instant;
use std::{ptr, slice};

/// `TableColumn::TYPE_INT`, `TYPE_FLOAT`, `TYPE_STRING`: [`ColumnType`]'s codes.
pub const TYPE_INT: u32 = 1;
pub const TYPE_FLOAT: u32 = 2;
pub const TYPE_STRING: u32 = 3;

/// `TableColumn::Type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColumnType {
    Int,
    Float,
    String,
}

impl ColumnType {
    /// The type a `TYPE_*` code names.
    #[must_use]
    pub const fn from_code(code: u32) -> Option<Self> {
        match code {
            TYPE_INT => Some(Self::Int),
            TYPE_FLOAT => Some(Self::Float),
            TYPE_STRING => Some(Self::String),
            _ => None,
        }
    }
}
/// `SW_TABLE_FLAG_NEW_ROW`, `SW_TABLE_FLAG_CONFLICT`.
pub const NEW_ROW: i32 = 1;
pub const CONFLICT: i32 = 2;
/// `SW_TABLE_KEY_SIZE`: keys keep at most 63 bytes and a NUL.
pub const KEY_SIZE: usize = crate::constants::SW_TABLE_KEY_SIZE as usize;
const MAX_ROWS: u32 = 0x8000_0000;
const FORCE_UNLOCK_MS: u128 = 2000;
const SPINLOCK_LOOP_N: u32 = crate::constants::SW_SPINLOCK_LOOP_N;
const SW_ERROR_MALLOC_FAIL: c_int = swoole_rs_error::ErrorCode::SW_ERROR_MALLOC_FAIL as c_int;

/// The host's hash of a key as the caller passed it, whose row compares its
/// first `compared` bytes: upstream's `hash_func(key, keylen)`, which
/// `swoole_hash_php` reads `compared` bytes of and `Swoole\Table`'s hook the
/// whole PHP string.
pub type Hash = fn(key: &[u8], compared: usize) -> u64;
/// Logs one whole warning line, as `swoole_warning` does.
pub type Warning = fn(message: &[u8]);
/// `swoole_set_last_error`.
pub type Error = fn(code: c_int);

/// A row header, then `item_size` bytes of column data. Other processes read
/// and write it at once, so every field is an atomic: fields other than the lock
/// change only under the bucket's lock, and `active` is also read before it
/// (`del`, the iterator), as upstream does. All-zero bytes are an unlocked,
/// inactive row with no key or link, which is how `create` makes one.
pub struct Row {
    lock: AtomicU32,
    lock_pid: AtomicI32,
    active: AtomicU8,
    key_len: AtomicU8,
    next: AtomicPtr<Row>,
    key: [AtomicU8; KEY_SIZE],
}
const ROW: usize = size_of::<Row>();

/// A row of a created table, read and written in place.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Live(*mut Row);

impl Live {
    /// The row `row` names, or None for null: a row pointer this table handed out.
    fn of(row: *mut Row) -> Option<Self> {
        (!row.is_null()).then_some(Self(row))
    }

    fn row(self) -> &'static Row {
        // SAFETY: a live row; its fields are atomics or cells, so a shared
        // reference holds while other processes change them.
        unsafe { &*self.0 }
    }

    fn lock_word(self) -> &'static AtomicU32 {
        &self.row().lock
    }

    fn owner(self) -> &'static AtomicI32 {
        &self.row().lock_pid
    }

    fn active(self) -> bool {
        self.row().active.load(Ordering::Relaxed) != 0
    }

    fn next(self) -> Option<Live> {
        // Links only point at rows of the same table.
        let next = self.row().next.load(Ordering::Relaxed);
        (!next.is_null()).then_some(Live(next))
    }

    fn set_next(self, next: Option<Live>) {
        let next = next.map_or(ptr::null_mut(), |row| row.0);
        self.row().next.store(next, Ordering::Relaxed);
    }

    fn key_len(self) -> u8 {
        self.row().key_len.load(Ordering::Relaxed)
    }

    /// The key's `key_len` (at most 63) bytes, which change only under the bucket's lock.
    fn key_bytes(self) -> &'static [AtomicU8] {
        let length = usize::from(self.key_len()).min(KEY_SIZE - 1);
        self.row().key.get(..length).unwrap_or_default()
    }

    /// Whether the row's key is `key`.
    fn key_is(self, key: &[u8]) -> bool {
        let own = self.key_bytes();
        own.len() == key.len()
            && own
                .iter()
                .zip(key)
                .all(|(byte, &other)| byte.load(Ordering::Relaxed) == other)
    }

    /// A copy of the row's key.
    fn key(self) -> Vec<u8> {
        let bytes = self.key_bytes().iter();
        bytes.map(|byte| byte.load(Ordering::Relaxed)).collect()
    }

    fn data(self) -> *mut u8 {
        self.0.wrapping_byte_add(ROW).cast()
    }

    /// `TableRow::clear()`: zero the header from `lock_pid` on.
    fn clear(self) {
        self.owner().store(0, Ordering::Relaxed);
        self.set_next(None);
        self.set_key(&[], 0);
    }

    /// `init_row`: a fresh, active row for a key of at most 63 bytes.
    fn claim(self, key: &[u8]) {
        self.set_next(None);
        self.set_key(key, 1);
    }

    /// The key, NUL-padded, its length and `active`.
    fn set_key(self, key: &[u8], active: u8) {
        let row = self.row();
        let key = key.get(..KEY_SIZE - 1).unwrap_or(key);
        for (at, to) in row.key.iter().enumerate() {
            to.store(key.get(at).copied().unwrap_or(0), Ordering::Relaxed);
        }
        let key_len = u8::try_from(key.len()).unwrap_or(u8::MAX);
        row.key_len.store(key_len, Ordering::Relaxed);
        row.active.store(active, Ordering::Relaxed);
    }

    /// Move `from`'s key and data into this row (a deleted bucket row).
    fn take(self, from: Live, item_size: usize) {
        let key_len = from.key_len();
        // The key and its NUL.
        let key = self.row().key.iter().zip(&from.row().key);
        for (to, byte) in key.take(usize::from(key_len) + 1) {
            to.store(byte.load(Ordering::Relaxed), Ordering::Relaxed);
        }
        self.row().key_len.store(key_len, Ordering::Relaxed);
        // SAFETY: two live rows of one table, under the bucket's lock; both hold
        // `item_size` data bytes.
        unsafe { ptr::copy_nonoverlapping(from.data(), self.data(), item_size) };
    }

    fn lock(self, warning: Warning) {
        let word = self.lock_word();
        let acquire = || {
            word.load(Ordering::Relaxed) == 0
                && word
                    .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
        };
        let mut since = None;
        loop {
            if acquire() || multicore() && spin(acquire) {
                break;
            }
            let owner = self.owner().load(Ordering::Relaxed);
            // Signal 0 only checks that `owner` exists.
            if crate::sys::kill(owner, 0)
                .is_err_and(|error| error.raw_os_error() == Some(libc::ESRCH))
            {
                word.store(1, Ordering::Relaxed);
                let message =
                    format!("TableRow::lock(): lock process[{owner}] not exists, force unlock");
                warn(warning, message.as_bytes());
                break;
            }
            match since {
                None => since = Some(Instant::now()),
                Some(start) if start.elapsed().as_millis() > FORCE_UNLOCK_MS => {
                    word.store(1, Ordering::Relaxed);
                    warn(warning, b"TableRow::lock(): timeout, force unlock");
                    break;
                }
                Some(_) => (),
            }
            std::thread::yield_now();
        }
        let pid = crate::sys::getpid();
        self.owner().store(pid, Ordering::Relaxed);
    }

    fn unlock(self) {
        self.lock_word().store(0, Ordering::Release);
    }
}

/// A declared column.
pub struct Column {
    pub kind: ColumnType,
    /// Bytes in a row: 8 for a number, the capacity plus a 4-byte length for a string.
    pub width: u32,
    /// Offset in the row's data.
    pub index: usize,
    name: Box<[u8]>,
}

impl Column {
    #[must_use]
    pub fn name(&self) -> &[u8] {
        &self.name
    }
}

/// The iterator's position and its private copy of the current row.
struct Cursor {
    absolute: usize,
    collision: usize,
    current: Vec<u8>,
}

/// The counters `Swoole\Table::stats()` reports.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct Stats {
    pub conflict_count: usize,
    pub conflict_max_level: u32,
    pub insert_count: i64,
    pub update_count: i64,
    pub delete_count: i64,
}

/// The table header, in `size_of::<Table>()` bytes of shared memory.
///
/// Invariant: while `memory` holds a mapping, it is the arena `create` laid out:
/// bucket row `i` is at `first + i * stride` and `pool` is a fixed pool of
/// collision rows.
pub struct Table {
    mutex: SharedMutex,
    iterator_mutex: SharedMutex,
    row_num: AtomicU32,
    conflict_max_level: AtomicU32,
    conflict_count: AtomicUsize,
    insert_count: AtomicI64,
    delete_count: AtomicI64,
    update_count: AtomicI64,
    size: usize,
    item_size: usize,
    memory_size: usize,
    conflict_proportion: f32,
    created: bool,
    hash: Hash,
    warning: Warning,
    error: Error,
    memory: Shm,
    first: *mut Row,
    stride: usize,
    pool: Option<Fixed>,
}

/// The arena of a table about to be created.
struct Plan {
    /// The arena, rows padded to `stride`.
    physical: usize,
    stride: usize,
    slices: usize,
}

/// Whether to spin before yielding (`SW_CPU_NUM > 1`, `SwooleG.cpu_num`).
fn multicore() -> bool {
    crate::swr_v1_core_cpu_num() > 1
}

/// `SW_SPINLOCK_LOOP_N` rounds of doubling pauses, trying after each.
fn spin(acquire: impl Fn() -> bool) -> bool {
    let mut n = 1;
    while n < SPINLOCK_LOOP_N {
        for _ in 0..n {
            std::hint::spin_loop();
        }
        if acquire() {
            return true;
        }
        n <<= 1;
    }
    false
}

/// Bytes up to the first NUL, as `%s` prints them.
fn c_str(bytes: &[u8]) -> &[u8] {
    bytes.split(|&byte| byte == 0).next().unwrap_or_default()
}

fn warn(warning: Warning, message: &[u8]) {
    warning(message);
}

/// The bytes of a key a row holds: its `uint16_t` length (as upstream's
/// `Table` takes it), then `check_key_length`'s 63 bytes.
fn clamp(key: &[u8]) -> &[u8] {
    let length = usize::from(key.len().c_cast::<u16>());
    key.get(..length.min(KEY_SIZE - 1)).unwrap_or(key)
}

impl Table {
    /// `Table::make`: an empty, uncreated table in `this`, whose locks are made in
    /// place; none when a lock can't be made.
    pub fn init(
        this: &mut MaybeUninit<Table>,
        rows: u32,
        conflict: f32,
        hash: Hash,
        warning: Warning,
        error: Error,
    ) -> Option<&mut Table> {
        let size = if rows >= MAX_ROWS {
            MAX_ROWS
        } else {
            rows.max(64).next_power_of_two()
        };
        let conflict = if conflict > 1.0 {
            1.0
        } else if f64::from(conflict) < SW_TABLE_CONFLICT_PROPORTION {
            SW_TABLE_CONFLICT_PROPORTION.c_cast::<f32>()
        } else {
            conflict
        };
        let table = Table {
            mutex: SharedMutex::new(),
            iterator_mutex: SharedMutex::new(),
            row_num: AtomicU32::new(0),
            conflict_max_level: AtomicU32::new(0),
            conflict_count: AtomicUsize::new(0),
            insert_count: AtomicI64::new(0),
            delete_count: AtomicI64::new(0),
            update_count: AtomicI64::new(0),
            size: size as usize,
            item_size: 0,
            memory_size: 0,
            conflict_proportion: conflict,
            created: false,
            hash,
            warning,
            error,
            memory: Shm::empty(),
            first: ptr::null_mut(),
            stride: 0,
            pool: None,
        };
        let table = this.write(table);
        if table.mutex.init().is_err() {
            return None;
        }
        if table.iterator_mutex.init().is_err() {
            table.mutex.destroy();
            return None;
        }
        Some(table)
    }

    pub fn set_hash(&mut self, hash: Hash) {
        self.hash = hash;
    }

    fn plan(&self) -> Option<Plan> {
        let rows = (self.size.c_cast::<f32>() * (1.0 + self.conflict_proportion)).c_cast::<usize>();
        let slices = rows.checked_sub(self.size)?;
        let row_size = ROW.checked_add(self.item_size)?;
        let stride = row_size.checked_next_multiple_of(8)?;
        let pool = stride
            .checked_add(size_of::<Slice>())?
            .checked_mul(slices)?;
        let physical = stride
            .checked_add(size_of::<*mut Row>())?
            .checked_mul(self.size)?
            .checked_add(pool)?
            .checked_add(HEADER)?;
        if u32::try_from(stride).is_err() || isize::try_from(physical).is_err() {
            return None;
        }
        Some(Plan {
            physical,
            stride,
            slices,
        })
    }

    /// `calc_memory_size()`: the arena's size as upstream counts it.
    #[must_use]
    pub fn memory_total(&self) -> usize {
        self.plan().map_or(0, |plan| plan.physical)
    }

    /// Bytes `create` needs, or 0 if the table is created or too large.
    #[must_use]
    pub fn arena_size(&self) -> usize {
        match self.plan() {
            Some(plan) if !self.created && plan.slices >= 2 => plan.physical,
            _ => 0,
        }
    }

    /// `Table::create`, in `arena`, a mapping of at least `arena_size()` bytes as
    /// shared as the table should be, which the table takes (leaving `arena` null)
    /// until `destroy` hands it back. False, leaving `arena` to the caller, otherwise.
    fn create(&mut self, arena: &mut Shm) -> bool {
        let Some(plan) = self.plan().filter(|plan| plan.slices >= 2) else {
            return false;
        };
        // The mapping's payload, which the table alone reaches from here on: written
        // below once it is known 8-aligned and `plan.physical` bytes.
        let memory = arena.as_ptr().cast::<u8>();
        let fits = arena.payload_len() >= plan.physical;
        if self.created || memory.is_null() || !memory.addr().is_multiple_of(8) || !fits {
            return false;
        }
        // SAFETY: the mapping's payload, `plan.physical` bytes (checked above), which
        // the table alone reaches from here on, until `destroy` hands it back (after
        // which no row operation passes `ready`).
        let bytes: &'static mut [MaybeUninit<u8>] =
            unsafe { slice::from_raw_parts_mut(memory.cast(), plan.physical) };
        // Upstream's bucket pointers come first; a bucket's row is found by its index.
        let rows = bytes.get_mut(self.size * size_of::<*mut Row>()..);
        let Some((rows, pool)) =
            rows.and_then(|rest| rest.split_at_mut_checked(self.size * plan.stride))
        else {
            return false;
        };
        for row in rows.chunks_exact_mut(plan.stride) {
            // Zero bytes are an empty row (see `Row`).
            row.get_mut(..ROW)
                .unwrap_or_default()
                .fill(MaybeUninit::new(0));
        }
        let Ok(width) = u32::try_from(plan.stride) else {
            return false;
        };
        let Some(layout) = Layout::new(0, width, pool.len(), false) else {
            return false;
        };
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "8-aligned: the arena is (checked above), and so is the stride"
        )]
        let first = rows.as_mut_ptr().cast::<Row>();
        let Some(pool) = fixed::construct(pool, layout) else {
            return false;
        };
        self.first = first;
        self.stride = plan.stride;
        self.pool = Some(pool);
        self.memory_size = plan.physical;
        self.memory = std::mem::take(arena);
        self.created = true;
        true
    }

    #[must_use]
    pub fn is_created(&self) -> bool {
        self.created
    }

    /// `ready()`: created and not destroyed. Every row operation checks it.
    #[must_use]
    pub fn ready(&self) -> bool {
        !self.memory.is_null()
    }

    #[must_use]
    pub fn size(&self) -> usize {
        self.size
    }

    #[must_use]
    pub fn conflict_proportion(&self) -> f32 {
        self.conflict_proportion
    }

    /// `get_memory_size()`: the allocated table arena's bytes.
    #[must_use]
    pub fn memory_size(&self) -> usize {
        self.memory_size
    }

    /// Active rows.
    #[must_use]
    pub fn count(&self) -> usize {
        self.row_num.load(Ordering::Relaxed) as usize
    }

    #[must_use]
    pub fn stats(&self) -> Stats {
        Stats {
            conflict_count: self.conflict_count.load(Ordering::Relaxed),
            conflict_max_level: self.conflict_max_level.load(Ordering::Relaxed),
            insert_count: self.insert_count.load(Ordering::Relaxed),
            update_count: self.update_count.load(Ordering::Relaxed),
            delete_count: self.delete_count.load(Ordering::Relaxed),
        }
    }

    fn pool(&self) -> Option<&Fixed> {
        self.pool.as_ref()
    }

    /// Spare collision rows (0 before `create`).
    #[must_use]
    pub fn available_slices(&self) -> u32 {
        let Some(pool) = self.pool() else { return 0 };
        _ = self.mutex.lock();
        let (spare, _) = pool.slices();
        _ = self.mutex.unlock();
        spare
    }

    /// All collision rows (0 before `create`).
    #[must_use]
    pub fn total_slices(&self) -> u32 {
        self.pool().map_or(0, |pool| pool.slices().1)
    }

    /// A created table's bucket row (see the invariant); callers keep `index < size`.
    fn bucket_row(&self, index: usize) -> Live {
        Live(self.first.wrapping_byte_add(index * self.stride))
    }

    /// `get_by_index`: bucket `index`'s row if it is active.
    #[must_use]
    pub fn by_index(&self, index: usize) -> *mut Row {
        if !self.ready() || index >= self.size {
            return ptr::null_mut();
        }
        let row = self.bucket_row(index);
        if row.active() { row.0 } else { ptr::null_mut() }
    }

    /// The bucket `key` hashes to, and the bytes its row compares ([`clamp`]).
    fn bucket<'k>(&self, key: &'k [u8]) -> (Live, &'k [u8]) {
        let compared = clamp(key);
        let hash = (self.hash)(key, compared.len());
        // Masked below the row count, a usize.
        let row = self.bucket_row((hash & (self.size as u64 - 1)).c_cast::<usize>());
        (row, compared)
    }

    fn claim(&self, row: Live, key: &[u8]) {
        row.claim(key);
        self.row_num.fetch_add(1, Ordering::Relaxed);
    }

    /// `Table::set`: the row for `key`, made if it is new, with `lock` set to
    /// the bucket's row, which stays locked even when no row could be made.
    /// `flags` gets `NEW_ROW` and `CONFLICT` unless that happens.
    pub fn set(&self, key: &[u8], lock: &mut *mut Row, flags: Option<&mut i32>) -> *mut Row {
        if !self.ready() {
            return ptr::null_mut();
        }
        let (root, key) = self.bucket(key);
        *lock = root.0;
        root.lock(self.warning);
        let mut found = 0;
        let mut row = root;
        if root.active() {
            let mut level = 1;
            while !row.key_is(key) {
                if let Some(next) = row.next() {
                    row = next;
                    found |= CONFLICT;
                    level += 1;
                    continue;
                }
                self.conflict_count.fetch_add(1, Ordering::Relaxed);
                self.conflict_max_level.fetch_max(level, Ordering::Relaxed);
                let Some(fresh) = self.alloc_row() else {
                    (self.error)(SW_ERROR_MALLOC_FAIL);
                    return ptr::null_mut();
                };
                self.claim(fresh, key);
                found |= NEW_ROW;
                row.set_next(Some(fresh));
                row = fresh;
                break;
            }
        } else {
            self.claim(root, key);
            found |= NEW_ROW;
        }
        if let Some(flags) = flags {
            *flags = found;
        }
        let counter = if found & NEW_ROW == 0 {
            &self.update_count
        } else {
            &self.insert_count
        };
        counter.fetch_add(1, Ordering::Relaxed);
        row.0
    }

    /// `Table::get`: the active row for `key` or null, with `lock` set to the
    /// bucket's row, which is locked either way.
    pub fn get(&self, key: &[u8], lock: &mut *mut Row) -> *mut Row {
        if !self.ready() {
            return ptr::null_mut();
        }
        let (mut row, key) = self.bucket(key);
        *lock = row.0;
        row.lock(self.warning);
        while !row.key_is(key) {
            match row.next() {
                Some(next) => row = next,
                None => return ptr::null_mut(),
            }
        }
        if row.active() { row.0 } else { ptr::null_mut() }
    }

    /// `Table::exists`.
    #[must_use]
    pub fn exists(&self, key: &[u8]) -> bool {
        let mut lock = ptr::null_mut();
        let row = self.get(key, &mut lock);
        if !lock.is_null() {
            Live(lock).unlock();
        }
        !row.is_null()
    }

    /// `Table::del`. Deleting a bucket's own row moves its first collision row in.
    pub fn del(&self, key: &[u8]) -> bool {
        if !self.ready() {
            return false;
        }
        let (root, key) = self.bucket(key);
        if !root.active() {
            return false;
        }
        root.lock(self.warning);
        let deleted = self.unlink(root, key);
        if deleted {
            self.delete_count.fetch_add(1, Ordering::Relaxed);
            self.row_num.fetch_sub(1, Ordering::Relaxed);
        }
        root.unlock();
        deleted
    }

    /// Delete `key` from a locked bucket.
    fn unlink(&self, root: Live, key: &[u8]) -> bool {
        let Some(mut next) = root.next() else {
            if !root.key_is(key) {
                return false;
            }
            root.clear();
            return true;
        };
        if root.key_is(key) {
            root.set_next(next.next());
            root.take(next, self.item_size);
        } else {
            let mut previous = root;
            while !next.key_is(key) {
                previous = next;
                let Some(after) = next.next() else {
                    return false;
                };
                next = after;
            }
            previous.set_next(next.next());
        }
        self.free_row(next);
        true
    }

    fn alloc_row(&self) -> Option<Live> {
        _ = self.mutex.lock();
        // Under the table's mutex.
        let row = self
            .pool()
            .map_or(ptr::null_mut(), fixed::alloc)
            .cast::<Row>();
        _ = self.mutex.unlock();
        Live::of(row)
    }

    /// Return a collision row no chain links any more.
    fn free_row(&self, row: Live) {
        _ = self.mutex.lock();
        row.clear();
        // A row this pool allocated, under the table's mutex.
        if let Some(pool) = self.pool() {
            fixed::free(pool, row.0.cast());
        }
        _ = self.mutex.unlock();
    }

    /// Copy `row`'s key and column data (zeros for `None`) to the cursor's row,
    /// which holds `stride >= row_size` bytes, where [`Current`] reads them.
    fn copy(cursor: &mut Cursor, row: Option<&Locked<'_>>, row_size: usize) {
        let Some(current) = cursor.current.get_mut(..row_size) else {
            return;
        };
        current.fill(0);
        let Some((row, data)) = row.and_then(|locked| locked.row.as_ref()) else {
            return;
        };
        if let Some(length) = current.get_mut(offset_of!(Row, key_len)) {
            *length = row.key_len();
        }
        let key = current.iter_mut().skip(offset_of!(Row, key));
        for (to, byte) in key.zip(row.key_bytes()) {
            *to = byte.load(Ordering::Relaxed);
        }
        let columns = current.iter_mut().skip(ROW);
        for (to, &byte) in columns.zip(data.iter()) {
            *to = byte;
        }
    }

    /// `Table::forward` of `cursor`: copies the next row to it, or an empty
    /// header past the last.
    fn forward(&self, cursor: &mut Cursor, row_size: usize) {
        while self.ready() && cursor.absolute < self.size {
            let bucket = self.bucket_row(cursor.absolute);
            if !bucket.active() {
                cursor.absolute += 1;
                continue;
            }
            bucket.lock(self.warning);
            let found = Self::visit(cursor, bucket);
            // Unlocked when the guard drops.
            let locked = self.locked(Some(bucket), found, 0);
            if locked.as_ref().is_some_and(Locked::found) {
                Self::copy(cursor, locked.as_ref(), row_size);
                return;
            }
            drop(locked);
            cursor.absolute += 1;
        }
        Self::copy(cursor, None, ROW);
    }

    /// The row of a locked bucket the cursor visits next, or `None` once it
    /// has visited them all.
    fn visit(cursor: &mut Cursor, bucket: Live) -> Option<Live> {
        if bucket.next().is_none() {
            cursor.absolute += 1;
            return Some(bucket);
        }
        let mut row = bucket;
        for _ in 0..cursor.collision {
            let Some(next) = row.next() else {
                cursor.collision = 0;
                return None;
            };
            row = next;
        }
        cursor.collision += 1;
        Some(row)
    }

    /// [`Table::set`] as a guard: None before `create`; a guard without a row
    /// when no row could be made.
    #[must_use]
    pub fn lock_set(&self, key: &[u8]) -> Option<Locked<'_>> {
        let (mut lock, mut flags) = (ptr::null_mut(), 0);
        let row = self.set(key, &mut lock, Some(&mut flags));
        self.locked(Live::of(lock), Live::of(row), flags)
    }

    /// [`Table::get`] as a guard: None before `create`; a guard without a row
    /// when `key` has none.
    #[must_use]
    pub fn lock_get(&self, key: &[u8]) -> Option<Locked<'_>> {
        let mut lock = ptr::null_mut();
        let row = self.get(key, &mut lock);
        self.locked(Live::of(lock), Live::of(row), 0)
    }

    /// A guard over `lock`, the bucket `get` or `set` locked, and `row`, a row of
    /// this table under it.
    fn locked(&self, lock: Option<Live>, row: Option<Live>, flags: i32) -> Option<Locked<'_>> {
        let row = row.map(|row| {
            // SAFETY: a row of this table holds `item_size` column bytes after its
            // header. The bucket's lock, which the guard holds until it drops, keeps
            // every other user out of them.
            let data = unsafe { slice::from_raw_parts_mut(row.data(), self.item_size) };
            (row, data)
        });
        lock.map(|bucket| Locked {
            table: self,
            bucket,
            row,
            flags,
        })
    }
}

/// A table as this process uses it: the shared header and what upstream's header
/// pointed at in each process's own heap, its columns and its iterator. A fork
/// copies the handle with the rest of the heap, so each process reads its own.
pub struct Handle {
    table: &'static mut Table,
    columns: Vec<Column>,
    /// The iterator, from `create` until `destroy`.
    cursor: Cell<Option<Cursor>>,
}

impl Handle {
    /// The handle of a table [`Table::init`] made, with no columns yet.
    #[must_use]
    pub fn new(table: &'static mut Table) -> Self {
        Self {
            table,
            columns: Vec::new(),
            cursor: Cell::new(None),
        }
    }

    pub fn set_hash(&mut self, hash: Hash) {
        self.table.set_hash(hash);
    }

    /// `Table::add_column`. Refused once the table is created: its rows have no
    /// room for another column.
    pub fn add_column(&mut self, name: &[u8], kind: u32, size: usize) -> bool {
        let table = &mut *self.table;
        let Some(kind) = ColumnType::from_code(kind) else {
            warn(table.warning, b"Table::add_column(): unknown column type");
            return false;
        };
        if table.created {
            return false;
        }
        if self.columns.iter().any(|column| column.name() == name) {
            let message = [
                b"Table::add_column(): column[".as_slice(),
                c_str(name),
                b"] already exists",
            ]
            .concat();
            warn(table.warning, &message);
            return false;
        }
        let width = if kind == ColumnType::String {
            let Some(width) = u32::try_from(size).ok().and_then(|n| n.checked_add(4)) else {
                let message = [
                    b"Table::add_column(): column[".as_slice(),
                    c_str(name),
                    b"] size is too large",
                ]
                .concat();
                warn(table.warning, &message);
                return false;
            };
            width
        } else {
            8
        };
        let Some(item_size) = table.item_size.checked_add(width as usize) else {
            warn(
                table.warning,
                b"Table::add_column(): table row size overflow",
            );
            return false;
        };
        self.columns.push(Column {
            kind,
            width,
            index: table.item_size,
            name: name.into(),
        });
        table.item_size = item_size;
        true
    }

    /// `Table::get_column`: the column declared with this name.
    #[must_use]
    pub fn column(&self, name: &[u8]) -> Option<&Column> {
        self.columns.iter().find(|column| column.name() == name)
    }

    /// Every column in declaration order.
    pub fn columns(&self) -> impl ExactSizeIterator<Item = &Column> {
        self.columns.iter()
    }

    /// `get_column_size()`: distinct names.
    #[must_use]
    pub fn column_size(&self) -> usize {
        self.columns.len()
    }

    /// `Table::create` in `arena` (see [`Table`]'s), with this process's iterator.
    pub fn create(&mut self, arena: &mut Shm) -> bool {
        if !self.table.create(arena) {
            return false;
        }
        self.cursor.set(Some(Cursor {
            absolute: 0,
            collision: 0,
            current: vec![0; self.table.stride],
        }));
        true
    }

    /// `Table::destroy`: the iterator and the locks go, and the arena comes back
    /// for the caller to free. The header is shared: every process sees `ready()`
    /// turn false, as upstream's do, and makes no row operation from then on.
    pub fn destroy(&mut self) -> Shm {
        self.cursor.set(None);
        let table = &mut *self.table;
        table.iterator_mutex.destroy();
        table.mutex.destroy();
        table.pool = None;
        std::mem::take(&mut table.memory)
    }

    /// `step` over this process's iterator under the iterator mutex, with the
    /// table's row size: None before `create` or after `destroy`. The iterator is
    /// taken out while `step` runs, which calls nothing that comes back here.
    fn cursor<T>(&self, step: impl FnOnce(&mut Cursor, usize) -> T) -> Option<T> {
        self.cursor.with_taken(|slot| {
            let cursor = slot.as_mut()?;
            let table = &*self.table;
            _ = table.iterator_mutex.lock();
            let result = step(cursor, ROW + table.item_size);
            _ = table.iterator_mutex.unlock();
            Some(result)
        })
    }

    /// `Table::rewind`.
    pub fn rewind(&self) {
        self.cursor(|cursor, row_size| {
            cursor.absolute = 0;
            cursor.collision = 0;
            Table::copy(cursor, None, row_size);
        });
    }

    /// `Table::forward`: copies the next row to `current_row()`, or an empty
    /// header past the last.
    pub fn forward(&self) {
        self.cursor(|cursor, row_size| self.table.forward(cursor, row_size));
    }

    /// A copy of the iterator's current row: None before `create`.
    #[must_use]
    pub fn current_row(&self) -> Option<Current> {
        self.cursor(|cursor, row_size| Current {
            row: cursor.current.clone(),
            size: row_size - ROW,
        })
    }
}

impl std::ops::Deref for Handle {
    type Target = Table;

    fn deref(&self) -> &Table {
        self.table
    }
}

/// A column's bytes in a row's column data: a number's 8, or a string's length
/// word and capacity. A column that does not fit (another table's) has none, so
/// it reads as zero or empty and takes no writes.
fn field<'d>(data: &'d [u8], column: &Column) -> Option<&'d [u8]> {
    let end = column.index.checked_add(column.width as usize)?;
    data.get(column.index..end).filter(|_| column.width >= 8)
}

fn field_mut<'d>(data: &'d mut [u8], column: &Column) -> Option<&'d mut [u8]> {
    let end = column.index.checked_add(column.width as usize)?;
    data.get_mut(column.index..end)
        .filter(|_| column.width >= 8)
}

/// A number column's 8 bytes.
fn number(data: &[u8], column: &Column) -> u64 {
    field(data, column)
        .and_then(<[u8]>::first_chunk)
        .map_or(0, |bytes| u64::from_ne_bytes(*bytes))
}

/// A string column's bytes: as many as its length word counts, at most its capacity.
fn bytes<'d>(data: &'d [u8], column: &Column) -> &'d [u8] {
    let Some((length, text)) = field(data, column).and_then(<[u8]>::split_first_chunk) else {
        return &[];
    };
    let length = u32::from_ne_bytes(*length) as usize;
    text.get(..length).unwrap_or(text)
}

/// `TableRow::set_value()`: a number column takes `value`'s 8 bytes; a string
/// column `value`, cut to its capacity with a warning naming `row`'s key.
fn set(data: &mut [u8], column: &Column, value: &[u8], row: Live, warning: Warning) {
    let Some(at) = field_mut(data, column) else {
        return;
    };
    if column.kind != ColumnType::String {
        if let (Some(to), Some(number)) = (at.first_chunk_mut::<8>(), value.first_chunk::<8>()) {
            *to = *number;
        }
        return;
    }
    let Some((length, text)) = at.split_first_chunk_mut::<4>() else {
        return;
    };
    if value.len() > text.len() {
        let mut message = b"TableRow::set_value(): [key=".to_vec();
        message.extend_from_slice(c_str(&row.key()));
        message.extend_from_slice(b",field=");
        message.extend_from_slice(c_str(column.name()));
        message.extend_from_slice(b"]string value is too long");
        warn(warning, &message);
    }
    let value = value.get(..text.len()).unwrap_or(value);
    *length = value.len().c_cast::<u32>().to_ne_bytes();
    if let Some(to) = text.get_mut(..value.len()) {
        to.copy_from_slice(value);
    }
}

/// `TableColumn::clear()`: an empty string or a zero number.
fn clear(data: &mut [u8], column: &Column) {
    let width = if column.kind == ColumnType::String {
        4
    } else {
        8
    };
    if let Some(at) = field_mut(data, column).and_then(|at| at.get_mut(..width)) {
        at.fill(0);
    }
}

/// A bucket this process locked, with the key's row in it if there is one
/// (`Table::set`'s or `Table::get`'s) and that row's column data, which the lock
/// keeps to this guard; unlocked on drop.
pub struct Locked<'a> {
    table: &'a Table,
    bucket: Live,
    row: Option<(Live, &'a mut [u8])>,
    flags: i32,
}

impl Locked<'_> {
    /// Whether the guard holds the key's row.
    #[must_use]
    pub fn found(&self) -> bool {
        self.row.is_some()
    }

    /// `SW_TABLE_FLAG_NEW_ROW`: `lock_set` made the row.
    #[must_use]
    pub fn is_new(&self) -> bool {
        self.flags & NEW_ROW != 0
    }

    fn data(&self) -> &[u8] {
        self.row.as_ref().map_or(&[], |(_, data)| &**data)
    }

    /// A number column's 8 bytes (0 without a row).
    #[must_use]
    pub fn number(&self, column: &Column) -> u64 {
        number(self.data(), column)
    }

    /// A string column's bytes (empty without a row).
    #[must_use]
    pub fn bytes(&self, column: &Column) -> &[u8] {
        bytes(self.data(), column)
    }

    /// `TableRow::set_value` of a number column's 8 bytes.
    pub fn set_number(&mut self, column: &Column, value: u64) {
        self.set(column, &value.to_ne_bytes());
    }

    /// `TableRow::set_value` of a string column: cut to it with a warning.
    /// Nothing for a number column.
    pub fn set_bytes(&mut self, column: &Column, value: &[u8]) {
        if column.kind == ColumnType::String {
            self.set(column, value);
        }
    }

    fn set(&mut self, column: &Column, value: &[u8]) {
        if let Some((row, data)) = &mut self.row {
            set(data, column, value, *row, self.table.warning);
        }
    }

    /// `TableColumn::clear()`: an empty string or a zero number.
    pub fn clear(&mut self, column: &Column) {
        if let Some((_, data)) = &mut self.row {
            clear(data, column);
        }
    }

    /// `Table::clear_row`: every column of the table's (its [`Handle::columns`])
    /// empty or zero.
    pub fn clear_row<'c>(&mut self, columns: impl Iterator<Item = &'c Column>) {
        for column in columns {
            self.clear(column);
        }
    }
}

impl Drop for Locked<'_> {
    fn drop(&mut self) {
        self.bucket.unlock();
    }
}

/// A copy of the iterator's current row: an empty key past the last.
pub struct Current {
    row: Vec<u8>,
    size: usize,
}

impl Current {
    fn data(&self) -> &[u8] {
        self.row.get(ROW..ROW + self.size).unwrap_or_default()
    }

    /// The row's key.
    #[must_use]
    pub fn key(&self) -> &[u8] {
        let length = self.row.get(offset_of!(Row, key_len)).copied().unwrap_or(0);
        let start = offset_of!(Row, key);
        let end = start + usize::from(length).min(KEY_SIZE - 1);
        self.row.get(start..end).unwrap_or_default()
    }

    /// A number column's 8 bytes.
    #[must_use]
    pub fn number(&self, column: &Column) -> u64 {
        number(self.data(), column)
    }

    /// A string column's bytes.
    #[must_use]
    pub fn bytes(&self, column: &Column) -> &[u8] {
        bytes(self.data(), column)
    }
}

#[cfg(test)]
#[path = "../../../../tests/modules/memory/table.rs"]
mod tests;
