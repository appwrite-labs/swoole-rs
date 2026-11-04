use super::*;
use std::alloc::{Layout as Alloc, alloc, dealloc};
use std::cell::RefCell;

thread_local! {
    static WARNINGS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static ERRORS: RefCell<Vec<c_int>> = const { RefCell::new(Vec::new()) };
}

unsafe extern "C" fn record_warning(message: *const c_char, length: usize) {
    let bytes = unsafe { slice::from_raw_parts(message.cast::<u8>(), length) };
    WARNINGS.with(|w| {
        w.borrow_mut()
            .push(String::from_utf8_lossy(bytes).into_owned());
    });
}

unsafe extern "C" fn record_error(code: c_int) {
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
