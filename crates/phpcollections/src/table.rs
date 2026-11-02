//! PHP `Swoole\Table`: class registration, object handlers and every method.
//! Rows, locks and the shared arena stay the memory module's C++ `TableV2`,
//! which C++ lends as a table of operations; the column records and row
//! values are the memory crate's, called directly.
//!
//! `E_ERROR` and Swoole's fatal errors leave a method by a bailout or exit;
//! where a method can raise one, nothing it holds needs dropping.
use std::{
    ffi::{CStr, c_char, c_void},
    ptr,
    sync::OnceLock,
};
use swoole_rs_memory::{
    swr_v1_memory_records_clear, swr_v1_memory_records_declare, swr_v1_memory_table_clear_value,
    swr_v1_memory_table_get_number, swr_v1_memory_table_get_string, swr_v1_memory_table_set_value,
    table::{Row, row_key},
    table_records::{Column, Records},
};
use swoole_rs_phpstubs::table;
use swoole_rs_zend::{
    Call, Class, Object, bailout,
    sys::{
        _zval_struct__bindgen_ty_1, E_ERROR, E_WARNING, HashPosition, IS_DOUBLE, IS_NULL,
        ZEND_ACC_NOT_SERIALIZABLE, add_assoc_double_ex, add_assoc_long_ex, add_assoc_stringl_ex,
        swr_zend_array_init, swr_zend_string_init, swr_zend_string_release, zend_ce_countable,
        zend_ce_iterator, zend_class_entry, zend_class_implements, zend_hash_get_current_data_ex,
        zend_hash_get_current_key_ex, zend_hash_internal_pointer_reset_ex,
        zend_hash_move_forward_ex, zend_hash_str_find, zend_object, zend_object_handlers,
        zend_string, zend_value, zval,
    },
    throw_constructed_twice, throw_exception, unset_property_deny,
};
use swoole_rs_zend::{method_table, sys as z};

/// `swoole::TableV2`'s operations (`swr_v1_phpcollections_table_ops`).
#[repr(C)]
pub struct TableOps {
    /// `TableV2::make(rows, conflict)` with Zend's string hash; null if it failed.
    make: unsafe extern "C" fn(rows: u32, conflict: f32) -> *mut c_void,
    destroy: unsafe extern "C" fn(table: *mut c_void),
    /// The column records, and the core's `item_size` and `created` they declare against.
    records: unsafe extern "C" fn(
        table: *mut c_void,
        item_size: *mut *mut usize,
        created: *mut *const u8,
    ) -> *mut Records,
    create: unsafe extern "C" fn(table: *mut c_void) -> u8,
    ready: unsafe extern "C" fn(table: *mut c_void) -> u8,
    count: unsafe extern "C" fn(table: *mut c_void) -> usize,
    size: unsafe extern "C" fn(table: *mut c_void) -> usize,
    memory_size: unsafe extern "C" fn(table: *mut c_void) -> usize,
    /// The eight values `stats()` returns, in its order.
    stats: unsafe extern "C" fn(table: *mut c_void, values: *mut i64),
    set: unsafe extern "C" fn(
        table: *mut c_void,
        key: *const c_char,
        length: u16,
        lock: *mut *mut Row,
        flags: *mut i32,
    ) -> *mut Row,
    get: unsafe extern "C" fn(
        table: *mut c_void,
        key: *const c_char,
        length: u16,
        lock: *mut *mut Row,
    ) -> *mut Row,
    del: unsafe extern "C" fn(table: *mut c_void, key: *const c_char, length: u16) -> u8,
    exists: unsafe extern "C" fn(table: *mut c_void, key: *const c_char, length: u16) -> u8,
    unlock: unsafe extern "C" fn(row: *mut Row),
    rewind: unsafe extern "C" fn(table: *mut c_void),
    forward: unsafe extern "C" fn(table: *mut c_void),
    current: unsafe extern "C" fn(table: *mut c_void) -> *mut Row,
    /// `swoole_fatal_error(SW_ERROR_WRONG_OPERATION, "must call constructor first")`.
    unconstructed: unsafe extern "C" fn(),
    /// `php_swoole_fatal_error(level, "%s", message)`.
    fatal: unsafe extern "C" fn(level: i32, message: *const c_char),
    /// `php_swoole_fatal_error(E_WARNING, "key[%s] is too long", key)`.
    key_too_long: unsafe extern "C" fn(key: *const c_char),
    /// `php_swoole_error(E_WARNING, "failed to set('%*s'), unable to allocate memory", length, key)`.
    set_failed: unsafe extern "C" fn(key: *const c_char, length: i32),
    /// `php_swoole_fatal_error(E_WARNING, "column[%s] does not exist", column)`.
    no_column: unsafe extern "C" fn(column: *const c_char),
    /// Swoole's warning log that a string value is too long for its column.
    too_long: unsafe extern "C" fn(row: *mut Row, field: *const c_char, length: usize),
    /// Swoole's warning log of a message.
    log_warning: unsafe extern "C" fn(message: *const c_char),
}

const TYPE_INT: u32 = 1;
const TYPE_FLOAT: u32 = 2;
const TYPE_STRING: u32 = 3;
/// `SW_TABLE_KEY_SIZE`, `SW_TABLE_FLAG_NEW_ROW`, `SW_TABLE_CONFLICT_PROPORTION`,
/// `SW_ERROR_MALLOC_FAIL`.
const KEY_SIZE: usize = 64;
const NEW_ROW: i32 = 1;
const CONFLICT_PROPORTION: f64 = 0.2;
const MALLOC_FAIL: i64 = 501;
/// `HASH_KEY_IS_STRING`, `HASH_KEY_NON_EXISTENT`.
const KEY_IS_STRING: i64 = 1;
const KEY_NON_EXISTENT: i64 = 3;

static OPS: OnceLock<&'static TableOps> = OnceLock::new();
static TABLE: Class = Class::new();
static EXCEPTION: OnceLock<usize> = OnceLock::new();

/// `struct TableObject { TableV2 *ptr; zend_object std; }`.
type TableObject = Object<*mut c_void>;

fn ops() -> &'static TableOps {
    // Methods and handlers run only after MINIT stored the table.
    OPS.get().copied().unwrap_or_else(|| std::process::abort())
}

unsafe extern "C" fn create_object(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: Zend calls `create_object` with this class or a subclass of it;
    // a null table is valid zeroed data.
    unsafe { TableObject::create(&TABLE, ce) }
}

unsafe extern "C" fn free_object(std: *mut zend_object) {
    // SAFETY: Zend frees an object this class created; like upstream, the
    // shared table outlives it (forked workers may use it).
    unsafe { TableObject::dtor(std) };
}

method_table! {
    static METHODS = [
        c"__construct" => zim_swoole_table___construct, &table::arginfo_class_Swoole_Table___construct, z::ZEND_ACC_PUBLIC;
        c"column" => zim_swoole_table_column, &table::arginfo_class_Swoole_Table_column, z::ZEND_ACC_PUBLIC;
        c"create" => zim_swoole_table_create, &table::arginfo_class_Swoole_Table_create, z::ZEND_ACC_PUBLIC;
        c"destroy" => zim_swoole_table_destroy, &table::arginfo_class_Swoole_Table_destroy, z::ZEND_ACC_PUBLIC;
        c"set" => zim_swoole_table_set, &table::arginfo_class_Swoole_Table_set, z::ZEND_ACC_PUBLIC;
        c"get" => zim_swoole_table_get, &table::arginfo_class_Swoole_Table_get, z::ZEND_ACC_PUBLIC;
        c"count" => zim_swoole_table_count, &table::arginfo_class_Swoole_Table_count, z::ZEND_ACC_PUBLIC;
        c"del" => zim_swoole_table_del, &table::arginfo_class_Swoole_Table_del, z::ZEND_ACC_PUBLIC;
        c"delete" => zim_swoole_table_del, &table::arginfo_class_Swoole_Table_del, z::ZEND_ACC_PUBLIC;
        c"exists" => zim_swoole_table_exists, &table::arginfo_class_Swoole_Table_exists, z::ZEND_ACC_PUBLIC;
        c"exist" => zim_swoole_table_exists, &table::arginfo_class_Swoole_Table_exists, z::ZEND_ACC_PUBLIC;
        c"incr" => zim_swoole_table_incr, &table::arginfo_class_Swoole_Table_incr, z::ZEND_ACC_PUBLIC;
        c"decr" => zim_swoole_table_decr, &table::arginfo_class_Swoole_Table_decr, z::ZEND_ACC_PUBLIC;
        c"getSize" => zim_swoole_table_getSize, &table::arginfo_class_Swoole_Table_getSize, z::ZEND_ACC_PUBLIC;
        c"getMemorySize" => zim_swoole_table_getMemorySize, &table::arginfo_class_Swoole_Table_getMemorySize, z::ZEND_ACC_PUBLIC;
        c"stats" => zim_swoole_table_stats, &table::arginfo_class_Swoole_Table_stats, z::ZEND_ACC_PUBLIC;
        c"rewind" => zim_swoole_table_rewind, &table::arginfo_class_Swoole_Table_rewind, z::ZEND_ACC_PUBLIC;
        c"valid" => zim_swoole_table_valid, &table::arginfo_class_Swoole_Table_valid, z::ZEND_ACC_PUBLIC;
        c"next" => zim_swoole_table_next, &table::arginfo_class_Swoole_Table_next, z::ZEND_ACC_PUBLIC;
        c"current" => zim_swoole_table_current, &table::arginfo_class_Swoole_Table_current, z::ZEND_ACC_PUBLIC;
        c"key" => zim_swoole_table_key, &table::arginfo_class_Swoole_Table_key, z::ZEND_ACC_PUBLIC;
    ];
}

/// MINIT: register `Swoole\Table` as `php_swoole_table_minit` did.
/// # Safety
/// Called once, at MINIT, with `swoole_exception_ce` and a static operations table.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpcollections_table_minit(
    exception: *mut zend_class_entry,
    ops: *const TableOps,
) {
    // SAFETY: per the caller, a static table.
    if let Some(ops) = unsafe { ops.as_ref() } {
        OPS.get_or_init(|| ops);
    }
    EXCEPTION.get_or_init(|| exception as usize);
    // SAFETY: MINIT, with a static table.
    unsafe { TABLE.register(c"Swoole\\Table", METHODS.as_ptr(), configure) };
    let entry = TABLE.entry();
    // SAFETY: Zend's interfaces are registered before extensions' MINIT.
    let iterator = unsafe { zend_ce_iterator };
    // SAFETY: as above.
    let countable = unsafe { zend_ce_countable };
    // SAFETY: MINIT, a registered class and a registered interface.
    unsafe { zend_class_implements(entry, 1, iterator) };
    // SAFETY: as above.
    unsafe { zend_class_implements(entry, 1, countable) };
    // SAFETY: MINIT, after registration.
    unsafe { TABLE.property_null(c"size") };
    // SAFETY: as above.
    unsafe { TABLE.property_null(c"memorySize") };
    for (name, value) in [
        (c"TYPE_INT", TYPE_INT),
        (c"TYPE_STRING", TYPE_STRING),
        (c"TYPE_FLOAT", TYPE_FLOAT),
    ] {
        // SAFETY: as above.
        unsafe { TABLE.constant_long(name, value.into()) };
    }
}

/// Not serializable, clone and unset denied, custom object.
fn configure(entry: &mut zend_class_entry, handlers: &mut zend_object_handlers) {
    entry.ce_flags |= ZEND_ACC_NOT_SERIALIZABLE;
    handlers.clone_obj = None;
    handlers.unset_property = Some(unset_property_deny);
    TableObject::install(entry, handlers, create_object, free_object);
}

fn this(call: &Call) -> &'static mut *mut c_void {
    // SAFETY: Table methods are non-static, so `$this` is one of our objects.
    unsafe { TableObject::data(call.this()) }
}

fn fatal(level: u32, message: &CStr) {
    #[expect(clippy::cast_possible_wrap, reason = "E_ERROR and E_WARNING")]
    // SAFETY: C++'s reporter, from a handler.
    bailout::defer(unsafe { bailout::report_level(ops().fatal, level as i32, message) });
}

/// A C++ reporter of Zend's NUL-terminated string bytes; true if a user error
/// handler bailed out (deferred: the method stops).
fn report(reporter: unsafe extern "C" fn(*const c_char), text: &[u8]) -> bool {
    // SAFETY: Zend's string bytes are NUL-terminated.
    let text = unsafe { CStr::from_ptr(text.as_ptr().cast()) };
    // SAFETY: C++'s reporter, from a handler.
    let reported = unsafe { bailout::report(reporter, text) };
    let bailed = reported.is_err();
    bailout::defer(reported);
    bailed
}

/// `table_get_and_check_ptr`: the table, or Swoole's fatal error (which exits).
fn constructed(call: &Call) -> Option<*mut c_void> {
    let table = *this(call);
    if table.is_null() {
        // SAFETY: C++'s fatal-error report.
        unsafe { (ops().unconstructed)() };
        return None;
    }
    Some(table)
}

fn ready(table: *mut c_void) -> bool {
    // SAFETY: a constructed table.
    unsafe { (ops().ready)(table) != 0 }
}

/// `table_get_and_check_ptr2`: also created and not destroyed (an `E_ERROR`).
fn created(call: &Call) -> Option<*mut c_void> {
    let table = constructed(call)?;
    if !ready(table) {
        fatal(E_ERROR, c"table is not created or has been destroyed");
    }
    Some(table)
}

/// The table's column records.
fn records<'a>(table: *mut c_void) -> &'a Records {
    let (mut size, mut flag) = (ptr::null_mut(), ptr::null());
    // SAFETY: a constructed table; writable outputs.
    let records = unsafe { (ops().records)(table, &raw mut size, &raw mut flag) };
    // SAFETY: a constructed table always has its records.
    unsafe { &*records }
}

/// `__construct(int $table_size, float $conflict_proportion = 0.2)`.
fn construct(call: &Call) {
    if !this(call).is_null() {
        // SAFETY: a live `$this`.
        unsafe { throw_constructed_twice(call.this()) };
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
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "upstream passes the zend_long to a uint32_t and the double to a float"
    )]
    // SAFETY: C++'s factory.
    let table = unsafe { (ops().make)(rows as u32, conflict as f32) };
    if table.is_null() {
        let exception = EXCEPTION.get().copied().unwrap_or(0) as *mut zend_class_entry;
        // SAFETY: Swoole's exception class, from MINIT.
        unsafe { throw_exception(exception, c"global memory allocation failure", MALLOC_FAIL) };
        call.set_bool(false);
        return;
    }
    *this(call) = table;
}

/// `column(string $name, int $type, int $size = 0): bool`.
fn column(call: &Call) {
    let Some(table) = constructed(call) else {
        return;
    };
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
    if ready(table) {
        fatal(
            E_WARNING,
            c"unable to add column after table has been created",
        );
        call.set_bool(false);
        return;
    }
    let (mut item_size, mut flag) = (ptr::null_mut(), ptr::null());
    // SAFETY: a constructed table; writable outputs.
    let records = unsafe { (ops().records)(table, &raw mut item_size, &raw mut flag) };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "upstream casts the type to the enum and the size to size_t"
    )]
    // SAFETY: the table's records and core fields, before creation.
    let status = unsafe {
        swr_v1_memory_records_declare(
            records,
            item_size,
            flag,
            name.as_ptr(),
            name.len(),
            kind as u32,
            size as usize,
        )
    };
    if status == -2 {
        // SAFETY: a NUL-terminated message.
        unsafe { (ops().log_warning)(c"Table::add_column(): unknown column type".as_ptr()) };
    }
    call.set_bool(status == 0);
}

/// `create(): bool`.
fn create(call: &Call) {
    let Some(table) = constructed(call) else {
        return;
    };
    let ops = ops();
    // SAFETY: a constructed table.
    if unsafe { (ops.create)(table) } == 0 {
        fatal(E_ERROR, c"unable to allocate memory");
        call.set_bool(false);
        return;
    }
    // SAFETY: a created table.
    let size = unsafe { (ops.size)(table) };
    // SAFETY: as above.
    let memory = unsafe { (ops.memory_size)(table) };
    call.update_property_long(Some(&TABLE), c"size", long(size));
    call.update_property_long(Some(&TABLE), c"memorySize", long(memory));
    call.set_bool(true);
}

#[expect(clippy::cast_possible_wrap, reason = "upstream's (zend_long) casts")]
fn long(value: usize) -> i64 {
    value as i64
}

/// `destroy(): bool`.
fn destroy(call: &Call) {
    let Some(table) = created(call) else { return };
    // SAFETY: the table this object made; nothing uses it after.
    unsafe { (ops().destroy)(table) };
    *this(call) = ptr::null_mut();
    call.set_bool(true);
}

fn is_null(value: *mut zval) -> bool {
    if value.is_null() {
        return true;
    }
    // SAFETY: a live zval.
    let value = unsafe { *value };
    // SAFETY: every zval sets `u1.type_info`.
    let kind = unsafe { value.u1.type_info };
    kind & 0xff == IS_NULL
}

/// `table_set_field_value`: store `value` into a row's column; a new row
/// clears a column its array leaves out or sets to null.
fn store(row: *mut Row, column: &Column, value: *mut zval, initialize: bool) {
    if initialize && is_null(value) {
        // SAFETY: a locked row of this table and one of its columns.
        unsafe { swr_v1_memory_table_clear_value(row, column.index, column.width, column.kind) };
        return;
    }
    match column.kind {
        TYPE_STRING => {
            // SAFETY: a live zval; `zval_get_string`.
            let string = unsafe { swoole_rs_zend::value::get_string(value) };
            // SAFETY: a live string.
            let bytes = unsafe { string_bytes(string) };
            if bytes.len() > column.width as usize - 4 {
                // SAFETY: a locked row; the column's name bytes.
                unsafe { (ops().too_long)(row, column.name.as_ptr().cast(), column.name.len()) };
            }
            // SAFETY: as above; the string is copied (cut to the column).
            unsafe { set_value(row, column, bytes.as_ptr().cast(), bytes.len()) };
            // SAFETY: our reference.
            unsafe { swr_zend_string_release(string) };
        }
        TYPE_FLOAT => {
            // SAFETY: a live zval; `zval_get_double`.
            let number = unsafe { swoole_rs_zend::value::get_double(value) };
            // SAFETY: as above.
            unsafe { set_value(row, column, (&raw const number).cast(), 0) };
        }
        _ => {
            // SAFETY: a live zval; `zval_get_long`.
            let number = unsafe { swoole_rs_zend::value::get_long(value) };
            // SAFETY: as above.
            unsafe { set_value(row, column, (&raw const number).cast(), 0) };
        }
    }
}

/// # Safety
/// A locked row, one of its table's columns, and `length` bytes at `value`
/// (8 for a number).
unsafe fn set_value(row: *mut Row, column: &Column, value: *const c_void, length: usize) {
    // SAFETY: per the caller.
    unsafe {
        swr_v1_memory_table_set_value(row, column.index, column.width, column.kind, value, length);
    }
}

/// `ZSTR_VAL`/`ZSTR_LEN`.
/// # Safety
/// A live string, not released while the result is used.
unsafe fn string_bytes<'a>(string: *mut zend_string) -> &'a [u8] {
    // SAFETY: per the caller.
    let length = unsafe { (*string).len };
    let bytes = string
        .wrapping_byte_add(std::mem::offset_of!(zend_string, val))
        .cast::<u8>();
    // SAFETY: as above; `val` holds `len` bytes.
    unsafe { std::slice::from_raw_parts(bytes, length) }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "upstream passes size_t keys as uint16_t"
)]
fn key_length(key: &[u8]) -> u16 {
    key.len() as u16
}

/// `set(string $key, array $value): bool`.
fn set(call: &Call) {
    let Some(table) = created(call) else { return };
    let parsed = call
        .arity(2, 2)
        .and_then(|()| Ok((call.string(1, b"")?, call.array(2)?)));
    let Ok((key, array)) = parsed else {
        call.set_bool(false);
        return;
    };
    if !ready(table) {
        fatal(E_ERROR, c"the table object does not exist");
        call.set_bool(false);
        return;
    }
    let ops = ops();
    if key.len() >= KEY_SIZE && report(ops.key_too_long, key) {
        call.set_bool(false);
        return;
    }
    let (mut lock, mut flags) = (ptr::null_mut(), 0);
    // SAFETY: a created table; writable outputs.
    let row = unsafe {
        (ops.set)(
            table,
            key.as_ptr().cast(),
            key_length(key),
            &raw mut lock,
            &raw mut flags,
        )
    };
    if row.is_null() {
        // SAFETY: the bucket lock `set` took.
        unsafe { (ops.unlock)(lock) };
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_possible_wrap,
            reason = "upstream's (int) keylen"
        )]
        let length = key.len() as i32;
        // SAFETY: the key's bytes, as `%*s` reads them.
        bailout::defer(unsafe {
            bailout::report_length(ops.set_failed, key.as_ptr().cast(), length)
        });
        call.set_bool(false);
        return;
    }
    let records = records(table);
    // SAFETY: an array argument.
    let hash = unsafe { swoole_rs_zend::value::table(array) };
    if flags & NEW_ROW != 0 {
        for column in records.columns() {
            // SAFETY: a live array; Zend reads `len` bytes of the name.
            let value =
                unsafe { zend_hash_str_find(hash, column.name.as_ptr().cast(), column.name.len()) };
            store(row, &column, value, true);
        }
    } else {
        let mut position: HashPosition = 0;
        // SAFETY: a live array and a writable position.
        unsafe { zend_hash_internal_pointer_reset_ex(hash, &raw mut position) };
        loop {
            let (mut name, mut index) = (ptr::null_mut(), 0);
            // SAFETY: as above.
            let kind = unsafe {
                zend_hash_get_current_key_ex(hash, &raw mut name, &raw mut index, &raw mut position)
            };
            // An int or a zend_hash_key_type, by PHP version.
            let kind = i64::from(kind);
            if kind == KEY_NON_EXISTENT {
                break;
            }
            if kind == KEY_IS_STRING {
                // SAFETY: a key of the live array.
                let name = unsafe { string_bytes(name) };
                if let Some(column) = records.column(name) {
                    // SAFETY: as above.
                    let value = unsafe { zend_hash_get_current_data_ex(hash, &raw mut position) };
                    store(row, &column, value, false);
                }
            }
            // SAFETY: as above.
            unsafe { zend_hash_move_forward_ex(hash, &raw mut position) };
        }
    }
    // SAFETY: the row lock `set` took.
    unsafe { (ops.unlock)(lock) };
    call.set_bool(true);
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
    let by = call.zval(3);
    let ops = ops();
    let (mut lock, mut flags) = (ptr::null_mut(), 0);
    // SAFETY: a created table; writable outputs.
    let row = unsafe {
        (ops.set)(
            table,
            key.as_ptr().cast(),
            key_length(key),
            &raw mut lock,
            &raw mut flags,
        )
    };
    // SAFETY: the bucket lock `set` took (released before any warning).
    let unlock = || unsafe { (ops.unlock)(lock) };
    if row.is_null() {
        unlock();
        fatal(E_WARNING, c"unable to allocate memory");
        call.set_bool(false);
        return;
    }
    let records = records(table);
    let Some(column) = records.column(name) else {
        unlock();
        report(ops.no_column, name);
        call.set_bool(false);
        return;
    };
    if flags & NEW_ROW != 0 {
        // SAFETY: the table's records and a new, locked row.
        unsafe { swr_v1_memory_records_clear(records, row) };
    }
    let bits = || {
        let mut bits = 0u64;
        // SAFETY: a locked row; 8 bytes of a number column.
        unsafe { swr_v1_memory_table_get_number(row, column.index, (&raw mut bits).cast()) };
        bits
    };
    match column.kind {
        TYPE_STRING => {
            unlock();
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
            let by = if by.is_null() {
                1.0
            } else {
                // SAFETY: a passed argument.
                unsafe { swoole_rs_zend::value::get_double(by) }
            };
            let current = f64::from_bits(bits());
            let value = if subtract { current - by } else { current + by };
            // SAFETY: a locked row and 8 bytes.
            unsafe { set_value(row, &column, (&raw const value).cast(), 0) };
            unlock();
            set_double(call, value);
        }
        _ => {
            let by = if by.is_null() {
                1
            } else {
                // SAFETY: a passed argument.
                unsafe { swoole_rs_zend::value::get_long(by) }
            };
            #[expect(clippy::cast_possible_wrap, reason = "the column's 8 bytes as a long")]
            let current = bits() as i64;
            let value = if subtract {
                current.wrapping_sub(by)
            } else {
                current.wrapping_add(by)
            };
            // SAFETY: a locked row and 8 bytes.
            unsafe { set_value(row, &column, (&raw const value).cast(), 0) };
            unlock();
            call.set_long(value);
        }
    }
}

fn set_double(call: &Call, value: f64) {
    // SAFETY: all-zero bytes are a valid zval.
    let mut zv: zval = unsafe { std::mem::zeroed() };
    zv.value = zend_value { dval: value };
    zv.u1 = _zval_struct__bindgen_ty_1 {
        type_info: IS_DOUBLE,
    };
    call.set_value(zv);
}

fn incr(call: &Call) {
    update(call, false);
}

fn decr(call: &Call) {
    update(call, true);
}

/// A column's value in a locked row.
enum Value<'a> {
    Long(i64),
    Double(f64),
    Bytes(&'a [u8]),
}

fn read<'a>(row: *mut Row, column: &Column) -> Value<'a> {
    match column.kind {
        TYPE_STRING => {
            let (mut text, mut length) = (ptr::null_mut(), 0);
            // SAFETY: a locked row; writable outputs.
            unsafe {
                swr_v1_memory_table_get_string(
                    row,
                    column.index,
                    column.width,
                    &raw mut text,
                    &raw mut length,
                );
            }
            if text.is_null() {
                return Value::Bytes(&[]);
            }
            // SAFETY: `length` bytes of the row's string, valid while it is locked.
            Value::Bytes(unsafe { std::slice::from_raw_parts(text, length as usize) })
        }
        kind => {
            let mut bits = 0u64;
            // SAFETY: a locked row; 8 bytes of a number column.
            unsafe { swr_v1_memory_table_get_number(row, column.index, (&raw mut bits).cast()) };
            if kind == TYPE_FLOAT {
                Value::Double(f64::from_bits(bits))
            } else {
                #[expect(clippy::cast_possible_wrap, reason = "the column's 8 bytes as a long")]
                Value::Long(bits as i64)
            }
        }
    }
}

/// `table_row2array`: every column of a locked row, into `target`.
fn row_array(records: &Records, row: *mut Row, target: *mut zval) {
    // SAFETY: a writable zval.
    unsafe { swr_zend_array_init(target) };
    for column in records.columns() {
        let (name, length) = (column.name.as_ptr().cast::<c_char>(), column.name.len());
        match read(row, &column) {
            // SAFETY: an array; Zend copies the key and the bytes.
            Value::Long(value) => unsafe { add_assoc_long_ex(target, name, length, value) },
            // SAFETY: as above.
            Value::Double(value) => unsafe { add_assoc_double_ex(target, name, length, value) },
            // SAFETY: as above.
            Value::Bytes(bytes) => unsafe {
                add_assoc_stringl_ex(
                    target,
                    name,
                    length,
                    bytes.as_ptr().cast_mut().cast(),
                    bytes.len(),
                );
            },
        }
    }
}

fn set_bytes(call: &Call, bytes: &[u8]) {
    // SAFETY: Zend copies the bytes.
    let string = unsafe { swr_zend_string_init(bytes.as_ptr().cast(), bytes.len()) };
    // SAFETY: a new string the return value takes over.
    unsafe { call.set_string(string) };
}

/// `get(string $key, ?string $field = null): mixed`.
fn get(call: &Call) {
    let Some(table) = created(call) else { return };
    let parsed = call.arity(1, 2).and_then(|()| {
        let key = call.string(1, b"")?;
        let field = if is_null(call.zval(2)) {
            None
        } else {
            call.str(2)?
        };
        Ok((key, field))
    });
    let Ok((key, field)) = parsed else {
        call.set_bool(false);
        return;
    };
    let ops = ops();
    let mut lock = ptr::null_mut();
    // SAFETY: a created table; a writable output.
    let row = unsafe { (ops.get)(table, key.as_ptr().cast(), key_length(key), &raw mut lock) };
    let records = records(table);
    if row.is_null() {
        call.set_bool(false);
    } else if let Some(field) = field {
        // SAFETY: a live string argument.
        match records.column(unsafe { string_bytes(field) }) {
            None => call.set_bool(false),
            Some(column) => match read(row, &column) {
                Value::Long(value) => call.set_long(value),
                Value::Double(value) => set_double(call, value),
                Value::Bytes(bytes) => set_bytes(call, bytes),
            },
        }
    } else {
        row_array(records, row, call.return_value());
    }
    // SAFETY: the bucket lock `get` took.
    unsafe { (ops.unlock)(lock) };
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
    // SAFETY: a created table.
    let found = unsafe { (ops().exists)(table, key.as_ptr().cast(), key_length(key)) };
    call.set_bool(found != 0);
}

/// `del(string $key): bool`.
fn del(call: &Call) {
    let Some(table) = created(call) else { return };
    let Some(key) = key(call) else { return };
    // SAFETY: a created table.
    let deleted = unsafe { (ops().del)(table, key.as_ptr().cast(), key_length(key)) };
    call.set_bool(deleted != 0);
}

/// `count(int $mode = COUNT_NORMAL): int`.
fn count(call: &Call) {
    let table = *this(call);
    if table.is_null() {
        call.set_long(0);
        return;
    }
    let Ok(mode) = call.arity(0, 1).and_then(|()| call.long(1, 0)) else {
        call.set_bool(false);
        return;
    };
    // SAFETY: a constructed table.
    let rows = unsafe { (ops().count)(table) };
    let count = if mode == 0 {
        rows
    } else {
        rows.wrapping_mul(records(table).columns().count())
    };
    call.set_long(long(count));
}

/// `getMemorySize(): int` and `getSize(): int`: 0 before `__construct`.
fn get_memory_size(call: &Call) {
    let table = *this(call);
    call.set_long(if table.is_null() {
        0
    } else {
        // SAFETY: a constructed table.
        long(unsafe { (ops().memory_size)(table) })
    });
}

fn get_size(call: &Call) {
    let table = *this(call);
    call.set_long(if table.is_null() {
        0
    } else {
        // SAFETY: a constructed table.
        long(unsafe { (ops().size)(table) })
    });
}

/// `stats(): array|false`.
fn stats(call: &Call) {
    let table = *this(call);
    if table.is_null() {
        call.set_bool(false);
        return;
    }
    let mut values = [0i64; 8];
    // SAFETY: a constructed table; eight writable values.
    unsafe { (ops().stats)(table, values.as_mut_ptr()) };
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
    let ops = ops();
    // SAFETY: a created table.
    unsafe { (ops.rewind)(table) };
    // SAFETY: as above.
    unsafe { (ops.forward)(table) };
}

fn next(call: &Call) {
    let Some(table) = created(call) else { return };
    // SAFETY: a created table.
    unsafe { (ops().forward)(table) };
}

/// The iterator's current row and its key (empty past the end).
fn current_row<'a>(table: *mut c_void) -> (*mut Row, &'a [u8]) {
    // SAFETY: a created table; its iterator's row buffer.
    let row = unsafe { (ops().current)(table) };
    // SAFETY: the iterator's copy of a row, which only the iterator rewrites.
    (row, unsafe { row_key(row) })
}

fn valid(call: &Call) {
    let Some(table) = created(call) else { return };
    call.set_bool(!current_row(table).1.is_empty());
}

fn current(call: &Call) {
    let Some(table) = created(call) else { return };
    let (row, key) = current_row(table);
    if key.is_empty() {
        call.set_null();
        return;
    }
    row_array(records(table), row, call.return_value());
}

fn key_of(call: &Call) {
    let Some(table) = created(call) else { return };
    let key = current_row(table).1;
    if key.is_empty() {
        call.set_null();
        return;
    }
    set_bytes(call, key);
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
