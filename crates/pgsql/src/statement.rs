//! PostgreSQL statement fetching and column metadata.
use crate::driver::{Handle, Res};
use pq_sys::Oid;
use std::{
    cell::Cell,
    ffi::{CStr, CString, c_int},
};
use swoole_rs_corebase::{CCast, sys::Text};
use swoole_rs_zend::sys::{
    PHP_VERSION_ID, pdo_param_type, pdo_param_type_PDO_PARAM_BOOL as BOOL,
    pdo_param_type_PDO_PARAM_INT as INT, pdo_param_type_PDO_PARAM_LOB as LOB,
    pdo_param_type_PDO_PARAM_STR as STR,
};
use swoole_rs_zend::{
    OwnedArray,
    bailout::Bailout,
    pdo::{Dbh, Slot, Stmt},
};
use swoole_rs_zend::{ZString, sys::pdo_column_data};

/// PostgreSQL's built-in OIDs with a known native label. The statement looks up
/// other labels in `pg_type`. PHP 8.2 and 8.3 also look up the floating-point types.
fn column_type(oid: Oid) -> (pdo_param_type, Option<&'static CStr>) {
    match oid {
        16 => (BOOL, Some(c"bool")),
        17 => (LOB, Some(c"bytea")),
        20 => (INT, Some(c"int8")),
        21 => (INT, Some(c"int2")),
        23 => (INT, Some(c"int4")),
        25 => (STR, Some(c"text")),
        26 => (LOB, None),
        700 if PHP_VERSION_ID >= 80400 => (STR, Some(c"float4")),
        701 if PHP_VERSION_ID >= 80400 => (STR, Some(c"float8")),
        1043 => (STR, Some(c"varchar")),
        1082 => (STR, Some(c"date")),
        1114 => (STR, Some(c"timestamp")),
        _ => (STR, None),
    }
}

fn column_meta(handle: &Handle, oid: Oid, table: Oid) -> Result<OwnedArray, Bailout> {
    let mut meta = OwnedArray::default();
    meta.insert_long(b"pgsql:oid", i64::from(oid));
    meta.insert_long(b"pgsql:table_oid", i64::from(table));
    // The bundled driver's table lookup formats Oid with %d; its type lookup uses %u.
    let table_query = swoole_rs_zend::message!(
        "SELECT RELNAME FROM PG_CLASS WHERE OID={}",
        table.cast_signed()
    );
    if let Some(name) = handle.catalogue_name(table_query.as_cstr())? {
        meta.insert_bytes(b"table", Some(name.as_bytes()));
    }
    let (kind, label) = column_type(oid);
    if let Some(label) = label {
        meta.insert_bytes(b"native_type", Some(label.to_bytes()));
    } else {
        let query = swoole_rs_zend::message!("SELECT TYPNAME FROM PG_TYPE WHERE OID={oid}");
        if let Some(name) = handle.catalogue_name(query.as_cstr())? {
            meta.insert_bytes(b"native_type", Some(name.as_bytes()));
        }
    }
    meta.insert_long(b"pdo_type", i64::from(kind));
    Ok(meta)
}

unsafe extern "C" {
    safe fn PQfsize(result: Res<'_>, column: c_int) -> c_int;
    safe fn PQfmod(result: Res<'_>, column: c_int) -> c_int;
    safe fn PQftable(result: Res<'_>, column: c_int) -> Oid;
    safe fn PQftype(result: Res<'_>, column: c_int) -> Oid;
}

/// Decode PostgreSQL's text-format result. PHP 8.2 and 8.3 return floats as strings.
#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_scalar_column(oid: Oid, bytes: std::io::IoSlice<'_>, output: &Slot) {
    use swoole_rs_zend::Owned;
    let text = || std::str::from_utf8(&bytes).unwrap_or_default();
    let value = match oid {
        16 => Owned::bool(bytes.first() == Some(&b't')),
        20 | 21 | 23 => Owned::long(text().parse().unwrap_or_default()),
        700 | 701 if PHP_VERSION_ID >= 80400 => Owned::double(text().parse().unwrap_or_default()),
        _ => Owned::string(ZString::new(&bytes)),
    };
    output.set(value);
}

/// Fill PDO's column description.
#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_describe_column(
    result: Res<'_>,
    column: c_int,
    desc: &mut pdo_column_data,
) -> bool {
    unsafe extern "C" {
        // The result owns the name, copied before this function returns.
        safe fn PQfname(result: Res<'_>, column: c_int) -> Option<Text<'_>>;
    }
    let Some(name) = PQfname(result, column) else {
        return false;
    };
    desc.name = ZString::new(name.get().to_bytes()).into_raw();
    desc.maxlen = PQfsize(result, column).c_cast();
    desc.precision = PQfmod(result, column).c_cast();
    true
}

swoole_rs_zend::exports! {
/// The metadata for a live statement column, returned to PDO.
#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_column_meta(handle: &Handle, result: Res<'_>, column: c_int, output: &Slot) -> std::ffi::c_int {
    match column_meta(handle, PQftype(result, column), PQftable(result, column)) {
        Ok(meta) => { output.set(meta.into_owned()); 1 }
        Err(error) => { error.defer(); -1 }
    }
}
}

/// Fetch the next batch of a PHP 8.5 single-row result.
fn fetch_unbuffered(
    dbh: &Dbh,
    stmt: &Stmt,
    result: &Cell<Option<crate::PgResult>>,
    running: &Cell<bool>,
) -> bool {
    let Some(handle) = crate::driver::handle(dbh) else {
        return false;
    };
    let conn = crate::driver::pdo_pgsql_server(handle);
    if let Some(previous) = result.take() {
        crate::PQclear(previous);
    }
    let Some(rows) = crate::driver::statement_result(dbh, stmt, result, crate::PQgetResult(conn))
    else {
        return false;
    };
    stmt.set_row_count(i64::from(rows));
    if rows == 0 {
        pdo_pgsql_finish_result(std::ptr::NonNull::new(conn.0), result, false);
        running.set(false);
    }
    true
}

/// Advance in the buffered result or issue FETCH for a scrollable cursor.
#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_fetch_row(
    dbh: &Dbh,
    stmt: &Stmt,
    result: &Cell<Option<crate::PgResult>>,
    cursor: Option<Text<'_>>,
    orientation: swoole_rs_zend::sys::pdo_fetch_orientation,
    offset: swoole_rs_zend::sys::zend_long,
    row: &Cell<c_int>,
    running: Option<&Cell<bool>>,
) -> bool {
    use swoole_rs_zend::sys::{
        pdo_fetch_orientation_PDO_FETCH_ORI_ABS as ABS,
        pdo_fetch_orientation_PDO_FETCH_ORI_FIRST as FIRST,
        pdo_fetch_orientation_PDO_FETCH_ORI_LAST as LAST,
        pdo_fetch_orientation_PDO_FETCH_ORI_NEXT as NEXT,
        pdo_fetch_orientation_PDO_FETCH_ORI_PRIOR as PRIOR,
        pdo_fetch_orientation_PDO_FETCH_ORI_REL as REL,
    };
    let Some(cursor) = cursor else {
        if i64::from(row.get()) >= stmt.row_count() {
            let Some(running) = running.filter(|running| running.get()) else {
                return false;
            };
            if !fetch_unbuffered(dbh, stmt, result, running) {
                return false;
            }
            row.set(0);
            if !running.get() {
                return false;
            }
        }
        let Some(next) = row.get().checked_add(1) else {
            return false;
        };
        row.set(next);
        return true;
    };
    let Some(handle) = crate::driver::handle(dbh) else {
        return false;
    };
    let direction = match orientation {
        NEXT => "NEXT",
        PRIOR => "BACKWARD",
        FIRST => "FIRST",
        LAST => "LAST",
        ABS => "ABSOLUTE",
        REL => "RELATIVE",
        _ => return false,
    };
    let mut query = if matches!(orientation, ABS | REL) {
        swoole_rs_zend::message!("FETCH {direction} {offset} FROM ")
    } else {
        swoole_rs_zend::message!("FETCH {direction} FROM ")
    };
    query.push(cursor.get().to_bytes());
    if let Some(previous) = result.take() {
        crate::PQclear(previous);
    }
    let fetched = crate::driver::statement_result(dbh, stmt, result, handle.exec(query.as_cstr()))
        .is_some_and(|rows| rows > 0);
    if fetched {
        row.set(1);
    }
    fetched
}

/// Close the driver-generated server cursor, discarding its command result.
#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_close_cursor(handle: &Handle, cursor: Text<'_>) {
    let mut query = swoole_rs_zend::message!("CLOSE ");
    query.push(cursor.get().to_bytes());
    if let Some(result) = handle.exec(query.as_cstr()) {
        crate::PQclear(result);
    }
}

swoole_rs_zend::exports! {
/// Declare the cursor and fetch its column descriptions without advancing it.
#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_open_cursor(
    dbh: &Dbh,
    stmt: &Stmt,
    result: &Cell<Option<crate::PgResult>>,
    cursor: Text<'_>,
    prepared: &Cell<bool>,
    sql: Text<'_>,
) -> bool {
    let Some(handle) = crate::driver::handle(dbh) else {
        return false;
    };
    if prepared.get() {
        pdo_pgsql_close_cursor(handle, cursor);
    }
    let mut declared = None;
    {
        let parts: &[&[u8]] = &[
            b"DECLARE ",
            cursor.get().to_bytes(),
            b" SCROLL CURSOR WITH HOLD FOR ",
            sql.get().to_bytes(),
        ];
        let Ok(query) = CString::new(parts.concat()) else {
            return false;
        };
        if let Err(error) = swoole_rs_zend::contain_with(|| declared = handle.exec(&query)) {
            error.defer();
            return false;
        }
    }
    if crate::driver::statement_result(dbh, stmt, result, declared).is_none() {
        return false;
    }
    if let Some(previous) = result.take() {
        crate::PQclear(previous);
    }
    prepared.set(true);
    let mut fetch = swoole_rs_zend::message!("FETCH FORWARD 0 FROM ");
    fetch.push(cursor.get().to_bytes());
    result.set(handle.exec(fetch.as_cstr()));
    true
}
}

/// Release PHP 8.5's active result. A displaced single-row query must be drained
/// before another send, including after the destructor requests cancellation.
#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_finish_result(
    active: Option<std::ptr::NonNull<pq_sys::PGconn>>,
    result: &Cell<Option<crate::PgResult>>,
    abort_query: bool,
) {
    use std::ptr::NonNull;
    unsafe extern "C" {
        safe fn PQgetCancel(conn: crate::Conn) -> Option<NonNull<pq_sys::PGcancel>>;
        safe fn PQcancel(
            cancel: NonNull<pq_sys::PGcancel>,
            error: &mut [std::ffi::c_char; 256],
            size: c_int,
        ) -> c_int;
        safe fn PQfreeCancel(cancel: NonNull<pq_sys::PGcancel>);
    }
    let previous = result.take();
    if previous.is_some()
        && abort_query
        && let Some(cancel) = active.and_then(|conn| PQgetCancel(crate::Conn(conn.as_ptr())))
    {
        let mut error = [0; 256];
        PQcancel(cancel, &mut error, 256);
        PQfreeCancel(cancel);
    }
    if let Some(previous) = previous {
        crate::PQclear(previous);
    }
    if let Some(conn) = active {
        while let Some(next) = crate::PQgetResult(crate::Conn(conn.as_ptr())) {
            crate::PQclear(next);
        }
    }
}

/// Release a generated statement name. PHP 8.4+ uses the close-prepared
/// protocol when the linked libpq implements it.
#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_close_statement(handle: &Handle, name: Text<'_>) {
    let result = if PHP_VERSION_ID >= 80400 && crate::lib_version() >= 170_000 {
        std::ptr::NonNull::new(crate::swoole_pgsql_close_prepared(
            crate::driver::pdo_pgsql_server(handle),
            name,
        ))
        .map(crate::PgResult)
    } else {
        let mut query = swoole_rs_zend::message!("DEALLOCATE ");
        query.push(name.get().to_bytes());
        handle.exec(query.as_cstr())
    };
    if let Some(result) = result {
        crate::PQclear(result);
    }
}

/// Prepare once parameter types are known. A pooled connection may still have
/// this generated name from its previous client; remove it and retry once.
#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_prepare_statement(
    dbh: &Dbh,
    stmt: &Stmt,
    slot: &Cell<Option<crate::PgResult>>,
    name: Text<'_>,
    query: Text<'_>,
    count: c_int,
    types: crate::Params<Oid>,
) -> bool {
    let Some(handle) = crate::driver::handle(dbh) else {
        return false;
    };
    let prepare = || {
        std::ptr::NonNull::new(crate::swoole_pgsql_prepare(
            crate::driver::pdo_pgsql_server(handle),
            Some(name),
            Some(query),
            count,
            types,
        ))
        .map(crate::PgResult)
    };
    let mut result = prepare();
    if crate::driver::sqlstate(result.as_ref()) == Some(c"42P05") {
        if let Some(old) = result.take() {
            crate::PQclear(old);
        }
        pdo_pgsql_close_statement(handle, name);
        result = prepare();
    }
    if crate::driver::statement_result(dbh, stmt, slot, result).is_none() {
        return false;
    }
    if let Some(old) = slot.take() {
        crate::PQclear(old);
    }
    true
}

/// Resolve a PDO parameter to the index expected by the bundled statement.
/// Direct dollar names keep their parsed index; map entries are one-based.
#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_normalize_parameter(
    dbh: &Dbh,
    stmt: &Stmt,
    name: Option<&swoole_rs_zend::ZStr>,
    index: &Cell<i64>,
) -> bool {
    unsafe extern "C" {
        // PDO's rewrite map owns the name until the statement is changed.
        safe fn pdo_pgsql_rewritten_name<'a>(
            stmt: &'a Stmt,
            name: &swoole_rs_zend::ZStr,
        ) -> Option<&'a swoole_rs_zend::ZStr>;
    }
    let Some(name) = name else {
        return true;
    };
    if let Some(number) = name.bytes().strip_prefix(b"$") {
        index.set(swoole_rs_corebase::strtol(number).0);
        return true;
    }
    if let Some(rewritten) = pdo_pgsql_rewritten_name(stmt, name) {
        let number = rewritten.bytes().get(1..).unwrap_or_default();
        index.set(swoole_rs_corebase::strtol(number).0 - 1);
        return true;
    }
    crate::driver::pdo_pgsql_raise(
        dbh,
        Some(stmt),
        0,
        Some(c"HY093".into()),
        Some(name.c_str().into()),
    );
    false
}

#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_check_parameter(
    dbh: &Dbh,
    stmt: &Stmt,
    map: Option<&swoole_rs_zend::Array>,
    index: i64,
) -> bool {
    if map.is_none_or(|map| map.index(index.cast_unsigned()).is_some()) {
        return true;
    }
    crate::driver::pdo_pgsql_raise(
        dbh,
        Some(stmt),
        0,
        Some(c"HY093".into()),
        Some(c"parameter was not defined".into()),
    );
    false
}

swoole_rs_zend::exports! {
/// With emulated placeholders, bind input booleans as PostgreSQL truth literals.
#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_emulated_parameter(kind: &Cell<pdo_param_type>, slot: &Slot) -> bool {
    use swoole_rs_zend::sys::pdo_param_type_PDO_PARAM_INPUT_OUTPUT as INPUT_OUTPUT;
    // PDO_PARAM_TYPE strips the upper 16 flag bits.
    if kind.get() & 0xffff != BOOL || kind.get() & INPUT_OUTPUT != 0 {
        return true;
    }
    let mut truth = false;
    if let Err(error) = swoole_rs_zend::contain_with(|| truth = slot.value().truthy()) {
        error.defer();
        return false;
    }
    kind.set(STR);
    slot.set(swoole_rs_zend::Owned::string(ZString::new(if truth { b"t" } else { b"f" })));
    true
}
}

/// Parallel libpq parameter arrays. Each slot also has stable storage for a
/// large object's network-order OID until libpq copies the bound parameters.
struct ParameterStorage {
    values: Vec<Cell<*const std::ffi::c_char>>,
    lengths: Vec<Cell<c_int>>,
    formats: Vec<Cell<c_int>>,
    types: Vec<Cell<Oid>>,
    owned: Vec<BoundValue>,
}

#[derive(Default)]
struct BoundValue {
    string: Cell<Option<ZString>>,
    lob_oid: Cell<Oid>,
}

#[expect(
    clippy::unnecessary_box_returns,
    reason = "the C statement owns this opaque pointer until its destructor"
)]
#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_allocate_parameters(
    count: usize,
    values: &mut *const *const std::ffi::c_char,
    lengths: &mut *const c_int,
    formats: &mut *const c_int,
    types: &mut *const Oid,
) -> Box<ParameterStorage> {
    let owner = Box::new(ParameterStorage {
        values: vec![Cell::new(std::ptr::null()); count],
        lengths: vec![Cell::new(0); count],
        formats: vec![Cell::new(0); count],
        types: vec![Cell::new(0); count],
        owned: (0..count).map(|_| BoundValue::default()).collect(),
    });
    *values = owner.values.as_ptr().cast();
    *lengths = owner.lengths.as_ptr().cast();
    *formats = owner.formats.as_ptr().cast();
    *types = owner.types.as_ptr().cast();
    owner
}

impl ParameterStorage {
    fn bind(
        &self,
        index: usize,
        bytes: *const std::ffi::c_char,
        length: c_int,
        binary: bool,
        oid: Oid,
    ) -> bool {
        let (Some(value), Some(len), Some(format), Some(ty)) = (
            self.values.get(index),
            self.lengths.get(index),
            self.formats.get(index),
            self.types.get(index),
        ) else {
            return false;
        };
        value.set(bytes);
        len.set(length);
        format.set(i32::from(binary));
        ty.set(oid);
        true
    }
}

#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_bind_oid(params: &ParameterStorage, index: usize, oid: Oid) -> bool {
    let Some(owned) = params.owned.get(index) else {
        return false;
    };
    owned.string.set(None);
    owned.lob_oid.set(oid.to_be());
    params.bind(index, owned.lob_oid.as_ptr().cast(), 4, true, 26)
}

swoole_rs_zend::exports! {
/// Keep the encoded bytes alive through libpq's send, including when conversion
/// of a later parameter invokes PHP and changes an earlier bound reference.
#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_bind_value(params: &ParameterStorage, index: usize, kind: pdo_param_type, slot: &Slot) -> bool {
    use swoole_rs_zend::sys::{IS_TRUE, IS_FALSE, pdo_param_type_PDO_PARAM_NULL as NULL};
    let Some(owned) = params.owned.get(index) else { return false; };
    let value = slot.value();
    let (bytes, len, string) = if kind == NULL || value.is_null() {
        (std::ptr::null(), 0, None)
    } else if matches!(value.kind(), IS_TRUE | IS_FALSE) {
        let literal = if value.kind() == IS_TRUE { c"t" } else { c"f" };
        (literal.as_ptr(), 1, None)
    } else {
        if value.str().is_none() {
            let Some(text) = value.applied_string() else { return false; };
            slot.set(swoole_rs_zend::Owned::string(text));
        }
        let Some(text) = slot.value().str() else { return false; };
        let bytes = text.bytes();
        let Ok(len) = c_int::try_from(bytes.len()) else { return false; };
        (bytes.as_ptr().cast(), len, Some(text.to_owned()))
    };
    owned.string.set(string);
    params.bind(index, bytes, len, kind == LOB, 0)
}
}

#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_free_parameters(_owner: Option<Box<ParameterStorage>>) {}
