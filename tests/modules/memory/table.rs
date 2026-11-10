use super::*;
use std::alloc::{Layout as Alloc, alloc, dealloc};
use std::cell::RefCell;

thread_local! {
    static WARNINGS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static ERRORS: RefCell<Vec<c_int>> = const { RefCell::new(Vec::new()) };
}

fn record_warning(message: &[u8]) {
    WARNINGS.with(|w| {
        w.borrow_mut()
            .push(String::from_utf8_lossy(message).into_owned());
    });
}

fn record_error(code: c_int) {
    ERRORS.with(|e| e.borrow_mut().push(code));
}

fn warnings() -> Vec<String> {
    WARNINGS.with(|w| w.borrow_mut().drain(..).collect())
}

/// `swoole_hash_php` (DJB "times 33"), as upstream's default.
unsafe extern "C" fn djb(key: *const c_char, length: usize) -> u64 {
    let bytes = unsafe { slice::from_raw_parts(key.cast::<u8>(), length) };
    bytes.iter().fold(5381u64, |hash, &byte| {
        hash.wrapping_mul(33).wrapping_add(u64::from(byte))
    })
}

unsafe extern "C" fn collide(_: *const c_char, _: usize) -> u64 {
    0
}

/// A table in heap memory (C++ uses shared memory; the logic is the same).
struct Owned {
    table: *mut Table,
    arena: Vec<u64>,
}

impl Owned {
    fn new(rows: u32, conflict: f32) -> Self {
        #[expect(clippy::cast_ptr_alignment, reason = "alloc honours Table's layout")]
        let table = unsafe { alloc(Alloc::new::<Table>()) }.cast::<Table>();
        assert!(!table.is_null());
        assert!(unsafe { Table::init(table, rows, conflict, djb, record_warning, record_error) });
        Owned {
            table,
            arena: Vec::new(),
        }
    }

    fn get(&self) -> &Table {
        unsafe { &*self.table }
    }

    fn get_mut(&mut self) -> &mut Table {
        unsafe { &mut *self.table }
    }

    fn create(&mut self) -> bool {
        let size = self.get().arena_size();
        if size == 0 {
            return false;
        }
        self.arena = vec![0; size.div_ceil(8)];
        let arena = self.arena.as_mut_ptr().cast();
        unsafe { (*self.table).create(arena) }
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        let created = self.get().ready();
        let arena = unsafe { Table::destroy(self.table) };
        assert_eq!(!arena.is_null(), created);
        assert!(arena.is_null() || arena == self.arena.as_mut_ptr().cast());
        // Other processes sharing the header see the table destroyed.
        assert!(!self.get().ready());
        unsafe { dealloc(self.table.cast(), Alloc::new::<Table>()) };
    }
}

fn three_columns(rows: u32, conflict: f32) -> Owned {
    let mut table = Owned::new(rows, conflict);
    assert!(table.get_mut().add_column(b"id", TYPE_INT, 0));
    assert!(table.get_mut().add_column(b"name", TYPE_STRING, 32));
    assert!(table.get_mut().add_column(b"score", TYPE_FLOAT, 0));
    assert!(table.create());
    table
}

fn put(table: &Table, key: &[u8], id: i64) -> i32 {
    let (mut lock, mut flags) = (ptr::null_mut(), -1);
    let row = table.set(key, &mut lock, Some(&mut flags));
    assert!(!row.is_null() && !lock.is_null());
    let column = table.column(b"id").unwrap();
    unsafe {
        table.set_value(row, column, (&raw const id).cast(), 0);
        unlock(lock);
    }
    flags
}

fn read(table: &Table, key: &[u8]) -> Option<i64> {
    let mut lock = ptr::null_mut();
    let row = table.get(key, &mut lock);
    let value = (!row.is_null())
        .then(|| unsafe { get_number(row, table.column(b"id").unwrap()) }.cast_signed());
    unsafe { unlock(lock) };
    value
}

#[test]
fn make_clamps_rows_and_conflict_as_upstream() {
    for (rows, size) in [(0, 64), (65, 128), (1024, 1024), (0x9000_0000, 0x8000_0000)] {
        assert_eq!(Owned::new(rows, 0.2).get().size(), size);
    }
    assert!((Owned::new(64, 1.2).get().conflict_proportion() - 1.0).abs() < f32::EPSILON);
    assert!((Owned::new(64, 0.1).get().conflict_proportion() - 0.2).abs() < f32::EPSILON);
    assert!((Owned::new(64, 0.5).get().conflict_proportion() - 0.5).abs() < f32::EPSILON);
}

#[test]
fn columns_keep_every_declaration_and_find_the_first() {
    let mut table = Owned::new(64, 0.2);
    let t = table.get_mut();
    assert!(t.add_column(b"integer", TYPE_INT, 0));
    assert!(t.add_column(b"binary", TYPE_STRING, 12));
    assert!(t.add_column(b"integer", TYPE_FLOAT, 0));
    assert!(t.add_column(b"field\0name", TYPE_STRING, 4));
    assert!(!t.add_column(b"bad", 8, 0));
    assert_eq!(warnings(), ["Table::add_column(): unknown column type"]);
    assert_eq!((t.columns().len(), t.column_size()), (4, 3));
    let integer = t.column(b"integer").unwrap();
    assert_eq!(
        (integer.kind, integer.width, integer.index),
        (TYPE_INT, 8, 0)
    );
    let binary = t.column(b"binary").unwrap();
    assert_eq!((binary.width, binary.index), (16, 8));
    assert_eq!(t.column(b"field\0name").unwrap().index, 32);
    assert!(t.column(b"field").is_none());
    assert!(table.create());
    assert!(!table.get_mut().add_column(b"late", TYPE_INT, 0));
    assert!(!table.create());
}

#[test]
fn sizes_match_upstream_accounting() {
    for (rows, conflict, width) in [(64, 0.2, 0), (1024, 0.2, 9), (128, 1.0, 13), (65, 0.5, 31)] {
        let mut table = Owned::new(rows, conflict);
        assert!(table.get_mut().add_column(b"s", TYPE_STRING, width));
        let size = table.get().size();
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss,
            reason = "upstream's float row count"
        )]
        let all = (size as f32 * (1.0 + conflict)) as usize;
        let row = 88 + width + 4;
        let total = all * row + 48 + (all - size) * 24 + size * 8;
        assert_eq!(table.get().memory_total(), total);
        assert!(table.create());
        assert_eq!(table.get().memory_size(), total - size * 8 - row * size);
        assert_eq!(table.get().total_slices() as usize, all - size);
        assert_eq!(table.get().available_slices(), table.get().total_slices());
        assert!(table.arena.len() * 8 >= total);
    }
}

#[test]
fn set_get_del_and_counters() {
    let table = three_columns(64, 0.2);
    let t = table.get();
    assert_eq!(put(t, b"a", 1), NEW_ROW);
    assert_eq!(put(t, b"a", 2), 0);
    assert_eq!(put(t, b"b", 3), NEW_ROW);
    assert_eq!(
        (read(t, b"a"), read(t, b"b"), read(t, b"c")),
        (Some(2), Some(3), None)
    );
    assert!(t.exists(b"a") && !t.exists(b"c"));
    assert_eq!(t.count(), 2);
    assert!(t.del(b"a") && !t.del(b"a") && !t.del(b"c"));
    assert_eq!((t.count(), read(t, b"a")), (1, None));
    assert_eq!(
        t.stats(),
        Stats {
            insert_count: 2,
            update_count: 1,
            delete_count: 1,
            ..Stats::default()
        }
    );
}

#[test]
fn long_keys_are_cut_to_63_bytes() {
    let table = three_columns(64, 0.2);
    let t = table.get();
    let long = [b'q'; 100];
    put(t, &long, 7);
    let mut lock = ptr::null_mut();
    let row = t.get(&long[..63], &mut lock);
    assert_eq!(unsafe { row_key(row) }, &long[..63]);
    unsafe { unlock(lock) };
    assert!(t.exists(&long[..80]) && t.del(&long));
}

#[test]
fn collisions_chain_and_deleting_the_root_promotes_the_next() {
    let mut table = three_columns(64, 0.2);
    table.get_mut().set_hash(collide);
    let t = table.get();
    assert_eq!(put(t, b"k0", 0), NEW_ROW);
    assert_eq!(put(t, b"k1", 1), NEW_ROW);
    assert_eq!(put(t, b"k2", 2), NEW_ROW | CONFLICT);
    assert_eq!(put(t, b"k2", 5), CONFLICT);
    let stats = t.stats();
    assert_eq!((stats.conflict_count, stats.conflict_max_level), (2, 2));
    assert_eq!(t.available_slices() + 2, t.total_slices());
    assert!(t.del(b"k0"));
    assert_eq!(unsafe { row_key(t.by_index(0)) }, b"k1");
    assert_eq!((read(t, b"k1"), read(t, b"k2")), (Some(1), Some(5)));
    assert!(t.del(b"k2") && t.del(b"k1"));
    assert_eq!((t.count(), t.available_slices()), (0, t.total_slices()));
    assert!(t.by_index(0).is_null());
}

#[test]
fn exhaustion_keeps_the_root_locked_and_flags_untouched() {
    let mut table = three_columns(64, 0.2);
    table.get_mut().set_hash(collide);
    let t = table.get();
    let capacity = t.total_slices() as usize + 1;
    for i in 0..capacity {
        put(t, format!("full{i}").as_bytes(), 0);
    }
    let (mut lock, mut flags) = (ptr::null_mut(), 717);
    assert!(t.set(b"exhausted", &mut lock, Some(&mut flags)).is_null());
    assert_eq!((flags, lock, t.available_slices()), (717, t.by_index(0), 0));
    assert_eq!(unsafe { (*lock).lock.load(Ordering::Relaxed) }, 1);
    assert_eq!(
        ERRORS.with(|e| e.borrow_mut().drain(..).collect::<Vec<_>>()),
        [501]
    );
    unsafe { unlock(lock) };
    assert_eq!(t.count(), capacity);
}

#[test]
fn iteration_visits_every_row_once_including_chains() {
    let mut table = three_columns(64, 0.2);
    for hash in [djb as Hash, collide] {
        table.get_mut().set_hash(hash);
        let t = table.get();
        let keys: Vec<Vec<u8>> = (0..10).map(|i| format!("key{i}").into_bytes()).collect();
        for (i, key) in keys.iter().enumerate() {
            put(t, key, i64::try_from(i).unwrap());
        }
        let mut seen = Vec::new();
        t.rewind();
        loop {
            t.forward();
            let row = t.current();
            let key = unsafe { row_key(row) };
            if key.is_empty() {
                break;
            }
            seen.push(key.to_vec());
        }
        seen.sort();
        assert_eq!(seen, keys);
        for key in &keys {
            assert!(t.del(key));
        }
    }
}

#[test]
fn values_clear_and_cut_strings_with_a_warning() {
    let table = three_columns(64, 0.2);
    let t = table.get();
    let (mut lock, name) = (ptr::null_mut(), t.column(b"name").unwrap());
    let row = t.set(b"golang", &mut lock, None);
    let long = [b'x'; 40];
    unsafe { t.set_value(row, name, long.as_ptr(), long.len()) };
    assert_eq!(unsafe { get_string(row, name) }, &long[..32]);
    assert_eq!(
        warnings(),
        ["TableRow::set_value(): [key=golang,field=name]string value is too long"]
    );
    let score = t.column(b"score").unwrap();
    let value = 4.5f64;
    unsafe { t.set_value(row, score, (&raw const value).cast(), 0) };
    assert_eq!(f64::from_bits(unsafe { get_number(row, score) }), 4.5);
    unsafe { t.clear_row(row) };
    assert_eq!(
        unsafe { (get_string(row, name), get_number(row, score)) },
        (&[][..], 0)
    );
    unsafe { t.set_value(row, name, ptr::null(), 9) };
    assert_eq!(unsafe { get_string(row, name) }, b"");
    unsafe { unlock(lock) };
}

#[test]
fn a_lock_whose_owner_died_is_taken_over() {
    let table = three_columns(64, 0.2);
    let t = table.get();
    put(t, b"java", 1);
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = i32::try_from(child.id()).unwrap();
    child.wait().unwrap();
    let mut lock = ptr::null_mut();
    assert!(!t.get(b"java", &mut lock).is_null());
    unsafe { (*lock).lock_pid.store(pid, Ordering::Relaxed) };
    assert_eq!(read(t, b"java"), Some(1));
    assert_eq!(
        warnings(),
        [format!(
            "TableRow::lock(): lock process[{pid}] not exists, force unlock"
        )]
    );
}

#[test]
fn a_lock_held_too_long_is_taken_over() {
    let table = three_columns(64, 0.2);
    let t = table.get();
    put(t, b"java", 1);
    let mut lock = ptr::null_mut();
    assert!(!t.get(b"java", &mut lock).is_null());
    let start = Instant::now();
    assert_eq!(read(t, b"java"), Some(1));
    assert!(start.elapsed().as_millis() > FORCE_UNLOCK_MS);
    assert_eq!(warnings(), ["TableRow::lock(): timeout, force unlock"]);
}

#[test]
fn threads_update_one_row_under_its_lock() {
    let table = three_columns(64, 0.2);
    put(table.get(), b"shared", 0);
    let address = table.table as usize;
    let workers: Vec<_> = (0..4)
        .map(|_| {
            std::thread::spawn(move || {
                let t = unsafe { &*(address as *const Table) };
                let id = t.column(b"id").unwrap();
                for _ in 0..1000 {
                    let mut lock = ptr::null_mut();
                    let row = t.get(b"shared", &mut lock);
                    let value = unsafe { get_number(row, id) } + 1;
                    unsafe { t.set_value(row, id, (&raw const value).cast(), 0) };
                    unsafe { unlock(lock) };
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(read(table.get(), b"shared"), Some(4000));
}

#[test]
fn uncreated_tables_refuse_row_operations() {
    let table = Owned::new(64, 0.2);
    let t = table.get();
    let mut lock = ptr::null_mut();
    assert!(t.set(b"a", &mut lock, None).is_null() && lock.is_null());
    assert!(t.get(b"a", &mut lock).is_null() && !t.exists(b"a") && !t.del(b"a"));
    assert!(t.current().is_null() && !t.ready() && !t.is_created());
    assert_eq!((t.available_slices(), t.total_slices()), (0, 0));
    t.rewind();
    t.forward();
}

// Upstream's core-tests/src/memory/table.cpp, one test per TEST, with its inputs and
// expected values. `Upstream` is its `table_t`: a table with `id`, `name` (32 bytes)
// and `score` columns whose header and arena are shared mappings, as
// `Table::make` and `Table::create` lay them out, so forked children share it.

struct Entry {
    name: Vec<u8>,
    id: i64,
    score: f64,
}

fn row(name: &[u8], id: i64, score: f64) -> Entry {
    Entry {
        name: name.to_vec(),
        id,
        score,
    }
}

/// `Table::make(rows_size, conflict_proportion)`, its header in a shared mapping.
fn make(rows: u32, conflict: f32) -> *mut Table {
    let table = crate::members::sw_shm_malloc(size_of::<Table>()).cast::<Table>();
    assert!(!table.is_null());
    assert!(unsafe { Table::init(table, rows, conflict, djb, record_warning, record_error) });
    table
}

/// `Table::destroy()`.
fn destroy(table: *mut Table) {
    let arena = unsafe { Table::destroy(table) };
    if !arena.is_null() {
        crate::members::sw_shm_free(arena.cast());
    }
    crate::members::sw_shm_free(table.cast());
}

struct Upstream(*mut Table);

impl Upstream {
    fn new(rows: u32) -> Self {
        let table = make(rows, 0.2);
        let this = unsafe { &mut *table };
        assert!(this.add_column(b"id", TYPE_INT, 0));
        assert!(this.add_column(b"name", TYPE_STRING, 32));
        assert!(this.add_column(b"score", TYPE_FLOAT, 0));
        assert!(
            unsafe { crate::members::swr_table_create(table) },
            "create failed"
        );
        Upstream(table)
    }

    fn ptr(&self) -> &Table {
        unsafe { &*self.0 }
    }

    fn set(&self, key: &str, value: &Entry) -> bool {
        let table = self.ptr();
        let mut lock = ptr::null_mut();
        let row = table.set(key.as_bytes(), &mut lock, None);
        if row.is_null() {
            unsafe { unlock(lock) };
            return false;
        }
        let column = |name: &[u8]| table.column(name).unwrap();
        unsafe {
            table.set_value(row, column(b"id"), (&raw const value.id).cast(), 8);
            table.set_value(row, column(b"name"), value.name.as_ptr(), value.name.len());
            table.set_value(row, column(b"score"), (&raw const value.score).cast(), 8);
            unlock(lock);
        }
        true
    }

    fn get(&self, key: &str) -> Entry {
        let table = self.ptr();
        let mut lock = ptr::null_mut();
        let found = table.get(key.as_bytes(), &mut lock);
        let column = |name: &[u8]| table.column(name).unwrap();
        let mut result = row(b"", 0, 0.0);
        if !found.is_null() {
            unsafe {
                result.id = get_number(found, column(b"id")).cast_signed();
                result.score = f64::from_bits(get_number(found, column(b"score")));
                result.name = get_string(found, column(b"name")).to_vec();
            }
        }
        unsafe { unlock(lock) };
        result
    }

    fn del(&self, key: &str) -> bool {
        self.ptr().del(key.as_bytes())
    }

    fn exists(&self, key: &str) -> bool {
        let mut lock = ptr::null_mut();
        let row = self.ptr().get(key.as_bytes(), &mut lock);
        unsafe { unlock(lock) };
        !row.is_null()
    }

    fn count(&self) -> usize {
        self.ptr().count()
    }

    /// A table filled by `create_table`.
    fn colliding(rows: u32) -> Self {
        let table = Upstream::new(rows);
        table.create_table();
        table
    }

    /// `create_table`: five rows that all hash to 1.
    fn create_table(&self) {
        let table = self;
        unsafe { &mut *table.0 }.set_hash(one);
        assert!(table.set("php", &row(b"php", 1, 1.245)));
        assert!(table.set("java", &row(b"java", 2, JAVA_SCORE)));
        assert!(table.set("c++", &row(b"c++", 3, 4.888)));
        assert!(table.set("js", &row(b"js", 9, 6565.0)));
        assert!(table.set("golang", &row(b"golang", 4, 9.888)));
    }

    fn used_slices(&self) -> usize {
        (self.ptr().total_slices() - self.ptr().available_slices()) as usize
    }
}

impl Drop for Upstream {
    fn drop(&mut self) {
        destroy(self.0);
    }
}

unsafe extern "C" fn one(_: *const c_char, _: usize) -> u64 {
    1
}

const TEST_TABLE_SIZE: u32 = 128;
#[expect(clippy::approx_constant, reason = "upstream's literal, not pi")]
const JAVA_SCORE: f64 = 3.141_592_6;
const TEST_JPG_MD5SUM: &str = "64a42b4c0f3c65a14c23b60d3880a917";

/// `TEST(table, create)`.
#[test]
fn table_create() {
    let table = Upstream::new(1024);
    let ptr = table.ptr();
    assert!(ptr.memory_size() > ptr.size() * ptr.column_size());
    assert!(!unsafe { crate::members::swr_table_create(table.0) }); // create again should fail
    assert!(table.set("php", &row(b"php", 1, 1.245)));
    assert!(table.set("java", &row(b"java", 2, JAVA_SCORE)));
    assert!(table.set("c++", &row(b"c++", 3, 4.888)));
    assert_eq!(table.count(), 3);
    let r1 = table.get("java");
    assert_eq!(r1.id, 2);
    assert_eq!(r1.score.to_bits(), JAVA_SCORE.to_bits());
    assert_eq!(r1.name, b"java");
    assert!(ptr.column(b"not-exists").is_none());
    assert!(table.exists("php"));
    assert!(table.del("php"));
    assert!(!table.exists("php"));
    assert!(!table.del("not-exists"));
    // Test with a string that is longer than the column size
    let long = format!("golang {TEST_JPG_MD5SUM}{TEST_JPG_MD5SUM}");
    assert!(table.set("golang", &row(long.as_bytes(), 3, 4.888)));
    warnings();
}

/// `start_iterator`: every row the iterator yields exists, and it yields `count()`.
fn start_iterator(ptr: &Table) {
    ptr.rewind();
    let mut count = 0;
    loop {
        ptr.forward();
        let row = ptr.current();
        let key_len = unsafe { (*row).key_len };
        if key_len == 0 {
            break;
        }
        let key = unsafe { &(&(*row).key)[..usize::from(key_len)] };
        assert!(ptr.exists(key));
        count += 1;
    }
    assert_eq!(count, ptr.count());
}

/// `TEST(table, iterator)`.
#[test]
fn table_iterator() {
    let table = Upstream::new(1024);
    table.set("php", &row(b"php", 1, 1.245));
    table.set("java", &row(b"java", 2, JAVA_SCORE));
    table.set("c++", &row(b"c++", 3, 4.888));
    start_iterator(table.ptr());
}

/// `TEST(table, iterator_2)`: every key hashes to 1.
#[test]
fn table_iterator_2() {
    let table = Upstream::new(1024);
    unsafe { &mut *table.0 }.set_hash(one);
    table.set("php", &row(b"php", 1, 1.245));
    table.set("java", &row(b"java", 2, JAVA_SCORE));
    table.set("c++", &row(b"c++", 3, 4.888));
    start_iterator(table.ptr());
}

/// `TEST(table, conflict1)`.
#[test]
fn table_conflict1() {
    let table = Upstream::new(TEST_TABLE_SIZE);
    assert!(!table.exists("swift"));
    table.create_table();
    assert!(!table.exists("kotlin"));
    assert!(table.del("php"));
    assert!(!table.exists("php"));
    assert!(table.set("rust", &row(b"rust", 5, 9.888)));
    assert!(table.del("golang"));
    assert!(!table.exists("golang"));
    assert!(table.set("erlang", &row(b"erlang", 6, 12.888)));
    assert!(table.del("java"));
    assert!(!table.exists("java"));
    assert_eq!(table.used_slices(), table.count() - 1);
}

/// `TEST(table, conflict2)`.
#[test]
fn table_conflict2() {
    let table = Upstream::colliding(TEST_TABLE_SIZE);
    assert!(table.del("java"));
    assert!(!table.exists("java"));
    assert!(table.set("rust", &row(b"rust", 5, 9.888)));
    assert!(table.del("golang"));
    assert!(!table.exists("golang"));
    assert!(table.set("erlang", &row(b"erlang", 6, 12.888)));
    assert_eq!(table.used_slices(), table.count() - 1);
}

/// `TEST(table, conflict3)`.
#[test]
fn table_conflict3() {
    let table = Upstream::colliding(TEST_TABLE_SIZE);
    assert!(table.del("golang"));
    assert!(table.set("erlang", &row(b"erlang", 6, 12.888)));
    assert!(table.del("java"));
    assert_eq!(table.used_slices(), table.count() - 1);
}

/// `TEST(table, conflict4)`.
#[test]
fn table_conflict4() {
    let table = Upstream::colliding(TEST_TABLE_SIZE);
    assert!(table.del("c++"));
    assert!(table.set("rust", &row(b"rust", 5, 9.888)));
    assert!(table.del("golang"));
    assert!(table.set("erlang", &row(b"erlang", 6, 12.888)));
    assert!(table.del("java"));
    assert_eq!(table.used_slices(), table.count() - 1);
}

/// `TEST(table, get_value)`: clearing a string column keeps its bytes, which
/// `ASSERT_STREQ` reads up to their NUL.
#[test]
fn table_get_value() {
    let table = Upstream::colliding(TEST_TABLE_SIZE);
    let ptr = table.ptr();
    let mut lock = ptr::null_mut();
    let row = ptr.get(b"php", &mut lock);
    unsafe { unlock(lock) };
    let column_id = ptr.column(b"id").unwrap();
    let column_name = ptr.column(b"name").unwrap();
    let column_score = ptr.column(b"score").unwrap();
    let streq = |expected: &std::ffi::CStr| {
        let value = unsafe { get_string(row, column_name) };
        assert_eq!(
            unsafe { std::ffi::CStr::from_ptr(value.as_ptr().cast()) },
            expected
        );
    };
    streq(c"php");
    assert_eq!(
        f64::from_bits(unsafe { get_number(row, column_score) }),
        1.245
    );
    assert_eq!(unsafe { get_number(row, column_id) }.cast_signed(), 1);
    unsafe {
        clear_value(row, column_id);
        clear_value(row, column_name);
        clear_value(row, column_score);
    }
    streq(c"php");
    assert_eq!(
        f64::from_bits(unsafe { get_number(row, column_score) }),
        0.0
    );
    assert_eq!(unsafe { get_number(row, column_id) }.cast_signed(), 0);
}

/// `TEST(table, lock)`: four threads in turn take the row's lock and keep it; the
/// test unlocks it once at the end.
#[test]
fn table_lock() {
    let table = Upstream::colliding(TEST_TABLE_SIZE);
    let address = table.0.expose_provenance();
    let mut lock = 0;
    for _ in 0..=3 {
        lock = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let ptr = unsafe { &*ptr::with_exposed_provenance::<Table>(address) };
                    let mut lock = ptr::null_mut();
                    let row = ptr.get(b"php", &mut lock);
                    let value = unsafe { get_string(row, ptr.column(b"name").unwrap()) };
                    assert_eq!(
                        unsafe { std::ffi::CStr::from_ptr(value.as_ptr().cast()) },
                        c"php"
                    );
                    lock.expose_provenance()
                })
                .join()
                .unwrap()
        });
    }
    unsafe { unlock(ptr::with_exposed_provenance_mut(lock)) };
}

/// `TEST(table, size_limit)`.
#[test]
fn table_size_limit() {
    let t1 = make(0x9000_0000, 1.2);
    assert_eq!(unsafe { &*t1 }.size(), 0x8000_0000);
    assert_eq!(unsafe { &*t1 }.conflict_proportion(), 1.0);
    assert!(!unsafe { &mut *t1 }.add_column(b"bad_field", 8, 0));
    let t2 = make(1024, 0.1);
    assert_eq!(unsafe { &*t2 }.size(), 1024);
    assert_eq!(unsafe { &*t2 }.conflict_proportion(), 0.2f32);
    destroy(t1);
    destroy(t2);
    warnings();
}

/// Waits for `child` and returns its exit status.
fn wait(child: libc::pid_t) -> c_int {
    assert!(child > 0);
    let mut status = 0;
    assert_eq!(unsafe { libc::waitpid(child, &raw mut status, 0) }, child);
    assert!(libc::WIFEXITED(status));
    libc::WEXITSTATUS(status)
}

/// `TEST(table, lock_crash)`: a child exits holding the row's lock; the parent
/// still gets the row.
#[test]
fn table_lock_crash() {
    let table = Upstream::colliding(TEST_TABLE_SIZE);
    let ptr = table.ptr();
    let child = unsafe { libc::fork() };
    if child == 0 {
        let mut lock = ptr::null_mut();
        ptr.get(b"java", &mut lock);
        unsafe { libc::usleep(5) };
        unsafe { libc::_exit(200) }; // Simulate a crash in the child process, no release lock
    }
    assert_eq!(wait(child), 200);
    let mut lock = ptr::null_mut();
    assert!(!ptr.get(b"java", &mut lock).is_null());
    unsafe { unlock(lock) };
}

/// `TEST(table, lock_race)`: a child and the parent take the row's lock at once.
#[test]
fn table_lock_race() {
    let table = Upstream::colliding(TEST_TABLE_SIZE);
    let ptr = table.ptr();
    let child = unsafe { libc::fork() };
    if child == 0 {
        let mut lock = ptr::null_mut();
        let found = !ptr.get(b"java", &mut lock).is_null();
        unsafe { libc::usleep(5) };
        unsafe { unlock(lock) };
        unsafe { libc::_exit(c_int::from(!found)) };
    }
    let mut lock = ptr::null_mut();
    assert!(!ptr.get(b"java", &mut lock).is_null());
    unsafe { unlock(lock) };
    assert_eq!(wait(child), 0);
}
