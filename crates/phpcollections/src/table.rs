//! PHP `Swoole\Table`: class registration, object handlers and every method,
//! over the memory crate's table (its header from Swoole's global pool, its
//! arena shared memory), hashing keys with Zend's string hash.
//!
//! Each method keeps its table alive while it runs, so PHP code it runs (a
//! `__toString`, an error handler) cannot free it underneath; `destroy()`
//! frees it once the last running method is done.
use std::{
    cell::Cell,
    ffi::{CStr, c_int},
    rc::Rc,
};
use swoole_rs_corebase::CCast;
use swoole_rs_memory::{
    members::SharedTable,
    table::{Column, Current, Locked, TYPE_FLOAT, TYPE_INT, TYPE_STRING, Table},
};
use swoole_rs_phpstubs::table;
use swoole_rs_zend::{
    Call, Custom, CustomClass, E_ERROR, E_WARNING, Message, Minit, Owned, OwnedArray, Value,
    ZString,
    bailout::{self, Bailout},
    method_table,
    sys::ZEND_ACC_PUBLIC,
};

/// `SW_TABLE_KEY_SIZE`, `SW_TABLE_CONFLICT_PROPORTION`.
const KEY_SIZE: usize = 64;
const CONFLICT_PROPORTION: f64 = 0.2;
/// `SW_ERROR_MALLOC_FAIL`, `SW_ERROR_PHP_FATAL_ERROR`, `SW_ERROR_WRONG_OPERATION`,
/// `SW_ERROR_PHP_RUNTIME_NOTICE`.
const MALLOC_FAIL: i64 = 501;
const PHP_FATAL_ERROR: c_int = 503;
const WRONG_OPERATION: c_int = 509;
const PHP_RUNTIME_NOTICE: c_int = 510;

static TABLE: CustomClass<Held> = CustomClass::new();

/// `struct TableObject { Table *ptr; zend_object std; }`: none until
/// `__construct`, and again after `destroy()`. Like upstream, the shared
/// table outlives its object (forked workers may use it).
struct Held(Cell<Option<Rc<SharedTable>>>);

impl Custom for Held {
    fn class() -> &'static CustomClass<Self> {
        &TABLE
    }

    fn new() -> Self {
        Self(Cell::new(None))
    }
}

impl Held {
    fn get(&self) -> Option<Rc<SharedTable>> {
        let table = self.0.take();
        let copy = table.clone();
        // Still empty: cloning an `Rc` runs no code, so nothing filled the slot.
        #[expect(
            clippy::mem_forget,
            reason = "the slot is still empty, so there is nothing to release"
        )]
        std::mem::forget(self.0.replace(table));
        copy
    }
}

method_table! {
    static METHODS = [
        c"__construct" => zim_swoole_table___construct, &table::arginfo_class_Swoole_Table___construct, ZEND_ACC_PUBLIC;
        c"column" => zim_swoole_table_column, &table::arginfo_class_Swoole_Table_column, ZEND_ACC_PUBLIC;
        c"create" => zim_swoole_table_create, &table::arginfo_class_Swoole_Table_create, ZEND_ACC_PUBLIC;
        c"destroy" => zim_swoole_table_destroy, &table::arginfo_class_Swoole_Table_destroy, ZEND_ACC_PUBLIC;
        c"set" => zim_swoole_table_set, &table::arginfo_class_Swoole_Table_set, ZEND_ACC_PUBLIC;
        c"get" => zim_swoole_table_get, &table::arginfo_class_Swoole_Table_get, ZEND_ACC_PUBLIC;
        c"count" => zim_swoole_table_count, &table::arginfo_class_Swoole_Table_count, ZEND_ACC_PUBLIC;
        c"del" => zim_swoole_table_del, &table::arginfo_class_Swoole_Table_del, ZEND_ACC_PUBLIC;
        c"delete" => zim_swoole_table_del, &table::arginfo_class_Swoole_Table_del, ZEND_ACC_PUBLIC;
        c"exists" => zim_swoole_table_exists, &table::arginfo_class_Swoole_Table_exists, ZEND_ACC_PUBLIC;
        c"exist" => zim_swoole_table_exists, &table::arginfo_class_Swoole_Table_exists, ZEND_ACC_PUBLIC;
        c"incr" => zim_swoole_table_incr, &table::arginfo_class_Swoole_Table_incr, ZEND_ACC_PUBLIC;
        c"decr" => zim_swoole_table_decr, &table::arginfo_class_Swoole_Table_decr, ZEND_ACC_PUBLIC;
        c"getSize" => zim_swoole_table_getSize, &table::arginfo_class_Swoole_Table_getSize, ZEND_ACC_PUBLIC;
        c"getMemorySize" => zim_swoole_table_getMemorySize, &table::arginfo_class_Swoole_Table_getMemorySize, ZEND_ACC_PUBLIC;
        c"stats" => zim_swoole_table_stats, &table::arginfo_class_Swoole_Table_stats, ZEND_ACC_PUBLIC;
        c"rewind" => zim_swoole_table_rewind, &table::arginfo_class_Swoole_Table_rewind, ZEND_ACC_PUBLIC;
        c"valid" => zim_swoole_table_valid, &table::arginfo_class_Swoole_Table_valid, ZEND_ACC_PUBLIC;
        c"next" => zim_swoole_table_next, &table::arginfo_class_Swoole_Table_next, ZEND_ACC_PUBLIC;
        c"current" => zim_swoole_table_current, &table::arginfo_class_Swoole_Table_current, ZEND_ACC_PUBLIC;
        c"key" => zim_swoole_table_key, &table::arginfo_class_Swoole_Table_key, ZEND_ACC_PUBLIC;
    ];
}

/// `php_swoole_table_minit`: register `Swoole\Table`, an `Iterator` and
/// `Countable`, with its properties and type constants.
/// # Errors
/// A registration bailed out.
pub fn table_minit(minit: &Minit) -> Result<(), Bailout> {
    minit.register_object::<Held>(c"Swoole\\Table", &METHODS.0, 0);
    minit.implements(&TABLE, minit.iterator());
    minit.implements(&TABLE, minit.countable());
    minit.property_null(&TABLE, c"size", ZEND_ACC_PUBLIC);
    minit.property_null(&TABLE, c"memorySize", ZEND_ACC_PUBLIC);
    for (name, value) in [
        (c"TYPE_INT", TYPE_INT),
        (c"TYPE_STRING", TYPE_STRING),
        (c"TYPE_FLOAT", TYPE_FLOAT),
    ] {
        minit.class_constant_long(&TABLE, name, value.into());
    }
    Ok(())
}

/// `php_swoole_fatal_error(level, "%s", message)`: always reported.
fn fatal(level: i32, message: &CStr) {
    swoole_rs_corebase::log::set_last_error(PHP_FATAL_ERROR);
    bailout::defer(swoole_rs_zend::report(level, message));
}

/// `php_swoole_fatal_error(E_WARNING, "<before>%s<after>", text)` of Zend's
/// string bytes (`%s` stops at a NUL); true if a user error handler bailed
/// out (deferred: the method stops).
fn report(before: &[u8], text: &[u8], after: &[u8]) -> bool {
    let mut message = Message::new();
    message.push(before);
    message.push(text.split(|&b| b == 0).next().unwrap_or_default());
    message.push(after);
    swoole_rs_corebase::log::set_last_error(PHP_FATAL_ERROR);
    let reported = swoole_rs_zend::report(E_WARNING, message.as_cstr());
    let bailed = reported.is_err();
    bailout::defer(reported);
    bailed
}

/// `php_swoole_error(E_WARNING, "failed to set('%*s'), unable to allocate
/// memory", (int) keylen, key)`: the key up to a NUL, right-aligned to its
/// length; shown when `swoole.display_errors` is on.
fn set_failed(key: &[u8]) {
    let text = key.split(|&b| b == 0).next().unwrap_or_default();
    let mut message = Message::new();
    message.push(b"failed to set('");
    for _ in text.len()..key.len() {
        message.push(b" ");
    }
    message.push(text);
    message.push(b"'), unable to allocate memory");
    swoole_rs_corebase::log::set_last_error(PHP_RUNTIME_NOTICE);
    if swoole_rs_phpswoole::display_errors() {
        bailout::defer(swoole_rs_zend::report(E_WARNING, message.as_cstr()));
    }
}

/// The table this object made, if any.
fn table_of(call: &Call) -> Option<Rc<SharedTable>> {
    call.custom::<Held>()?.get()
}

/// `table_get_and_check_ptr`: the table, or Swoole's fatal error (which exits).
fn constructed(call: &Call) -> Option<Rc<SharedTable>> {
    let table = table_of(call);
    if table.is_none() {
        bailout::defer(swoole_rs_phpswoole::swoole_fatal_error(
            WRONG_OPERATION,
            c"must call constructor first",
        ));
    }
    table
}

/// `table_get_and_check_ptr2`: also created and not destroyed (an `E_ERROR`).
fn created(call: &Call) -> Option<Rc<SharedTable>> {
    let table = constructed(call)?;
    if !table.ready() {
        // The deferred bailout ends the method here, as upstream's fatal does.
        fatal(E_ERROR, c"table is not created or has been destroyed");
        return None;
    }
    Some(table)
}

/// `change(table)` on this object's table while no running method uses it
/// (one that does has created it, which every change refuses); false otherwise.
fn change(call: &Call, change: impl FnOnce(&mut SharedTable) -> bool) -> bool {
    let Some(held) = call.custom::<Held>() else {
        return false;
    };
    let mut table = held.0.take();
    let changed = table.as_mut().and_then(Rc::get_mut).is_some_and(change);
    held.0.set(table);
    changed
}

/// `__construct(int $table_size, float $conflict_proportion = 0.2)`.
fn construct(call: &Call) {
    let Some(held) = call.custom::<Held>() else {
        return;
    };
    if held.get().is_some() {
        call.throw_constructed_twice();
        call.set_bool(false);
        return;
    }
    let parsed = call
        .arity(1, 2)
        .and_then(|()| Ok((call.long(1, 0)?, call.double(2, CONFLICT_PROPORTION)?)));
    let Ok((rows, conflict)) = parsed else {
        call.set_bool(false);
        return;
    };
    let made = SharedTable::make(
        rows.c_cast::<u32>(),
        conflict.c_cast::<f32>(),
        swoole_rs_zend::string::hash,
    );
    let Some(table) = made else {
        let exception = swoole_rs_phpswoole::exception();
        call.throw(exception, c"global memory allocation failure", MALLOC_FAIL);
        call.set_bool(false);
        return;
    };
    held.0.set(Some(Rc::new(table)));
}

/// `column(string $name, int $type, int $size = 0): bool`.
fn column(call: &Call) {
    // No handle across the parse: a `__toString` it calls may create the table.
    if constructed(call).is_none() {
        return;
    }
    let parsed = call
        .arity(2, 3)
        .and_then(|()| Ok((call.string(1, b"")?, call.long(2, 0)?, call.long(3, 0)?)));
    let Ok((name, kind, mut size)) = parsed else {
        call.set_bool(false);
        return;
    };
    if kind == i64::from(TYPE_STRING) {
        if size < 1 {
            fatal(
                E_WARNING,
                c"the length of string type values has to be more than zero",
            );
            call.set_bool(false);
            return;
        }
        // SW_MEM_ALIGNED_SIZE: to `sizeof(unsigned long)`.
        size = (size + 7) & !7;
    }
    if table_of(call).is_some_and(|table| table.ready()) {
        fatal(
            E_WARNING,
            c"unable to add column after table has been created",
        );
        call.set_bool(false);
        return;
    }
    call.set_bool(change(call, |table| {
        table.add_column(name, kind.c_cast::<u32>(), size.c_cast::<usize>())
    }));
}

/// `create(): bool`.
fn create(call: &Call) {
    if constructed(call).is_none() {
        return;
    }
    if !change(call, SharedTable::create) {
        fatal(E_ERROR, c"unable to allocate memory");
        call.set_bool(false);
        return;
    }
    let Some(table) = table_of(call) else { return };
    call.update_property_long(Some(&TABLE), c"size", long(table.size()));
    call.update_property_long(Some(&TABLE), c"memorySize", long(table.memory_size()));
    call.set_bool(true);
}

fn long(value: usize) -> i64 {
    value.c_cast::<i64>()
}

/// `destroy(): bool`.
fn destroy(call: &Call) {
    if created(call).is_none() {
        return;
    }
    if let Some(table) = call.custom::<Held>().and_then(|held| held.0.take()) {
        table.destroy();
    }
    call.set_bool(true);
}

/// `table_set_field_value`: store `value` into a row's column; a new row
/// clears a column its array leaves out or sets to null. None, with nothing
/// written, once the conversion bailed out: upstream's longjmp skipped the
/// write and every later column.
fn store(row: &Locked, column: &Column, value: Option<&Value>, initialize: bool) -> Option<()> {
    let Some(value) = value.filter(|value| !(initialize && value.is_null())) else {
        row.clear(column);
        return Some(());
    };
    match column.kind {
        // Copied, cut to the column with a warning.
        TYPE_STRING => row.set_bytes(column, value.applied_string()?.bytes()),
        TYPE_FLOAT => row.set_number(column, value.applied_double()?.to_bits()),
        _ => row.set_number(column, (value.applied_long()?).c_cast::<u64>()),
    }
    Some(())
}

/// `set(string $key, array $value): bool`.
fn set(call: &Call) {
    let Some(table) = created(call) else { return };
    let parsed = call
        .arity(2, 2)
        .and_then(|()| Ok((call.string(1, b"")?, call.array_ref(2)?)));
    let Ok((key, array)) = parsed else {
        call.set_bool(false);
        return;
    };
    if !table.ready() {
        fatal(E_ERROR, c"the table object does not exist");
        call.set_bool(false);
        return;
    }
    if key.len() >= KEY_SIZE && report(b"key[", key, b"] is too long") {
        call.set_bool(false);
        return;
    }
    let Some(row) = table.lock_set(key).filter(Locked::found) else {
        set_failed(key);
        call.set_bool(false);
        return;
    };
    let stored = if row.is_new() {
        table
            .columns()
            .try_for_each(|column| store(&row, column, array.find(column.name()), true))
    } else {
        array.entries().try_for_each(|(name, value)| {
            match name.and_then(|name| table.column(name)) {
                Some(column) => store(&row, column, Some(value), false),
                None => Some(()),
            }
        })
    };
    drop(row);
    call.set_bool(stored.is_some());
}

/// `incr()`/`decr()`: `(string $key, string $column, mixed $incrby = 1)`.
fn update(call: &Call, subtract: bool) {
    let Some(table) = created(call) else { return };
    let parsed = call
        .arity(2, 3)
        .and_then(|()| Ok((call.string(1, b"")?, call.string(2, b"")?)));
    let Ok((key, name)) = parsed else {
        call.set_bool(false);
        return;
    };
    let by = call.value(3);
    let Some(row) = table.lock_set(key).filter(Locked::found) else {
        fatal(E_WARNING, c"unable to allocate memory");
        call.set_bool(false);
        return;
    };
    let Some(column) = table.column(name) else {
        drop(row);
        report(b"column[", name, b"] does not exist");
        call.set_bool(false);
        return;
    };
    if row.is_new() {
        row.clear_row();
    }
    match column.kind {
        TYPE_STRING => {
            drop(row);
            fatal(
                E_WARNING,
                if subtract {
                    c"can't execute 'decr' on a string type column"
                } else {
                    c"can't execute 'incr' on a string type column"
                },
            );
            call.set_bool(false);
        }
        TYPE_FLOAT => {
            let Some(by) = by.map_or(Some(1.0), Value::applied_double) else {
                return;
            };
            let current = f64::from_bits(row.number(column));
            let value = if subtract { current - by } else { current + by };
            row.set_number(column, value.to_bits());
            drop(row);
            set_double(call, value);
        }
        _ => {
            let Some(by) = by.map_or(Some(1), Value::applied_long) else {
                return;
            };
            let current = row.number(column).c_cast::<i64>();
            let value = if subtract {
                current.wrapping_sub(by)
            } else {
                current.wrapping_add(by)
            };
            row.set_number(column, value.c_cast::<u64>());
            drop(row);
            call.set_long(value);
        }
    }
}

fn set_double(call: &Call, value: f64) {
    call.set_owned(Owned::double(value));
}

fn incr(call: &Call) {
    update(call, false);
}

fn decr(call: &Call) {
    update(call, true);
}

/// A locked row, or the iterator's copy of one.
trait Row {
    fn number(&self, column: &Column) -> u64;
    fn bytes(&self, column: &Column) -> &[u8];
}

impl Row for Locked<'_> {
    fn number(&self, column: &Column) -> u64 {
        Locked::number(self, column)
    }
    fn bytes(&self, column: &Column) -> &[u8] {
        Locked::bytes(self, column)
    }
}

impl Row for Current {
    fn number(&self, column: &Column) -> u64 {
        Current::number(self, column)
    }
    fn bytes(&self, column: &Column) -> &[u8] {
        Current::bytes(self, column)
    }
}

/// A column's value in a row.
enum Field<'a> {
    Long(i64),
    Double(f64),
    Bytes(&'a [u8]),
}

fn read<'a>(row: &'a impl Row, column: &Column) -> Field<'a> {
    match column.kind {
        TYPE_STRING => Field::Bytes(row.bytes(column)),
        TYPE_FLOAT => Field::Double(f64::from_bits(row.number(column))),
        _ => Field::Long(row.number(column).c_cast::<i64>()),
    }
}

/// `table_row2array`: every column of a row, as the result.
fn row_array(call: &Call, table: &Table, row: &impl Row) {
    let mut array = OwnedArray::default();
    for column in table.columns() {
        let name = column.name();
        match read(row, column) {
            Field::Long(value) => array.insert_long(name, value),
            Field::Double(value) => array.insert_double(name, value),
            Field::Bytes(bytes) => array.insert_bytes(name, Some(bytes)),
        }
    }
    call.set_owned(array.into_owned());
}

fn set_bytes(call: &Call, bytes: &[u8]) {
    call.set_zstring(ZString::new(bytes));
}

/// `get(string $key, ?string $field = null): mixed`.
fn get(call: &Call) {
    let Some(table) = created(call) else { return };
    let parsed = call.arity(1, 2).and_then(|()| {
        let key = call.string(1, b"")?;
        let field = if call.value(2).is_none_or(Value::is_null) {
            None
        } else {
            call.zstr(2)?
        };
        Ok((key, field))
    });
    let Ok((key, field)) = parsed else {
        call.set_bool(false);
        return;
    };
    let Some(row) = table.lock_get(key).filter(Locked::found) else {
        call.set_bool(false);
        return;
    };
    if let Some(field) = field {
        match table.column(field.bytes()) {
            None => call.set_bool(false),
            Some(column) => match read(&row, column) {
                Field::Long(value) => call.set_long(value),
                Field::Double(value) => set_double(call, value),
                Field::Bytes(bytes) => set_bytes(call, bytes),
            },
        }
    } else {
        row_array(call, &table, &row);
    }
}

/// A key argument (`"s"` or `Z_PARAM_STRING`), or `RETURN_FALSE`.
fn key(call: &Call) -> Option<&[u8]> {
    let key = call.arity(1, 1).and_then(|()| call.string(1, b""));
    if key.is_err() {
        call.set_bool(false);
    }
    key.ok()
}

/// `exists(string $key): bool`.
fn exists(call: &Call) {
    let Some(table) = created(call) else { return };
    let Some(key) = key(call) else { return };
    call.set_bool(table.exists(key));
}

/// `del(string $key): bool`.
fn del(call: &Call) {
    let Some(table) = created(call) else { return };
    let Some(key) = key(call) else { return };
    call.set_bool(table.del(key));
}

/// `count(int $mode = COUNT_NORMAL): int`.
fn count(call: &Call) {
    let Some(table) = table_of(call) else {
        call.set_long(0);
        return;
    };
    let Ok(mode) = call.arity(0, 1).and_then(|()| call.long(1, 0)) else {
        call.set_bool(false);
        return;
    };
    let rows = table.count();
    let count = if mode == 0 {
        rows
    } else {
        rows.wrapping_mul(table.columns().len())
    };
    call.set_long(long(count));
}

/// `getMemorySize(): int` and `getSize(): int`: 0 before `__construct`.
fn get_memory_size(call: &Call) {
    call.set_long(table_of(call).map_or(0, |table| long(table.memory_size())));
}

fn get_size(call: &Call) {
    call.set_long(table_of(call).map_or(0, |table| long(table.size())));
}

/// `stats(): array|false`.
fn stats(call: &Call) {
    let Some(table) = table_of(call) else {
        call.set_bool(false);
        return;
    };
    let stats = table.stats();
    let values = [
        table.count().c_cast::<i64>(),
        stats.conflict_count.c_cast::<i64>(),
        i64::from(stats.conflict_max_level),
        stats.insert_count,
        stats.update_count,
        stats.delete_count,
        i64::from(table.available_slices()),
        i64::from(table.total_slices()),
    ];
    call.set_array();
    for (key, value) in [
        c"num",
        c"conflict_count",
        c"conflict_max_level",
        c"insert_count",
        c"update_count",
        c"delete_count",
        c"available_slice_num",
        c"total_slice_num",
    ]
    .into_iter()
    .zip(values)
    {
        call.add_assoc_long(key, value);
    }
}

fn rewind(call: &Call) {
    let Some(table) = created(call) else { return };
    table.rewind();
    table.forward();
}

fn next(call: &Call) {
    let Some(table) = created(call) else { return };
    table.forward();
}

/// The iterator's current row, unless it is past the end (an empty key).
fn current_row(call: &Call) -> Option<(Rc<SharedTable>, Current)> {
    let table = created(call)?;
    let row = table.current_row()?;
    Some((table, row))
}

fn valid(call: &Call) {
    if let Some((_, row)) = current_row(call) {
        call.set_bool(!row.key().is_empty());
    }
}

fn current(call: &Call) {
    match current_row(call) {
        Some((table, row)) if !row.key().is_empty() => row_array(call, &table, &row),
        Some(_) => call.set_null(),
        None => {}
    }
}

fn key_of(call: &Call) {
    match current_row(call) {
        Some((_, row)) if !row.key().is_empty() => set_bytes(call, row.key()),
        Some(_) => call.set_null(),
        None => {}
    }
}

swoole_rs_zend::handlers! {
    zim_swoole_table___construct => construct;
    zim_swoole_table_column => column;
    zim_swoole_table_create => create;
    zim_swoole_table_destroy => destroy;
    zim_swoole_table_set => set;
    zim_swoole_table_get => get;
    zim_swoole_table_count => count;
    zim_swoole_table_del => del;
    zim_swoole_table_exists => exists;
    zim_swoole_table_incr => incr;
    zim_swoole_table_decr => decr;
    zim_swoole_table_getSize => get_size;
    zim_swoole_table_getMemorySize => get_memory_size;
    zim_swoole_table_stats => stats;
    zim_swoole_table_rewind => rewind;
    zim_swoole_table_valid => valid;
    zim_swoole_table_next => next;
    zim_swoole_table_current => current;
    zim_swoole_table_key => key_of;
}
