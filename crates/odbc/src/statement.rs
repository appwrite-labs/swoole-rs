//! PDO's ODBC cursor and result-set callbacks.
use crate::{
    Statement,
    diagnostics::{pdo_odbc_error, swr_odbc_statement},
};
use swoole_rs_corebase::sys::Text;
use swoole_rs_odbc_sys::{SQLLEN, SQLRETURN};
use swoole_rs_zend::{
    pdo::{Dbh, Stmt},
    sys::{
        pdo_fetch_orientation, pdo_fetch_orientation_PDO_FETCH_ORI_ABS as ABS,
        pdo_fetch_orientation_PDO_FETCH_ORI_FIRST as FIRST,
        pdo_fetch_orientation_PDO_FETCH_ORI_LAST as LAST,
        pdo_fetch_orientation_PDO_FETCH_ORI_NEXT as NEXT,
        pdo_fetch_orientation_PDO_FETCH_ORI_PRIOR as PRIOR,
        pdo_fetch_orientation_PDO_FETCH_ORI_REL as REL,
    },
};

unsafe extern "C" {
    #[link_name = "SQLFetchScroll"]
    safe fn fetch_scroll(statement: Statement, orientation: i16, offset: SQLLEN) -> SQLRETURN;
    #[link_name = "SQLMoreResults"]
    safe fn more_results(statement: Statement) -> SQLRETURN;
    #[link_name = "SQLNumResultCols"]
    safe fn result_cols(statement: Statement, count: &mut i16) -> SQLRETURN;
    #[link_name = "SQLCloseCursor"]
    safe fn close_cursor(statement: Statement) -> SQLRETURN;
    safe fn swr_odbc_replace_columns(stmt: &Stmt, count: i16);
}

#[unsafe(no_mangle)]
extern "C" fn swr_odbc_fetch(
    dbh: &Dbh,
    stmt: &Stmt,
    orientation: pdo_fetch_orientation,
    offset: SQLLEN,
    file: Text<'static>,
    info_line: i32,
    error_line: i32,
) -> bool {
    let direction = match orientation {
        NEXT => 1,
        FIRST => 2,
        LAST => 3,
        PRIOR => 4,
        ABS => 5,
        REL => 6,
        _ => {
            stmt.set_error_code(c"HY106");
            return false;
        }
    };
    let statement = swr_odbc_statement(stmt);
    match fetch_scroll(statement, direction, offset) {
        0 => true,
        100 => false, // SQL_NO_DATA
        status => {
            let has_row = status == 1; // SQL_SUCCESS_WITH_INFO
            pdo_odbc_error(
                dbh,
                Some(stmt),
                statement,
                c"SQLFetchScroll".into(),
                file,
                if has_row { info_line } else { error_line },
            );
            has_row
        }
    }
}

#[unsafe(no_mangle)]
extern "C" fn swr_odbc_next_rowset(stmt: &Stmt) -> bool {
    let statement = swr_odbc_statement(stmt);
    if !matches!(more_results(statement), 0 | 1) {
        return false;
    }
    let mut count = 0;
    result_cols(statement, &mut count);
    swr_odbc_replace_columns(stmt, count);
    true
}

#[unsafe(no_mangle)]
extern "C" fn swr_odbc_close_cursor(stmt: &Stmt) -> bool {
    matches!(close_cursor(swr_odbc_statement(stmt)), 0 | 1)
}

unsafe extern "C" {
    #[link_name = "SQLGetCursorName"]
    safe fn get_cursor_name(
        statement: Statement,
        name: &mut [u8; 256],
        capacity: i16,
        length: &mut i16,
    ) -> SQLRETURN;
    #[link_name = "SQLSetCursorName"]
    safe fn set_cursor_name(statement: Statement, name: crate::Text, length: i16) -> SQLRETURN;
    safe fn swr_odbc_stmt_set_utf8(stmt: &Stmt, enabled: bool);
}

use crate::driver::Attribute;
use swoole_rs_corebase::CCast;
use swoole_rs_zend::{
    Owned, STREAM_PAGE, Value, ZString,
    pdo::Slot,
    sys::{
        IS_RESOURCE, pdo_attribute_type_PDO_ATTR_CURSOR_NAME as CURSOR_NAME,
        pdo_attribute_type_PDO_ATTR_DRIVER_SPECIFIC as DRIVER_SPECIFIC, zend_long,
    },
};

#[unsafe(no_mangle)]
extern "C" fn swr_odbc_stmt_set_attr(
    dbh: &Dbh,
    stmt: &Stmt,
    attr: zend_long,
    value: &Value,
    file: Text<'static>,
    line: i32,
) -> Attribute {
    match u32::try_from(attr) {
        Ok(CURSOR_NAME) => {
            // The PDO adapter converts the zval in place before lending its string.
            let Some(name) = value.str() else {
                return Attribute::Unsupported;
            };
            let Ok(length) = i16::try_from(name.bytes().len()) else {
                stmt.set_error_code(c"HY090");
                return Attribute::Unsupported;
            };
            let bytes = name.bytes_with_nul();
            let statement = swr_odbc_statement(stmt);
            let rc = set_cursor_name(statement, crate::Text(bytes.as_ptr().cast_mut()), length);
            if matches!(rc, 0 | 1) {
                return Attribute::Set;
            }
            pdo_odbc_error(
                dbh,
                Some(stmt),
                statement,
                c"SQLSetCursorName".into(),
                file,
                line,
            );
            Attribute::Unsupported
        }
        Ok(n) if n == DRIVER_SPECIFIC + 1 => {
            swr_odbc_stmt_set_utf8(stmt, value.truthy());
            Attribute::Unsupported
        }
        _ => {
            crate::diagnostics::unknown_attribute(dbh, Some(stmt), c"setAttribute".into());
            Attribute::Error
        }
    }
}

#[unsafe(no_mangle)]
extern "C" fn swr_odbc_stmt_get_attr(
    dbh: &Dbh,
    stmt: &Stmt,
    attr: zend_long,
    value: &Slot,
    utf8: bool,
    file: Text<'static>,
    line: i32,
) -> Attribute {
    match u32::try_from(attr) {
        Ok(CURSOR_NAME) => {
            let mut buffer = [0; 256];
            let mut length = 0;
            let statement = swr_odbc_statement(stmt);
            if matches!(
                get_cursor_name(statement, &mut buffer, 256, &mut length),
                0 | 1
            ) {
                let count = usize::try_from(length).unwrap_or(0).min(255);
                value.set(Owned::string(ZString::new(buffer.split_at(count).0)));
                return Attribute::Set;
            }
            pdo_odbc_error(
                dbh,
                Some(stmt),
                statement,
                c"SQLGetCursorName".into(),
                file,
                line,
            );
            Attribute::Unsupported
        }
        Ok(n) if n == DRIVER_SPECIFIC + 1 => {
            value.set(Owned::bool(utf8));
            Attribute::Unsupported
        }
        _ => {
            crate::diagnostics::unknown_attribute(dbh, Some(stmt), c"getAttribute".into());
            Attribute::Error
        }
    }
}

/// A parameter PDO bound: the token `swr_odbc_bind_parameter` gives `SQLBindParameter`
/// for an input parameter, which `SQLParamData` hands back when the parameter wants its
/// data. PDO keeps the record while the statement lives. Rust never makes one.
#[repr(transparent)]
struct Param(std::cell::Cell<swoole_rs_zend::sys::pdo_bound_param_data>);

unsafe extern "C" {
    /// `SQLParamData`: with `SQL_NEED_DATA`, `token` is the pointer the parameter that
    /// wants data was bound with. Only input parameters are bound for data at execution
    /// (`odbc_stmt_param_hook`), each with its PDO record.
    #[link_name = "SQLParamData"]
    safe fn param_data(statement: Statement, token: &mut Option<&Param>) -> SQLRETURN;
    /// What the parameter holds now, through its reference if it is one.
    safe fn swr_odbc_parameter_value(param: &Param) -> &Value;
    safe fn swr_odbc_has_columns(stmt: &Stmt) -> bool;
}

/// `SQL_NEED_DATA`.
const NEED_DATA: SQLRETURN = 99;

/// The parameter `statement` wants data for next, borrowed for as long as its
/// statement is.
fn wanted(_stmt: &Stmt, statement: Statement) -> (SQLRETURN, Option<&Param>) {
    let mut token = None;
    (param_data(statement, &mut token), token)
}

/// `pdo_odbc`'s data-at-execution loop: each parameter `SQLParamData` asks for is sent
/// with `SQLPutData`, a string whole and a stream a page at a time. Every `SQLPutData`
/// is offloaded on its own; the stream is read here, on the request's thread. The
/// execution's final status, or `None` once the cursor is closed after an input that
/// is no stream (reported at `line`) or a stream wrapper's bailout (deferred).
fn supply_data(
    dbh: &Dbh,
    stmt: &Stmt,
    statement: Statement,
    file: Text<'static>,
    line: i32,
) -> Option<SQLRETURN> {
    let put = |bytes: &[u8]| {
        crate::swoole_odbc_SQLPutData(
            statement,
            crate::Data(bytes.as_ptr().cast_mut().cast()),
            bytes.len().c_cast(),
        )
    };
    let mut page = Box::new([0; STREAM_PAGE]);
    let mut status = NEED_DATA;
    while status == NEED_DATA {
        let (next, param) = wanted(stmt, statement);
        status = next;
        let Some(param) = param.filter(|_| next == NEED_DATA) else {
            break;
        };
        let value = swr_odbc_parameter_value(param);
        if value.kind() != IS_RESOURCE {
            let text = value.string();
            if swoole_rs_zend::bailout::pending() {
                close_cursor(statement);
                return None;
            }
            let rc = put(text.bytes());
            if !matches!(rc, 0 | 1) {
                status = rc;
            }
            continue;
        }
        // A failed chunk decides the status, and the rest of the stream is still read.
        loop {
            let rc = match value.read_stream(&mut page) {
                Ok(Some([])) => break,
                Ok(Some(chunk)) => put(chunk),
                Ok(None) => {
                    let what = c"input LOB is no longer a stream".into();
                    pdo_odbc_error(dbh, Some(stmt), statement, what, file, line);
                    close_cursor(statement);
                    return None;
                }
                Err(bailout) => {
                    bailout.defer();
                    close_cursor(statement);
                    return None;
                }
            };
            if !matches!(rc, 0 | 1) {
                status = rc;
            }
        }
    }
    Some(status)
}

swoole_rs_zend::exports! {
#[unsafe(no_mangle)]
extern "C" fn swr_odbc_execute(dbh: &Dbh, stmt: &Stmt, executed: bool, file: Text<'static>, lines: &[i32; 3]) -> bool {
    let statement = swr_odbc_statement(stmt);
    if executed {
        close_cursor(statement);
    }
    let mut status = crate::swoole_odbc_SQLExecute(statement);
    if status == NEED_DATA {
        let Some(supplied) = supply_data(dbh, stmt, statement, file, lines[2]) else {
            return false;
        };
        status = supplied;
    }
    if status != 0 {
        let completed = matches!(status, 1 | 100);
        pdo_odbc_error(
            dbh,
            Some(stmt),
            statement,
            c"SQLExecute".into(),
            file,
            if completed { lines[0] } else { lines[1] },
        );
        if !completed {
            return false;
        }
    }
    let mut rows = -1;
    crate::driver::row_count(statement, &mut rows);
    stmt.set_row_count(rows);
    if !swr_odbc_has_columns(stmt) {
        let mut count = 0;
        result_cols(statement, &mut count);
        swr_odbc_replace_columns(stmt, count);
    }
    true
}
}

fn read_chunk(
    statement: Statement,
    column: u16,
    kind: i16,
    buffer: &mut [u8],
    length: &mut SQLLEN,
) -> SQLRETURN {
    crate::swoole_odbc_SQLGetData(
        statement,
        column,
        kind,
        crate::Out(buffer.as_mut_ptr().cast()),
        buffer.len().c_cast(),
        crate::Out(length),
    )
}

/// Read an unbound column using one coroutine-aware ODBC call per chunk.
/// The return value tells the statement adapter whether it has a string to convert.
#[unsafe(no_mangle)]
extern "C" fn swr_odbc_read_long(
    stmt: &Stmt,
    column: u16,
    unicode: bool,
    page: usize,
    declared_size: usize,
    output: &Slot,
) -> bool {
    let statement = swr_odbc_statement(stmt);
    let kind = if unicode { -2 } else { 1 }; // SQL_C_BINARY or SQL_C_CHAR
    let mut first = vec![0; page];
    let mut total = -1;
    let status = read_chunk(statement, column, kind, &mut first, &mut total);
    if !matches!(status, 0 | 1) {
        return false;
    }
    if status == 0 && total < page.c_cast::<SQLLEN>() {
        let Ok(length) = usize::try_from(total) else {
            output.set(Owned::null());
            return false;
        };
        output.set(Owned::string(ZString::new(first.split_at(length).0)));
        return true;
    }
    // Character buffers reserve a terminator. Binary buffers use every byte.
    let terminator = usize::from(!unicode);
    let first_payload = page - terminator;
    let mut bytes = first;
    bytes.truncate(first_payload);
    let chunk_size = if swoole_rs_zend::sys::PHP_VERSION_ID < 80500 {
        first_payload
    } else {
        usize::try_from(total).unwrap_or(declared_size.min(first_payload))
    }
    .clamp(1, 64 * 1024);
    let mut buffer = vec![0; chunk_size + terminator];
    loop {
        let mut remaining = 0;
        buffer.fill(0);
        let rc = read_chunk(statement, column, kind, &mut buffer, &mut remaining);
        if !matches!(rc, 0 | 1) {
            break;
        }
        // The indicator excludes the terminator and counts from this call.
        // Some drivers consume fewer bytes than the previous buffer allowed.
        if total >= 0
            && remaining >= 0
            && let Ok(offset) = usize::try_from(total - remaining)
        {
            bytes.truncate(offset);
        }
        let count = usize::try_from(remaining)
            .unwrap_or(chunk_size)
            .min(chunk_size);
        bytes.extend_from_slice(buffer.split_at(count).0);
        if rc == 0 || count == 0 {
            break;
        }
    }
    output.set(Owned::string(ZString::new(&bytes)));
    true
}

unsafe extern "C" {
    #[link_name = "SQLSetStmtAttr"]
    safe fn set_stmt_attr(stmt: Statement, attr: i32, value: usize, length: i32) -> SQLRETURN;
    safe fn swr_odbc_publish_statement(stmt: &Stmt, statement: Statement);
    safe fn swr_odbc_prepare_error(dbh: &Dbh, stmt: &Stmt);
}

swoole_rs_zend::exports! {
#[unsafe(no_mangle)]
extern "C" fn swr_odbc_prepare(dbh: &Dbh, stmt: &Stmt, sql: &swoole_rs_zend::ZStr, options: Option<&Value>, file: Text<'static>, lines: &[i32; 3]) -> bool {
    use crate::{Handle, driver::{alloc_handle, free_handle, swr_odbc_connection}};
    let mut handle = Handle(std::ptr::null_mut());
    let rc = alloc_handle(3, Handle(swr_odbc_connection(dbh).0), &mut handle);
    if matches!(rc, -1 | -2) {
        pdo_odbc_error(dbh, None, Statement::NULL, c"SQLAllocStmt".into(), file, lines[0]);
        return false;
    }
    let statement = Statement(handle.0);
    swr_odbc_publish_statement(stmt, statement);
    let cursor = swoole_rs_zend::pdo::attr_long(options, swoole_rs_zend::sys::pdo_attribute_type_PDO_ATTR_CURSOR, 0);
    if cursor != 0 && !matches!(set_stmt_attr(statement, -1, 1, 0), 0 | 1) {
        pdo_odbc_error(dbh, Some(stmt), statement, c"SQLSetStmtAttr: SQL_ATTR_CURSOR_SCROLLABLE".into(), file, lines[1]);
        free_handle(3, handle);
        swr_odbc_publish_statement(stmt, Statement::NULL);
        return false;
    }
    let rc = crate::swoole_odbc_SQLPrepare(statement, crate::Text(sql.bytes_with_nul().as_ptr().cast_mut()), -3);
    if rc != 0 {
        pdo_odbc_error(dbh, Some(stmt), statement, c"SQLPrepare".into(), file, lines[2]);
        if rc != 1 {
            swr_odbc_prepare_error(dbh, stmt);
            return false;
        }
    }
    true
}
}

unsafe extern "C" {
    #[link_name = "SQLColAttribute"]
    safe fn column_attribute(
        stmt: Statement,
        column: u16,
        field: u16,
        text: Option<&mut !>,
        capacity: i16,
        length: Option<&mut i16>,
        number: &mut SQLLEN,
    ) -> SQLRETURN;
    safe fn swr_odbc_column_info(
        stmt: &Stmt,
        index: i32,
        name: &[u8; 128],
        length: usize,
        kind: i16,
        display_size: usize,
        unbound: bool,
    );
    safe fn swr_odbc_bind_column(stmt: &Stmt, index: i32) -> SQLRETURN;
}

/// ODBC drivers can report an unlimited character or binary type with size zero.
/// After the first unbound column, subsequent columns must be read in order.
#[unsafe(no_mangle)]
extern "C" fn swr_odbc_describe(
    dbh: &Dbh,
    stmt: &Stmt,
    index: i32,
    going_long: bool,
    page: usize,
    file: Text<'static>,
    lines: &[i32; 3],
) -> bool {
    let statement = swr_odbc_statement(stmt);
    let column = index.wrapping_add(1).c_cast();
    let mut name = [0; 128];
    let mut name_len = 0;
    let mut sql_type = 0;
    let mut size = 0;
    let rc = crate::swoole_odbc_SQLDescribeCol(
        statement,
        column,
        crate::Out(name.as_mut_ptr()),
        127,
        crate::Out(&raw mut name_len),
        crate::Out(&raw mut sql_type),
        crate::Out(&raw mut size),
        crate::Out(std::ptr::null_mut()),
        crate::Out(std::ptr::null_mut()),
    );
    let report = |what: &'static std::ffi::CStr, line| {
        pdo_odbc_error(dbh, Some(stmt), statement, what.into(), file, line);
    };
    if rc != 0 {
        report(c"SQLDescribeCol", lines[0]);
        if rc != 1 {
            return false;
        }
    }
    let mut display = 0;
    let rc = column_attribute(statement, column, 6, None, 0, None, &mut display); // SQL_DESC_DISPLAY_SIZE
    if rc != 0 {
        report(c"SQLColAttribute", lines[1]);
        if rc != 1 {
            return false;
        }
    }
    let display_size = display.c_cast::<usize>();
    let unlimited = size == 0 && matches!(sql_type, 12 | -1 | -9 | -10 | -3 | -4);
    let unbound = going_long || unlimited || display_size >= page;
    let length = usize::try_from(name_len).unwrap_or(0).min(126);
    swr_odbc_column_info(stmt, index, &name, length, sql_type, display_size, unbound);
    if !unbound && swr_odbc_bind_column(stmt, index) != 0 {
        report(c"SQLBindCol", lines[2]);
        return false;
    }
    true
}

unsafe extern "C" {
    #[link_name = "SQLDescribeParam"]
    safe fn describe_param(
        statement: Statement,
        index: u16,
        kind: &mut i16,
        precision: &mut usize,
        scale: &mut i16,
        nullable: &mut i16,
    ) -> SQLRETURN;
    safe fn swr_odbc_bind_parameter(
        stmt: &Stmt,
        parameter: std::ptr::NonNull<swoole_rs_zend::sys::pdo_bound_param_data>,
        index: u16,
        direction: i16,
        c_type: i16,
        sql_type: i16,
        precision: usize,
        scale: i16,
        unicode: bool,
        output: bool,
        units: usize,
    ) -> SQLRETURN;
}

#[unsafe(no_mangle)]
extern "C" fn swr_odbc_allocate_parameter(
    dbh: &Dbh,
    stmt: &Stmt,
    parameter: std::ptr::NonNull<swoole_rs_zend::sys::pdo_bound_param_data>,
    index: u16,
    param_type: swoole_rs_zend::sys::pdo_param_type,
    max_length: SQLLEN,
    inout: bool,
    utf8: bool,
    file: Text<'static>,
    lines: &[i32; 2],
) -> bool {
    use swoole_rs_zend::sys::{
        pdo_param_type_PDO_PARAM_INT as INT, pdo_param_type_PDO_PARAM_LOB as LOB,
        pdo_param_type_PDO_PARAM_NULL as NULL, pdo_param_type_PDO_PARAM_STMT as STMT,
    };
    if param_type == STMT {
        return false;
    }
    let statement = swr_odbc_statement(stmt);
    let mut sql_type = 0;
    let mut precision = 0;
    let mut scale = 0;
    let length = usize::try_from(max_length).ok().filter(|&n| n > 0);
    if !matches!(
        describe_param(
            statement,
            index,
            &mut sql_type,
            &mut precision,
            &mut scale,
            &mut 0
        ),
        0 | 1
    ) {
        sql_type = match param_type {
            INT => 4,
            LOB => -4,
            _ => -1,
        };
        precision = length.unwrap_or(4000);
        scale = 5;
    }
    let direction = if inout {
        2
    } else if length.is_some() {
        4
    } else {
        1
    };
    if param_type == LOB && direction != 1 {
        pdo_odbc_error(
            dbh,
            Some(stmt),
            statement,
            c"Can't bind a lob for output".into(),
            file,
            lines[0],
        );
        return false;
    }
    let unicode = utf8 && matches!(sql_type, -10..=-8);
    let c_type = if unicode || matches!(sql_type, -4..=-2) {
        -2
    } else {
        1
    };
    let output = direction != 1 && param_type != NULL;
    let units = if output {
        length.unwrap_or(precision)
    } else {
        0
    };
    if matches!(
        swr_odbc_bind_parameter(
            stmt, parameter, index, direction, c_type, sql_type, precision, scale, unicode, output,
            units
        ),
        0 | 1
    ) {
        return true;
    }
    pdo_odbc_error(
        dbh,
        Some(stmt),
        statement,
        c"SQLBindParameter".into(),
        file,
        lines[1],
    );
    false
}
