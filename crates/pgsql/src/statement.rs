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

/// Fill PDO's column description and retain its PostgreSQL type for `getColumnMeta`.
#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_describe_column(
    result: Res<'_>,
    column: c_int,
    desc: &mut pdo_column_data,
    oid: &mut Oid,
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
    *oid = PQftype(result, column);
    true
}

swoole_rs_zend::exports! {
/// The metadata for a live statement column, returned to PDO.
#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_column_meta(handle: &Handle, oid: Oid, table: Oid, output: &Slot) -> std::ffi::c_int {
    match column_meta(handle, oid, table) {
        Ok(meta) => { output.set(meta.into_owned()); 1 }
        Err(error) => { error.defer(); -1 }
    }
}
}

/// Issue FETCH for a scrollable cursor. The driver names every cursor `pdo_crsr_%08x`.
#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_fetch_cursor(
    dbh: &Dbh,
    stmt: &Stmt,
    result: &Cell<Option<crate::PgResult>>,
    cursor: Text<'_>,
    orientation: swoole_rs_zend::sys::pdo_fetch_orientation,
    offset: swoole_rs_zend::sys::zend_long,
) -> bool {
    use swoole_rs_zend::sys::{
        pdo_fetch_orientation_PDO_FETCH_ORI_ABS as ABS,
        pdo_fetch_orientation_PDO_FETCH_ORI_FIRST as FIRST,
        pdo_fetch_orientation_PDO_FETCH_ORI_LAST as LAST,
        pdo_fetch_orientation_PDO_FETCH_ORI_NEXT as NEXT,
        pdo_fetch_orientation_PDO_FETCH_ORI_PRIOR as PRIOR,
        pdo_fetch_orientation_PDO_FETCH_ORI_REL as REL,
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
    crate::driver::statement_result(dbh, stmt, result, handle.exec(query.as_cstr()))
        .is_some_and(|rows| rows > 0)
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
    values: Vec<*const std::ffi::c_char>,
    lengths: Vec<c_int>,
    formats: Vec<c_int>,
    types: Vec<Oid>,
    lob_oids: Vec<Oid>,
}

#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_allocate_parameters(
    count: usize,
    values: &mut *mut *const std::ffi::c_char,
    lengths: &mut *mut c_int,
    formats: &mut *mut c_int,
    types: &mut *mut Oid,
    lob_oids: &mut *mut Oid,
) -> Box<ParameterStorage> {
    let mut owner = Box::new(ParameterStorage {
        values: vec![std::ptr::null(); count],
        lengths: vec![0; count],
        formats: vec![0; count],
        types: vec![0; count],
        lob_oids: vec![0; count],
    });
    *values = owner.values.as_mut_ptr();
    *lengths = owner.lengths.as_mut_ptr();
    *formats = owner.formats.as_mut_ptr();
    *types = owner.types.as_mut_ptr();
    *lob_oids = owner.lob_oids.as_mut_ptr();
    owner
}

#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_free_parameters(_owner: Option<Box<ParameterStorage>>) {}
