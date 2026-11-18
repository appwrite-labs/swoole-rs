//! PDO COPY methods over libpq, PHP row producers and PHP streams.
use super::{
    Conn, Dbh, ExecStatusType, drain_copy_results, handle, method_dbh,
    pdo_pgsql_handle_method_error, pgsql_fatal_error, raise,
};
use std::{
    ffi::{CStr, c_int, c_void},
    ptr::NonNull,
};
use swoole_rs_corebase::sys::Text;
use swoole_rs_zend::{Call, Owned, OwnedArray, ZStr, ZString, bailout, contain_with};

swoole_rs_zend::handlers! {
    pgsqlCopyFromArray_internal => from_array;
    zim_PDO_PGSql_Ext_pgsqlCopyFromArray => from_array;
    pgsqlCopyFromFile_internal => from_file;
    zim_PDO_PGSql_Ext_pgsqlCopyFromFile => from_file;
    pgsqlCopyToArray_internal => to_array;
    zim_PDO_PGSql_Ext_pgsqlCopyToArray => to_array;
    pgsqlCopyToFile_internal => to_file;
    zim_PDO_PGSql_Ext_pgsqlCopyToFile => to_file;
}

unsafe extern "C" {
    safe fn pdo_pgsql_copy_row(conn: Conn, row: &mut Owned) -> c_int;
    safe fn pdo_pgsql_copy_open(path: Text<'_>, writing: bool) -> Option<CopyFile>;
    safe fn pdo_pgsql_copy_write(stream: NonNull<c_void>, row: &ZStr) -> bool;
    safe fn pdo_pgsql_copy_line(stream: NonNull<c_void>, row: &mut Owned) -> bool;
    safe fn pdo_pgsql_check_rows(rows: &swoole_rs_zend::Value) -> bool;
    safe fn pdo_pgsql_copy_close(stream: NonNull<c_void>);
}

/// A PHP stream opened by this method, including when the path has a user wrapper.
#[repr(transparent)]
struct CopyFile(NonNull<c_void>);

// User stream wrappers, iterators and libpq notice handlers can bail out.
fn leaf<T>(call: impl FnOnce() -> T) -> Option<T> {
    let mut value = None;
    bailout::defer(contain_with(|| value = Some(call())));
    value
}

impl CopyFile {
    fn open(call: &Call<'_>, dbh: &Dbh, path: &CStr, writing: bool) -> Option<Self> {
        let stream = leaf(|| pdo_pgsql_copy_open(path.into(), writing))?;
        if stream.is_none() {
            error(
                dbh,
                if writing {
                    c"Unable to open the file for writing"
                } else {
                    c"Unable to open the file"
                },
            );
            finish_error(call, dbh);
        }
        stream
    }
}

impl Drop for CopyFile {
    fn drop(&mut self) {
        bailout::defer(contain_with(|| pdo_pgsql_copy_close(self.0)));
    }
}

fn to_array(call: &Call<'_>) {
    copy_out(call, false);
}

fn to_file(call: &Call<'_>) {
    copy_out(call, true);
}

enum Destination {
    File(CopyFile),
    Array(OwnedArray),
}

impl Destination {
    fn append(&mut self, row: &Owned) -> bool {
        match self {
            Self::File(file) => row
                .str()
                .is_some_and(|text| pdo_pgsql_copy_write(file.0, text)),
            Self::Array(array) => {
                array.push(row.copy());
                true
            }
        }
    }
}

fn error(dbh: &Dbh, message: &CStr) {
    raise(
        dbh,
        None,
        ExecStatusType::PGRES_FATAL_ERROR as c_int,
        None,
        Some(message),
    );
}

fn finish_error(call: &Call<'_>, dbh: &Dbh) {
    bailout::defer(contain_with(|| pdo_pgsql_handle_method_error(dbh)));
    call.set_bool(false);
}

struct Options {
    table: ZString,
    fields: Option<ZString>,
    delimiter: u8,
    null_marker: Option<ZString>,
}

impl Options {
    fn parse(call: &Call<'_>, table: ZString, first: u32) -> Option<Self> {
        let delimiter = call
            .zstr(first)
            .ok()?
            .and_then(|s| s.bytes().first())
            .copied()
            .unwrap_or(b'\t');
        let null_marker = call
            .zstr(first + 1)
            .ok()?
            .filter(|s| !s.bytes().is_empty())
            .map(ZStr::to_owned);
        let fields = call.zstr_or_null(first + 2).ok()?.map(ZStr::to_owned);
        Some(Self {
            table,
            fields,
            delimiter,
            null_marker,
        })
    }

    fn start(&self, dbh: &Dbh, direction: ExecStatusType) -> bool {
        super::start_copy(
            dbh,
            self.table.c_str().into(),
            self.fields.as_ref().map(|s| s.c_str().into()),
            self.delimiter,
            self.null_marker.as_ref().map(|s| s.c_str().into()),
            direction,
        )
    }
}

fn copy_out(call: &Call<'_>, file: bool) {
    let required = if file { 2 } else { 1 };
    if call.arity(required, required + 3).is_err() {
        return;
    }
    let Ok(Some(table)) = call.zstr(1) else {
        return;
    };
    let table = table.to_owned();
    let path = if file {
        let Ok(bytes) = call.path(2, b"") else { return };
        let Ok(path) = std::ffi::CString::new(bytes) else {
            return;
        };
        Some(path)
    } else {
        None
    };
    let Some(options) = Options::parse(call, table, required + 1) else {
        return;
    };
    let Some(dbh) = method_dbh(call, true) else {
        return;
    };
    let Some(handle) = handle(dbh) else { return };
    let mut dest = if let Some(path) = path {
        let Some(stream) = CopyFile::open(call, dbh, &path, true) else {
            return;
        };
        Destination::File(stream)
    } else {
        Destination::Array(OwnedArray::default())
    };
    let mut complete = false;
    if options.start(dbh, ExecStatusType::PGRES_COPY_OUT) {
        loop {
            let mut row = Owned::default();
            let Some(count) = leaf(|| pdo_pgsql_copy_row(handle.server, &mut row)) else {
                return;
            };
            if count <= 0 {
                complete = count == -1;
                if !complete {
                    pgsql_fatal_error(dbh, None);
                }
                break;
            }
            let Some(written) = leaf(|| dest.append(&row)) else {
                return;
            };
            if !written {
                error(dbh, c"Unable to write to file");
                break;
            }
        }
    }
    if complete {
        match dest {
            Destination::Array(rows) => call.set_owned(rows.into_owned()),
            Destination::File(stream) => {
                drop(stream);
                call.set_bool(true);
            }
        }
        if !bailout::pending() {
            bailout::defer(contain_with(|| drain_copy_results(handle)));
        }
    } else {
        drop(dest);
        if !bailout::pending() {
            finish_error(call, dbh);
        }
    }
}

fn from_array(call: &Call<'_>) {
    copy_in(call, false);
}
fn from_file(call: &Call<'_>) {
    copy_in(call, true);
}

enum Source {
    Rows(Owned),
    File(CopyFile),
}

impl Source {
    fn send(&self, handle: &super::Handle) -> bool {
        match self {
            Self::Rows(rows) => {
                #[expect(
                    improper_ctypes,
                    reason = "the PDO iterator adapter passes the opaque driver pointer back to the Rust row sender"
                )]
                unsafe extern "C" {
                    safe fn pdo_pgsql_send_rows(
                        handle: &super::Handle,
                        rows: &swoole_rs_zend::Value,
                    ) -> bool;
                }
                leaf(|| pdo_pgsql_send_rows(handle, rows)).unwrap_or(false)
            }
            Self::File(file) => loop {
                let mut line = Owned::default();
                match leaf(|| pdo_pgsql_copy_line(file.0, &mut line)) {
                    None => return false,
                    Some(false) => return true,
                    Some(true) => {}
                }
                let Some(bytes) = line.bytes() else {
                    return false;
                };
                if leaf(|| {
                    super::pdo_pgsql_send_copy_data(handle, std::io::IoSlice::new(bytes), false)
                }) != Some(true)
                {
                    return false;
                }
            },
        }
    }
}

fn copy_in(call: &Call<'_>, file: bool) {
    use swoole_rs_zend::sys::PHP_VERSION_ID;
    if call.arity(2, 5).is_err() {
        return;
    }
    let Ok(Some(table)) = call.zstr(1) else {
        return;
    };
    let table = table.to_owned();
    let Some(arg) = call.value(2) else { return };
    let value = if file {
        let Ok(bytes) = call.path(2, b"") else { return };
        Owned::string(ZString::new(bytes))
    } else if PHP_VERSION_ID < 80500 {
        let Ok(array) = call.array(2) else { return };
        array.referred().copy()
    } else {
        let arg = arg.referred();
        if leaf(|| pdo_pgsql_check_rows(arg)) != Some(true) {
            return;
        }
        arg.copy()
    };
    let Some(options) = Options::parse(call, table, 3) else {
        return;
    };
    if !file && PHP_VERSION_ID < 80500 && value.array().is_some_and(|a| a.count() == 0) {
        call.argument_value_error(2, c"cannot be empty");
        return;
    }
    let Some(dbh) = method_dbh(call, true) else {
        return;
    };
    let Some(handle) = handle(dbh) else { return };
    let source = if file {
        let Some(path) = value.str() else { return };
        let Some(stream) = CopyFile::open(call, dbh, path.c_str(), false) else {
            return;
        };
        Source::File(stream)
    } else {
        Source::Rows(value)
    };
    let started = options.start(dbh, ExecStatusType::PGRES_COPY_IN);
    let mut sent = false;
    if started {
        sent = source.send(handle);
        if !sent && !bailout::pending() && (file || swoole_rs_zend::host::exception().is_null()) {
            pgsql_fatal_error(dbh, None);
        }
    }
    drop(source);
    if started && (!sent || bailout::pending() || !swoole_rs_zend::host::exception().is_null()) {
        bailout::defer(contain_with(|| super::abort_copy_in(handle)));
        if bailout::pending() || !swoole_rs_zend::host::exception().is_null() {
            return;
        }
    }
    if bailout::pending() {
        return;
    }
    let mut complete = false;
    let end = contain_with(|| {
        complete = sent && super::end_copy_in(dbh);
        pdo_pgsql_handle_method_error(dbh);
    });
    bailout::defer(end);
    call.set_bool(complete);
}
