//! `swoole::Table`: a fixed-size hash table in shared memory, used by forked
//! workers at once. Rust owns the whole table; its two blocks come from
//! `sw_mem_pool()` (the header) and `sw_shm_malloc` (the arena), so both
//! stay shared across `fork()` exactly as upstream's are.
//!
//! The header holds what upstream's `Table` held in shared memory: sizes,
//! counters, the pool mutex and the iterator mutex. Columns and the iterator's
//! cursor are process-private heap objects, as upstream's maps and
//! `TableIterator` were. The arena is `size` bucket pointers, `size` bucket
//! rows, then a fixed pool of collision rows. Each row is an 88-byte header
//! followed by the column data, rounded up to 8
//! bytes so every lock word and link is aligned.
//!
//! Row locks are upstream's spin locks, with its recovery from a dead owner
//! (`kill(pid, 0)` fails with `ESRCH`) and from an owner that holds a lock for
//! more than two seconds.
use crate::fixed::{self, Fixed, Layout, Slice};
use std::cell::UnsafeCell;
use std::ffi::c_int;
use std::mem::{MaybeUninit, size_of};
use std::sync::atomic::{
    AtomicI32, AtomicI64, AtomicPtr, AtomicU8, AtomicU32, AtomicUsize, Ordering,
};
use std::sync::{Mutex, PoisonError};
use std::time::Instant;
use std::{ptr, slice};
use swoole_rs_locks::SharedMutex;

/// `TableColumn::TYPE_INT`, `TYPE_FLOAT`, `TYPE_STRING`.
pub const TYPE_INT: u32 = 1;
pub const TYPE_FLOAT: u32 = 2;
pub const TYPE_STRING: u32 = 3;
/// `SW_TABLE_FLAG_NEW_ROW`, `SW_TABLE_FLAG_CONFLICT`.
pub const NEW_ROW: i32 = 1;
pub const CONFLICT: i32 = 2;
/// `SW_TABLE_KEY_SIZE`: keys keep at most 63 bytes and a NUL.
pub const KEY_SIZE: usize = 64;
const MAX_ROWS: u32 = 0x8000_0000;
const CONFLICT_PROPORTION: f32 = 0.2;
const FORCE_UNLOCK_MS: u128 = 2000;
const SPINLOCK_LOOP_N: u32 = 1024;
const SW_ERROR_MALLOC_FAIL: c_int = 501;

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
/// and write it at once, so every field is an atomic or a cell: fields other
/// than the lock change only under the bucket's lock, and `active` is also read
/// before it (`del`, the iterator), as upstream does.
#[repr(C)]
pub struct Row {
    lock: AtomicU32,
    lock_pid: AtomicI32,
    active: AtomicU8,
    key_len: AtomicU8,
    next: AtomicPtr<Row>,
    key: UnsafeCell<[u8; KEY_SIZE]>,
}
const ROW: usize = size_of::<Row>();

impl Row {
    /// An unlocked, inactive row with no key or link.
    const fn empty() -> Self {
        Self {
            lock: AtomicU32::new(0),
            lock_pid: AtomicI32::new(0),
            active: AtomicU8::new(0),
            key_len: AtomicU8::new(0),
            next: AtomicPtr::new(ptr::null_mut()),
            key: UnsafeCell::new([0; KEY_SIZE]),
        }
    }
}

/// A row of a created table, or the iterator's copy of one, read and written
/// in place.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Live(*mut Row);

impl Live {
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

    fn key(self) -> &'static [u8] {
        let length = usize::from(self.key_len()).min(KEY_SIZE - 1);
        // SAFETY: a live row holds `key_len` (at most 63) key bytes, which change
        // only under the bucket's lock.
        unsafe { slice::from_raw_parts(self.row().key.get().cast::<u8>(), length) }
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
        let mut bytes = [0; KEY_SIZE];
        let key = key.get(..KEY_SIZE - 1).unwrap_or(key);
        for (to, from) in bytes.iter_mut().zip(key) {
            *to = *from;
        }
        // SAFETY: a live row's key, under the bucket's lock.
        unsafe { row.key.get().write(bytes) };
        #[expect(clippy::cast_possible_truncation, reason = "at most 63 bytes")]
        row.key_len.store(key.len() as u8, Ordering::Relaxed);
        row.active.store(active, Ordering::Relaxed);
    }

    /// Move `from`'s key and data into this row (a deleted bucket row).
    fn take(self, from: Live, item_size: usize) {
        let key_len = from.key_len();
        let (key, from_key) = (
            self.row().key.get().cast::<u8>(),
            from.row().key.get().cast(),
        );
        // SAFETY: two live rows of one table, under the bucket's lock; the key and its NUL.
        unsafe { ptr::copy_nonoverlapping(from_key, key, usize::from(key_len) + 1) };
        self.row().key_len.store(key_len, Ordering::Relaxed);
        // SAFETY: as above; both rows hold `item_size` data bytes.
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
            if swoole_rs_corebase::sys::kill(owner, 0) < 0
                && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
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
        let pid = swoole_rs_corebase::sys::getpid();
        self.owner().store(pid, Ordering::Relaxed);
    }

    fn unlock(self) {
        self.lock_word().store(0, Ordering::Release);
    }
}

/// A declared column.
#[repr(C)]
pub struct Column {
    pub kind: u32,
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

/// Every declaration in order, repeats included (upstream's `column_list`),
/// and where the first declaration of each name sits (upstream's `column_map`).
/// Tables have a handful of columns, so a scan beats hashing the name.
#[derive(Default)]
struct Columns {
    #[expect(
        clippy::vec_box,
        reason = "callers keep column pointers across later declarations"
    )]
    list: Vec<Box<Column>>,
    first: Vec<usize>,
}

/// The iterator's position and its private copy of the current row.
struct Cursor {
    absolute: usize,
    collision: usize,
    current: Vec<u64>,
}

/// The process-private parts of each table this process made and hasn't destroyed,
/// as this process allocated them (header, columns, cursor): a fork copies them, and a
/// table created after one has another process's cursor. The headers are in the shared
/// pool, which leak checkers don't scan and `swoole_clean` frees: `release` frees these.
static PRIVATE: Mutex<Vec<[usize; 3]>> = Mutex::new(Vec::new());

fn private<T>(f: impl FnOnce(&mut Vec<[usize; 3]>) -> T) -> T {
    f(&mut PRIVATE.lock().unwrap_or_else(PoisonError::into_inner))
}

/// Frees the process-private parts of every table this process hasn't destroyed, before
/// the pool takes their headers: a step of MSHUTDOWN.
///
/// # Safety
/// No table is used afterwards.
pub unsafe extern "C" fn release() {
    for [_, columns, cursor] in private(std::mem::take) {
        // SAFETY: `init` made the columns and this process hasn't destroyed the table.
        drop(unsafe { Box::from_raw(ptr::with_exposed_provenance_mut::<Columns>(columns)) });
        if cursor != 0 {
            // SAFETY: as above, from `create`.
            drop(unsafe { Box::from_raw(ptr::with_exposed_provenance_mut::<Cursor>(cursor)) });
        }
    }
}

/// The counters `Swoole\Table::stats()` reports.
#[repr(C)]
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
/// Invariant: once `created`, `memory` is the arena `create` laid out: `rows`
/// holds `size` pointers to bucket rows, `pool` is a fixed pool of collision
/// rows, and `cursor` is this process's cursor.
#[repr(C)]
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
    columns: *mut Columns,
    cursor: *mut Cursor,
    memory: *mut u8,
    rows: *mut *mut Row,
    pool: *mut Fixed,
}

/// The arena of a table about to be created.
struct Plan {
    /// `calc_memory_size()`: upstream's byte count, rows unpadded.
    logical: usize,
    /// The arena, rows padded to `stride`.
    physical: usize,
    row_size: usize,
    stride: usize,
    slices: usize,
}

/// Whether to spin before yielding (`SW_CPU_NUM > 1`, `SwooleG.cpu_num`).
fn multicore() -> bool {
    swoole_rs_corebase::swr_v1_core_cpu_num() > 1
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
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream's Table takes uint16_t key lengths"
    )]
    let length = usize::from(key.len() as u16);
    key.get(..length.min(KEY_SIZE - 1)).unwrap_or(key)
}

impl Table {
    /// `Table::make`: an empty, uncreated table in `this`, whose locks are made in
    /// place.
    pub fn init(
        this: &mut MaybeUninit<Table>,
        rows: u32,
        conflict: f32,
        hash: Hash,
        warning: Warning,
        error: Error,
    ) -> bool {
        let size = if rows >= MAX_ROWS {
            MAX_ROWS
        } else {
            rows.max(64).next_power_of_two()
        };
        let conflict = if conflict > 1.0 {
            1.0
        } else if f64::from(conflict) < 0.2 {
            CONFLICT_PROPORTION
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
            columns: ptr::null_mut(),
            cursor: ptr::null_mut(),
            memory: ptr::null_mut(),
            rows: ptr::null_mut(),
            pool: ptr::null_mut(),
        };
        let table = this.write(table);
        if table.mutex.init() != 0 {
            return false;
        }
        if table.iterator_mutex.init() != 0 {
            table.mutex.destroy();
            return false;
        }
        table.columns = Box::into_raw(Box::default());
        let columns = table.columns.expose_provenance();
        let this = ptr::from_mut(table).addr();
        private(|tables| tables.push([this, columns, 0]));
        true
    }

    fn column_set(&self) -> &Columns {
        // SAFETY: `init` made the columns; only `destroy` frees them.
        unsafe { &*self.columns }
    }

    pub fn set_hash(&mut self, hash: Hash) {
        self.hash = hash;
    }

    /// `Table::add_column`. Refused once the table is created: its rows have no
    /// room for another column.
    pub fn add_column(&mut self, name: &[u8], kind: u32, size: usize) -> bool {
        if !(TYPE_INT..=TYPE_STRING).contains(&kind) {
            warn(self.warning, b"Table::add_column(): unknown column type");
            return false;
        }
        if self.created {
            return false;
        }
        #[expect(clippy::cast_possible_truncation, reason = "upstream's uint32_t size")]
        let width = if kind == TYPE_STRING {
            size.wrapping_add(4) as u32
        } else {
            8
        };
        let column = Column {
            kind,
            width,
            index: self.item_size,
            name: name.into(),
        };
        self.item_size = self.item_size.wrapping_add(width as usize);
        // SAFETY: as in `column_set`; `&mut self` excludes other users.
        let columns = unsafe { &mut *self.columns };
        if columns.list.iter().all(|column| *column.name != *name) {
            columns.first.push(columns.list.len());
        }
        columns.list.push(Box::new(column));
        true
    }

    /// `Table::get_column`: the first column declared with this name.
    #[must_use]
    pub fn column(&self, name: &[u8]) -> Option<&Column> {
        let columns = self.column_set();
        columns
            .first
            .iter()
            .filter_map(|&at| columns.list.get(at))
            .find(|column| *column.name == *name)
            .map(|column| &**column)
    }

    /// Every declaration, repeats included (`column_list`).
    pub fn columns(&self) -> impl ExactSizeIterator<Item = &Column> {
        self.column_set().list.iter().map(|column| &**column)
    }

    /// `get_column_size()`: distinct names.
    #[must_use]
    pub fn column_size(&self) -> usize {
        self.column_set().first.len()
    }

    fn plan(&self) -> Option<Plan> {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss,
            reason = "upstream sizes rows in float: size_t * (1 + float)"
        )]
        let rows = (self.size as f32 * (1.0 + self.conflict_proportion)) as usize;
        let slices = rows.checked_sub(self.size)?;
        let row_size = ROW.checked_add(self.item_size)?;
        let stride = row_size.checked_next_multiple_of(8)?;
        let bytes = |row: usize| {
            let pool = row.checked_add(size_of::<Slice>())?.checked_mul(slices)?;
            row.checked_add(size_of::<*mut Row>())?
                .checked_mul(self.size)?
                .checked_add(pool)?
                .checked_add(size_of::<Fixed>())
        };
        Some(Plan {
            logical: bytes(row_size)?,
            physical: bytes(stride).filter(|&n| isize::try_from(n).is_ok())?,
            row_size,
            stride,
            slices,
        })
    }

    /// `calc_memory_size()`: the arena's size as upstream counts it.
    #[must_use]
    pub fn memory_total(&self) -> usize {
        self.plan().map_or(0, |plan| plan.logical)
    }

    /// Bytes `create` needs, or 0 if the table is created or too large.
    #[must_use]
    pub fn arena_size(&self) -> usize {
        match self.plan() {
            Some(plan) if !self.created && plan.slices >= 2 => plan.physical,
            _ => 0,
        }
    }

    /// `Table::create`, in an arena of `arena_size()` bytes.
    /// # Safety
    /// `arena` is 8-aligned, writable, `arena_size()` bytes and as shared as
    /// the table should be; the table owns it until `destroy`.
    pub unsafe fn create(&mut self, arena: *mut u8) -> bool {
        let Some(plan) = self.plan().filter(|plan| plan.slices >= 2) else {
            return false;
        };
        if self.created || arena.is_null() || !arena.addr().is_multiple_of(8) {
            return false;
        }
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "the arena is 8-aligned (checked above)"
        )]
        let rows = arena.cast::<*mut Row>();
        let first = arena.wrapping_add(self.size * size_of::<*mut Row>());
        for i in 0..self.size {
            #[expect(
                clippy::cast_ptr_alignment,
                reason = "8-aligned: the stride is a multiple of 8"
            )]
            let row = first.wrapping_add(i * plan.stride).cast::<Row>();
            // SAFETY: per the caller; bucket `i` lies in the arena.
            unsafe { rows.wrapping_add(i).write(row) };
            // SAFETY: as above; the row isn't shared yet.
            unsafe { row.write(Row::empty()) };
        }
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "8-aligned: it follows whole rows"
        )]
        let pool = first.wrapping_add(self.size * plan.stride).cast::<Fixed>();
        let pool_bytes = plan.physical - self.size * (plan.stride + size_of::<*mut Row>());
        let Ok(width) = u32::try_from(plan.stride) else {
            return false;
        };
        let Some(layout) = Layout::new(0, width, pool_bytes, false) else {
            return false;
        };
        // SAFETY: the arena's last `pool_bytes`, per the caller.
        if unsafe { !fixed::construct(pool, layout, true, false) } {
            return false;
        }
        self.cursor = Box::into_raw(Box::new(Cursor {
            absolute: 0,
            collision: 0,
            current: vec![0; plan.stride / 8],
        }));
        self.rows = rows;
        self.pool = pool;
        self.memory_size = plan.logical - self.size * (plan.row_size + size_of::<*mut Row>());
        self.memory = arena;
        self.created = true;
        let (this, cursor) = (ptr::from_mut(self).addr(), self.cursor.expose_provenance());
        private(|tables| {
            tables
                .iter_mut()
                .filter(|t| t[0] == this)
                .for_each(|t| t[2] = cursor);
        });
        true
    }

    /// `Table::destroy`: frees the process-private parts and the locks, and
    /// returns the arena (null if the table was never created) for the host to
    /// free with the header.
    /// # Safety
    /// `this` is an initialised table nothing uses afterwards.
    pub unsafe fn destroy(this: *mut Table) -> *mut u8 {
        // SAFETY: per the caller.
        let table = unsafe { &mut *this };
        // SAFETY: made by `init`, freed only here.
        drop(unsafe { Box::from_raw(table.columns) });
        if !table.cursor.is_null() {
            // SAFETY: made by `create`, freed only here.
            drop(unsafe { Box::from_raw(table.cursor) });
        }
        table.iterator_mutex.destroy();
        table.mutex.destroy();
        // The header is shared: forked processes see `ready()` turn false, as upstream's do.
        private(|tables| tables.retain(|t| t[0] != this.addr()));
        std::mem::replace(&mut table.memory, ptr::null_mut())
    }

    #[must_use]
    pub fn is_created(&self) -> bool {
        self.created
    }

    /// `ready()`: created and not destroyed.
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

    /// `get_memory_size()`: the collision pool's bytes, as upstream reports them.
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
        // SAFETY: a created table's pool (see the invariant), or null.
        unsafe { self.pool.as_ref() }
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

    fn bucket_row(&self, index: usize) -> Live {
        // SAFETY: a created table's buckets (see the invariant); callers keep `index < size`.
        Live(unsafe { self.rows.wrapping_add(index).read() })
    }

    /// `get_by_index`: bucket `index`'s row if it is active.
    #[must_use]
    pub fn by_index(&self, index: usize) -> *mut Row {
        if !self.created || index >= self.size {
            return ptr::null_mut();
        }
        let row = self.bucket_row(index);
        if row.active() { row.0 } else { ptr::null_mut() }
    }

    /// The bucket `key` hashes to, and the bytes its row compares ([`clamp`]).
    fn bucket<'k>(&self, key: &'k [u8]) -> (Live, &'k [u8]) {
        let compared = clamp(key);
        let hash = (self.hash)(key, compared.len());
        #[expect(
            clippy::cast_possible_truncation,
            reason = "masked below the row count"
        )]
        let row = self.bucket_row((hash & (self.size as u64 - 1)) as usize);
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
        if !self.created {
            return ptr::null_mut();
        }
        let (root, key) = self.bucket(key);
        *lock = root.0;
        root.lock(self.warning);
        let mut found = 0;
        let mut row = root;
        if root.active() {
            let mut level = 1;
            while row.key() != key {
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
        if !self.created {
            return ptr::null_mut();
        }
        let (mut row, key) = self.bucket(key);
        *lock = row.0;
        row.lock(self.warning);
        while row.key() != key {
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
        if !self.created {
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
            if root.key() != key {
                return false;
            }
            root.clear();
            return true;
        };
        if root.key() == key {
            root.set_next(next.next());
            root.take(next, self.item_size);
        } else {
            let mut previous = root;
            while next.key() != key {
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
        (!row.is_null()).then_some(Live(row))
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

    fn cursor<T>(&self, step: impl FnOnce(&mut Cursor, usize) -> T) -> Option<T> {
        if self.cursor.is_null() {
            return None;
        }
        _ = self.iterator_mutex.lock();
        // SAFETY: this process's cursor (see the invariant), under the iterator mutex.
        let result = step(unsafe { &mut *self.cursor }, ROW + self.item_size);
        _ = self.iterator_mutex.unlock();
        Some(result)
    }

    /// `Table::rewind`.
    pub fn rewind(&self) {
        self.cursor(|cursor, row_size| {
            cursor.absolute = 0;
            cursor.collision = 0;
            Self::copy(cursor, None, row_size);
        });
    }

    /// Copy `row_size` bytes of `row` (zeros for `None`) to the cursor's row,
    /// which holds `stride >= row_size` bytes.
    fn copy(cursor: &mut Cursor, row: Option<Live>, row_size: usize) {
        let current = cursor.current.as_mut_ptr().cast::<u8>();
        match row {
            // SAFETY: a locked row's bytes, into the cursor's row.
            Some(row) => unsafe { ptr::copy_nonoverlapping(row.0.cast::<u8>(), current, row_size) },
            // SAFETY: the cursor's row.
            None => unsafe { ptr::write_bytes(current, 0, row_size) },
        }
    }

    /// `Table::forward`: copies the next row to `current()`, or an empty
    /// header past the last.
    pub fn forward(&self) {
        self.cursor(|cursor, row_size| {
            while cursor.absolute < self.size {
                let bucket = self.bucket_row(cursor.absolute);
                if !bucket.active() {
                    cursor.absolute += 1;
                    continue;
                }
                bucket.lock(self.warning);
                let found = Self::visit(cursor, bucket);
                if found.is_some() {
                    Self::copy(cursor, found, row_size);
                }
                bucket.unlock();
                if found.is_some() {
                    return;
                }
                cursor.absolute += 1;
            }
            Self::copy(cursor, None, ROW);
        });
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
        self.locked(lock, row, flags)
    }

    /// [`Table::get`] as a guard: None before `create`; a guard without a row
    /// when `key` has none.
    #[must_use]
    pub fn lock_get(&self, key: &[u8]) -> Option<Locked<'_>> {
        let mut lock = ptr::null_mut();
        let row = self.get(key, &mut lock);
        self.locked(lock, row, 0)
    }

    fn locked(&self, lock: *mut Row, row: *mut Row, flags: i32) -> Option<Locked<'_>> {
        (!lock.is_null()).then(|| Locked {
            table: self,
            bucket: Live(lock),
            row: (!row.is_null()).then_some(Live(row)),
            flags,
        })
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

/// A row's column data, `size` bytes at `data`: a locked row's or the
/// iterator's copy. Reads and writes stay inside the column's bytes; a column
/// that does not fit (another table's) reads as zero or empty.
#[derive(Clone, Copy)]
struct Data {
    data: *mut u8,
    size: usize,
}

impl Data {
    /// The column's bytes: a number's 8, or a string's length word and capacity.
    fn slot(self, column: &Column) -> Option<*mut u8> {
        let end = column.index.checked_add(column.width as usize)?;
        (end <= self.size && column.width >= 8).then(|| self.data.wrapping_add(column.index))
    }

    fn number(self, column: &Column) -> u64 {
        // SAFETY: the column's 8 bytes inside the row (`slot`).
        self.slot(column)
            .map_or(0, |at| unsafe { at.cast::<u64>().read_unaligned() })
    }

    fn bytes<'a>(self, column: &Column) -> &'a [u8] {
        let Some(at) = self.slot(column) else {
            return &[];
        };
        // SAFETY: the column's length word (`slot`).
        let length = unsafe { at.cast::<u32>().read_unaligned() } as usize;
        let length = length.min(column.width as usize - 4);
        // SAFETY: at most the column's capacity, inside the row.
        unsafe { slice::from_raw_parts(at.wrapping_add(4), length) }
    }

    /// `TableRow::set_value()`: a number column takes `value`'s 8 bytes; a string
    /// column `value`, cut to its capacity with a warning naming the row's `key`.
    fn set(self, column: &Column, value: &[u8], key: &[u8], warning: Warning) {
        let Some(at) = self.slot(column) else {
            return;
        };
        if column.kind != TYPE_STRING {
            if let Some(number) = value.first_chunk::<8>() {
                // SAFETY: the column's 8 bytes inside the row (`slot`), under its lock.
                unsafe { at.cast::<[u8; 8]>().write_unaligned(*number) };
            }
            return;
        }
        let capacity = column.width as usize - 4;
        if value.len() > capacity {
            let mut message = b"TableRow::set_value(): [key=".to_vec();
            message.extend_from_slice(c_str(key));
            message.extend_from_slice(b",field=");
            message.extend_from_slice(c_str(column.name()));
            message.extend_from_slice(b"]string value is too long");
            warn(warning, &message);
        }
        let value = value.get(..capacity).unwrap_or(value);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "upstream stores the low 4 bytes"
        )]
        let length = value.len() as u32;
        // SAFETY: the column's length word inside the row (`slot`), under its lock.
        unsafe { at.cast::<u32>().write_unaligned(length) };
        // SAFETY: at most the column's capacity after its length word, inside the row.
        unsafe { ptr::copy_nonoverlapping(value.as_ptr(), at.wrapping_add(4), value.len()) };
    }

    /// `TableColumn::clear()`: an empty string or a zero number.
    fn clear(self, column: &Column) {
        if let Some(at) = self.slot(column) {
            let width = if column.kind == TYPE_STRING { 4 } else { 8 };
            // SAFETY: the column's first `width` bytes inside the row (`slot`).
            unsafe { ptr::write_bytes(at, 0, width) };
        }
    }
}

/// A bucket this process locked, with the key's row in it if there is one
/// (`Table::set`'s or `Table::get`'s); unlocked on drop.
pub struct Locked<'a> {
    table: &'a Table,
    bucket: Live,
    row: Option<Live>,
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

    fn data(&self) -> Option<Data> {
        self.row.map(|row| Data {
            data: row.data(),
            size: self.table.item_size,
        })
    }

    /// A number column's 8 bytes (0 without a row).
    #[must_use]
    pub fn number(&self, column: &Column) -> u64 {
        self.data().map_or(0, |data| data.number(column))
    }

    /// A string column's bytes (empty without a row).
    #[must_use]
    pub fn bytes(&self, column: &Column) -> &[u8] {
        self.data().map_or(&[], |data| data.bytes(column))
    }

    /// `TableRow::set_value` of a number column's 8 bytes.
    pub fn set_number(&self, column: &Column, value: u64) {
        self.set(column, &value.to_ne_bytes());
    }

    /// `TableRow::set_value` of a string column: cut to it with a warning.
    /// Nothing for a number column.
    pub fn set_bytes(&self, column: &Column, value: &[u8]) {
        if column.kind == TYPE_STRING {
            self.set(column, value);
        }
    }

    fn set(&self, column: &Column, value: &[u8]) {
        if let (Some(row), Some(data)) = (self.row, self.data()) {
            data.set(column, value, row.key(), self.table.warning);
        }
    }

    /// `TableColumn::clear()`: an empty string or a zero number.
    pub fn clear(&self, column: &Column) {
        if let Some(data) = self.data() {
            data.clear(column);
        }
    }

    /// `Table::clear_row`: every column empty or zero.
    pub fn clear_row(&self) {
        for column in self.table.columns() {
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
    row: Vec<u64>,
    size: usize,
}

impl Current {
    fn live(&self) -> Live {
        Live(self.row.as_ptr().cast_mut().cast())
    }

    fn data(&self) -> Data {
        Data {
            data: self.live().data(),
            size: self.size,
        }
    }

    /// The row's key.
    #[must_use]
    pub fn key(&self) -> &[u8] {
        self.live().key()
    }

    /// A number column's 8 bytes.
    #[must_use]
    pub fn number(&self, column: &Column) -> u64 {
        self.data().number(column)
    }

    /// A string column's bytes.
    #[must_use]
    pub fn bytes(&self, column: &Column) -> &[u8] {
        self.data().bytes(column)
    }
}

#[cfg(test)]
#[path = "../../../tests/modules/memory/table.rs"]
mod tests;
