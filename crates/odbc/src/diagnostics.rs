//! Error records shared with the bundled statement driver.
use std::{cell::Cell, ffi::CStr};

use swoole_rs_corebase::sys::Text;
use swoole_rs_odbc_sys::SQLRETURN;
use swoole_rs_zend::pdo::{Dbh, Stmt};

use crate::{Handle, Statement, driver::swr_odbc_connection};

/// The `pdo_odbc_errinfo` record from `php_pdo_odbc_int.h`. File and operation names are static
/// C literals, including for errors originating in the statement callbacks.
#[repr(C)]
#[derive(Clone, Copy)]
struct ErrorInfo {
    state: [u8; 6],
    message: [u8; 512],
    code: i32,
    file: Option<Text<'static>>,
    what: Option<Text<'static>>,
    line: i32,
}

unsafe extern "C" {
    safe fn swr_odbc_error_record<'a>(dbh: &'a Dbh, stmt: Option<&'a Stmt>) -> &'a Cell<ErrorInfo>;
    safe fn swr_odbc_environment(dbh: &Dbh) -> Handle;
    pub(crate) safe fn swr_odbc_statement(stmt: &Stmt) -> Statement;
    safe fn swr_odbc_connect_exception(dbh: &Dbh);

    #[link_name = "SQLGetDiagRec"]
    safe fn diagnostic(
        kind: i16,
        handle: Handle,
        record: i16,
        state: &mut [u8; 6],
        code: &mut i32,
        message: &mut [u8; 1024],
        capacity: i16,
        used: &mut i16,
    ) -> SQLRETURN;
}

/// The statement C calls this same entry point as the Rust driver.
#[unsafe(no_mangle)]
pub(crate) extern "C" fn pdo_odbc_error(
    dbh: &Dbh,
    stmt: Option<&Stmt>,
    statement: Statement,
    what: Text<'static>,
    file: Text<'static>,
    line: i32,
) {
    let statement = if statement.0.is_null() {
        stmt.map_or(statement, |s| swr_odbc_statement(s))
    } else {
        statement
    };
    let connection = swr_odbc_connection(dbh);
    let (kind, handle) = if !statement.0.is_null() {
        (3, Handle(statement.0))
    } else if !connection.0.is_null() {
        (2, Handle(connection.0))
    } else {
        (1, swr_odbc_environment(dbh))
    };

    let slot = swr_odbc_error_record(dbh, stmt);
    let mut info = slot.get();
    let mut message = [0; 1024];
    let mut used = 0;
    let mut rc = diagnostic(
        kind,
        handle,
        1,
        &mut info.state,
        &mut info.code,
        &mut message,
        511,
        &mut used,
    );
    let copied = if matches!(rc, 0 | 1) {
        usize::try_from(used).unwrap_or(0).min(510)
    } else {
        0
    };
    let (prefix, rest) = info.message.split_at_mut(copied);
    prefix.copy_from_slice(message.split_at(copied).0);
    if let Some(terminator) = rest.first_mut() {
        *terminator = 0;
    }
    info.what = Some(what);
    info.file = Some(file);
    info.line = line;
    slot.set(info);
    let state = CStr::from_bytes_until_nul(&info.state).unwrap_or(c"");
    if let Some(stmt) = stmt {
        stmt.set_error_code(state);
    } else {
        dbh.set_error_code(state);
    }
    if !dbh.has_methods() {
        swr_odbc_connect_exception(dbh);
    }

    // Drain every record. SQL Server can disturb an active cursor if diagnostic
    // records from PRINT and other statements remain unread.
    let mut record = 1_i16;
    let mut discarded = [0; 6];
    let mut code = 0;
    while matches!(rc, 0 | 1) {
        record = record.wrapping_add(1);
        rc = diagnostic(
            kind,
            handle,
            record,
            &mut discarded,
            &mut code,
            &mut message,
            1023,
            &mut used,
        );
    }
}

pub(crate) fn unknown_attribute(dbh: &Dbh, stmt: Option<&Stmt>, operation: Text<'static>) {
    let slot = swr_odbc_error_record(dbh, stmt);
    let mut info = slot.get();
    let message = c"Unknown Attribute".to_bytes_with_nul();
    info.message
        .split_at_mut(message.len())
        .0
        .copy_from_slice(message);
    info.what = Some(operation);
    info.state = *b"IM001\0";
    slot.set(info);
}

/// Append the driver-specific fields of `PDO::errorInfo` without transcoding
/// the driver's message or the source path.
#[unsafe(no_mangle)]
extern "C" fn swr_odbc_fetch_error(
    dbh: &Dbh,
    stmt: Option<&Stmt>,
    output: &swoole_rs_zend::pdo::Slot,
) {
    let info = swr_odbc_error_record(dbh, stmt).get();
    let message = CStr::from_bytes_until_nul(&info.message)
        .unwrap_or(c"")
        .to_bytes();
    if swoole_rs_zend::sys::PHP_VERSION_ID >= 80500 && message.is_empty() {
        return;
    }
    let text = |value: Option<Text<'static>>| value.map_or(c"(null)", Text::get).to_bytes();
    let mut description = message.to_vec();
    description.extend_from_slice(b" (");
    description.extend_from_slice(text(info.what));
    description.extend_from_slice(format!("[{}] at ", info.code).as_bytes());
    description.extend_from_slice(text(info.file));
    description.extend_from_slice(format!(":{})", info.line).as_bytes());
    let state = CStr::from_bytes_until_nul(&info.state).unwrap_or(c"");
    output.edit_array(|fields| {
        fields.push_long(info.code.into());
        fields.push_bytes(&description);
        fields.push_bytes(state.to_bytes());
    });
}
