//! PHP `Swoole\Table`: class registration, object handlers and every method,
//! over the memory crate's table (its header from Swoole's global pool, its
//! arena shared memory), hashing keys with Zend's string hash.
//!
//! `E_ERROR` and Swoole's fatal errors leave a method by a bailout or exit;
//! where a method can raise one, nothing it holds needs dropping.
use std::{
    ffi::{CStr, c_char, c_int},
    ptr,
};
use swoole_rs_memory::{
    members::{swr_table_create, swr_table_destroy, swr_table_make},
    table::{
        Column, NEW_ROW, Row, TYPE_FLOAT, TYPE_INT, TYPE_STRING, Table, clear_value, get_number,
        get_string, row_key, unlock,
    },
};
use swoole_rs_phpstubs::table;
use swoole_rs_zend::{
    Call, Class, Message, Minit, Object, Owned, bailout,
    sys::{
        E_ERROR, E_WARNING, HashPosition, IS_NULL, ZEND_ACC_NOT_SERIALIZABLE, add_assoc_double_ex,
        add_assoc_long_ex, add_assoc_stringl_ex, swr_zend_array_init, swr_zend_string_init,
        swr_zend_string_release, zend_ce_countable, zend_ce_iterator, zend_class_entry,
        zend_class_implements, zend_hash_get_current_data_ex, zend_hash_get_current_key_ex,
        zend_hash_internal_pointer_reset_ex, zend_hash_move_forward_ex, zend_hash_str_find,
        zend_object, zend_object_handlers, zend_string, zend_string_hash_func, zval,
    },
    unset_property_deny,
};
use swoole_rs_zend::{method_table, sys as z};

/// `SW_TABLE_KEY_SIZE`, `SW_TABLE_CONFLICT_PROPORTION`.
const KEY_SIZE: usize = 64;
const CONFLICT_PROPORTION: f64 = 0.2;
/// `SW_ERROR_MALLOC_FAIL`, `SW_ERROR_PHP_FATAL_ERROR`, `SW_ERROR_WRONG_OPERATION`,
/// `SW_ERROR_PHP_RUNTIME_NOTICE`.
const MALLOC_FAIL: i64 = 501;
const PHP_FATAL_ERROR: c_int = 503;
const WRONG_OPERATION: c_int = 509;
const PHP_RUNTIME_NOTICE: c_int = 510;
/// `HASH_KEY_IS_STRING`, `HASH_KEY_NON_EXISTENT`.
const KEY_IS_STRING: i64 = 1;
const KEY_NON_EXISTENT: i64 = 3;

static TABLE: Class = Class::new();

/// `struct TableObject { Table *ptr; zend_object std; }`.
type TableObject = Object<*mut Table>;

/// `zend_string_hash_val` of the PHP string whose bytes `key` points into
/// (every key is one), as upstream's `set_hash_func` hook did.
unsafe extern "C" fn zend_hash(key: *const c_char, _length: usize) -> u64 {
    #[expect(
        clippy::cast_ptr_alignment,
        reason = "`val` follows the string's aligned header"
    )]
    let string = key
        .wrapping_byte_sub(std::mem::offset_of!(zend_string, val))
        .cast::<zend_string>()
        .cast_mut();
    // SAFETY: `key` is a live zend_string's `val`.
    let cached = unsafe { (*string).h };
    if cached != 0 {
        return cached;
    }
    // SAFETY: as above; Zend caches the hash on the string.
    unsafe { zend_string_hash_func(string) }
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

/// MINIT: register `Swoole\Table` as upstream's `php_swoole_table_minit` did.
/// # Safety
/// Called once, at MINIT.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn php_swoole_table_minit(module: c_int) {
    // SAFETY: Zend's MINIT of this module, on this thread.
    let minit = unsafe { Minit::new(module) };
    minit.register(&TABLE, c"Swoole\\Table", &METHODS.0, configure);
    let entry = TABLE.entry();
    // SAFETY: Zend's interfaces are registered before extensions' MINIT.
    let iterator = unsafe { zend_ce_iterator };
    // SAFETY: as above.
    let countable = unsafe { zend_ce_countable };
    // SAFETY: MINIT, a registered class and a registered interface.
    unsafe { zend_class_implements(entry, 1, iterator) };
    // SAFETY: as above.
    unsafe { zend_class_implements(entry, 1, countable) };
    minit.property_null(&TABLE, c"size", z::ZEND_ACC_PUBLIC);
    minit.property_null(&TABLE, c"memorySize", z::ZEND_ACC_PUBLIC);
    for (name, value) in [
        (c"TYPE_INT", TYPE_INT),
        (c"TYPE_STRING", TYPE_STRING),
        (c"TYPE_FLOAT", TYPE_FLOAT),
    ] {
        minit.class_constant_long(&TABLE, name, value.into());
    }
}

/// Not serializable, clone and unset denied, custom object.
fn configure(entry: &mut zend_class_entry, handlers: &mut zend_object_handlers) {
    entry.ce_flags |= ZEND_ACC_NOT_SERIALIZABLE;
    handlers.clone_obj = None;
    handlers.unset_property = Some(unset_property_deny);
    TableObject::install(entry, handlers, create_object, free_object);
}

fn this(call: &Call) -> &'static mut *mut Table {
    // SAFETY: Table methods are non-static, so `$this` is one of our objects.
    unsafe { TableObject::data(call.this()) }
}

/// `php_swoole_fatal_error(level, "%s", message)`: always reported.
fn fatal(level: u32, message: &CStr) {
    swoole_rs_corebase::log::set_last_error(PHP_FATAL_ERROR);
    #[expect(clippy::cast_possible_wrap, reason = "E_ERROR and E_WARNING")]
    bailout::defer(swoole_rs_zend::report(level as i32, message));
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
    #[expect(clippy::cast_possible_wrap, reason = "E_WARNING")]
    let reported = swoole_rs_zend::report(E_WARNING as i32, message.as_cstr());
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
        #[expect(clippy::cast_possible_wrap, reason = "E_WARNING")]
        bailout::defer(swoole_rs_zend::report(E_WARNING as i32, message.as_cstr()));
    }
}

/// The table this object made, if any.
fn table_of(call: &Call) -> Option<&'static mut Table> {
    // SAFETY: a table `make` returned stays valid until `destroy` nulls the pointer.
    unsafe { this(call).as_mut() }
}

/// `table_get_and_check_ptr`: the table, or Swoole's fatal error (which exits).
fn constructed(call: &Call) -> Option<&'static mut Table> {
    let table = table_of(call);
    if table.is_none() {
        // SAFETY: Swoole's fatal error, from a handler.
        unsafe { swoole_rs_corebase::fatal_error(WRONG_OPERATION, c"must call constructor first") };
    }
    table
}

/// `table_get_and_check_ptr2`: also created and not destroyed (an `E_ERROR`).
fn created(call: &Call) -> Option<&'static mut Table> {
    let table = constructed(call)?;
    if !table.ready() {
        // The deferred bailout ends the method here, as upstream's fatal does.
        fatal(E_ERROR, c"table is not created or has been destroyed");
        return None;
    }
    Some(table)
}

/// The key as upstream passes it: its length cut to a `uint16_t`.
fn table_key(key: &[u8]) -> &[u8] {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "upstream passes size_t keys as uint16_t"
    )]
    let length = key.len() as u16;
    key.get(..usize::from(length)).unwrap_or(key)
}

/// `__construct(int $table_size, float $conflict_proportion = 0.2)`.
fn construct(call: &Call) {
    if !this(call).is_null() {
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
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "upstream passes the zend_long to a uint32_t and the double to a float"
    )]
    let table = swr_table_make(rows as u32, conflict as f32);
    // SAFETY: a made table, or null.
    if let Some(made) = unsafe { table.as_mut() } {
        made.set_hash(zend_hash);
    } else {
        let exception = swoole_rs_phpswoole::exception();
        call.throw(exception, c"global memory allocation failure", MALLOC_FAIL);
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
    if table.ready() {
        fatal(
            E_WARNING,
            c"unable to add column after table has been created",
        );
        call.set_bool(false);
        return;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "upstream casts the type to the enum and the size to size_t"
    )]
    call.set_bool(table.add_column(name, kind as u32, size as usize));
}

/// `create(): bool`.
fn create(call: &Call) {
    let Some(table) = constructed(call) else {
        return;
    };
    // SAFETY: a constructed table.
    if unsafe { !swr_table_create(table) } {
        fatal(E_ERROR, c"unable to allocate memory");
        call.set_bool(false);
        return;
    }
    call.update_property_long(Some(&TABLE), c"size", long(table.size()));
    call.update_property_long(Some(&TABLE), c"memorySize", long(table.memory_size()));
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
    unsafe { swr_table_destroy(table) };
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
fn store(table: &Table, row: *mut Row, column: &Column, value: *mut zval, initialize: bool) {
    if initialize && is_null(value) {
        // SAFETY: a locked row of this table and one of its columns.
        unsafe { clear_value(row, column) };
        return;
    }
    match column.kind {
        TYPE_STRING => {
            // SAFETY: a live zval; `zval_get_string`.
            let string = unsafe { swoole_rs_zend::value::get_string(value) };
            // SAFETY: a live string.
            let bytes = unsafe { string_bytes(string) };
            // SAFETY: as above; the string is copied (cut to the column, with a warning).
            unsafe { table.set_value(row, column, bytes.as_ptr(), bytes.len()) };
            // SAFETY: our reference.
            unsafe { swr_zend_string_release(string) };
        }
        TYPE_FLOAT => {
            // SAFETY: a live zval; `zval_get_double`.
            let number = unsafe { swoole_rs_zend::value::get_double(value) };
            // SAFETY: as above.
            unsafe { table.set_value(row, column, (&raw const number).cast(), 0) };
        }
        _ => {
            // SAFETY: a live zval; `zval_get_long`.
            let number = unsafe { swoole_rs_zend::value::get_long(value) };
            // SAFETY: as above.
            unsafe { table.set_value(row, column, (&raw const number).cast(), 0) };
        }
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
    if !table.ready() {
        fatal(E_ERROR, c"the table object does not exist");
        call.set_bool(false);
        return;
    }
    if key.len() >= KEY_SIZE && report(b"key[", key, b"] is too long") {
        call.set_bool(false);
        return;
    }
    let (mut lock, mut flags) = (ptr::null_mut(), 0);
    let row = table.set(table_key(key), &mut lock, Some(&mut flags));
    if row.is_null() {
        // SAFETY: the bucket lock `set` took.
        unsafe { unlock(lock) };
        set_failed(key);
        call.set_bool(false);
        return;
    }
    // SAFETY: an array argument.
    let hash = unsafe { swoole_rs_zend::value::table(array) };
    if flags & NEW_ROW != 0 {
        for column in table.columns() {
            let name = column.name();
            // SAFETY: a live array; Zend reads `len` bytes of the name.
            let value = unsafe { zend_hash_str_find(hash, name.as_ptr().cast(), name.len()) };
            store(table, row, column, value, true);
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
                if let Some(column) = table.column(name) {
                    // SAFETY: as above.
                    let value = unsafe { zend_hash_get_current_data_ex(hash, &raw mut position) };
                    store(table, row, column, value, false);
                }
            }
            // SAFETY: as above.
            unsafe { zend_hash_move_forward_ex(hash, &raw mut position) };
        }
    }
    // SAFETY: the row lock `set` took.
    unsafe { unlock(lock) };
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
    let (mut lock, mut flags) = (ptr::null_mut(), 0);
    let row = table.set(table_key(key), &mut lock, Some(&mut flags));
    // SAFETY: the bucket lock `set` took (released before any warning).
    let unlock = || unsafe { unlock(lock) };
    if row.is_null() {
        unlock();
        fatal(E_WARNING, c"unable to allocate memory");
        call.set_bool(false);
        return;
    }
    let Some(column) = table.column(name) else {
        unlock();
        report(b"column[", name, b"] does not exist");
        call.set_bool(false);
        return;
    };
    if flags & NEW_ROW != 0 {
        // SAFETY: a new, locked row of this table.
        unsafe { table.clear_row(row) };
    }
    // SAFETY: a locked row; 8 bytes of a number column.
    let bits = || unsafe { get_number(row, column) };
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
            unsafe { table.set_value(row, column, (&raw const value).cast(), 0) };
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
            unsafe { table.set_value(row, column, (&raw const value).cast(), 0) };
            unlock();
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

/// A column's value in a locked row.
enum Value<'a> {
    Long(i64),
    Double(f64),
    Bytes(&'a [u8]),
}

fn read<'a>(row: *mut Row, column: &Column) -> Value<'a> {
    match column.kind {
        // SAFETY: a locked row (or the iterator's copy); valid while it is.
        TYPE_STRING => Value::Bytes(unsafe { get_string(row, column) }),
        kind => {
            // SAFETY: a locked row; 8 bytes of a number column.
            let bits = unsafe { get_number(row, column) };
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
fn row_array(table: &Table, row: *mut Row, target: *mut zval) {
    // SAFETY: a writable zval.
    unsafe { swr_zend_array_init(target) };
    for column in table.columns() {
        let (name, length) = (swoole_rs_zend::key_ptr(column.name()), column.name().len());
        match read(row, column) {
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
    let mut lock = ptr::null_mut();
    let row = table.get(table_key(key), &mut lock);
    if row.is_null() {
        call.set_bool(false);
    } else if let Some(field) = field {
        // SAFETY: a live string argument.
        match table.column(unsafe { string_bytes(field) }) {
            None => call.set_bool(false),
            Some(column) => match read(row, column) {
                Value::Long(value) => call.set_long(value),
                Value::Double(value) => set_double(call, value),
                Value::Bytes(bytes) => set_bytes(call, bytes),
            },
        }
    } else {
        row_array(table, row, call.return_value());
    }
    // SAFETY: the bucket lock `get` took.
    unsafe { unlock(lock) };
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
    call.set_bool(table.exists(table_key(key)));
}

/// `del(string $key): bool`.
fn del(call: &Call) {
    let Some(table) = created(call) else { return };
    let Some(key) = key(call) else { return };
    call.set_bool(table.del(table_key(key)));
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
    #[expect(
        clippy::cast_possible_wrap,
        reason = "upstream's (zend_long) casts of its counters"
    )]
    let values = [
        table.count() as i64,
        stats.conflict_count as i64,
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

/// The iterator's current row and its key (empty past the end).
fn current_row<'a>(table: &Table) -> (*mut Row, &'a [u8]) {
    let row = table.current();
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
    row_array(table, row, call.return_value());
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
