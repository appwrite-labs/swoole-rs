//! PostgreSQL statement fetching and column metadata.
use crate::driver::{Handle, Res};
use pq_sys::Oid;
use std::ffi::{CStr, c_int};
use swoole_rs_corebase::{CCast, sys::Text};
use swoole_rs_zend::sys::{
    PHP_VERSION_ID, pdo_param_type, pdo_param_type_PDO_PARAM_BOOL as BOOL,
    pdo_param_type_PDO_PARAM_INT as INT, pdo_param_type_PDO_PARAM_LOB as LOB,
    pdo_param_type_PDO_PARAM_STR as STR,
};
use swoole_rs_zend::{OwnedArray, bailout::Bailout, pdo::Slot};
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
    dbh: &swoole_rs_zend::pdo::Dbh,
    stmt: &swoole_rs_zend::pdo::Stmt,
    result: &std::cell::Cell<Option<crate::PgResult>>,
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
    crate::driver::cursor_result(dbh, stmt, result, handle.exec(query.as_cstr()))
}
