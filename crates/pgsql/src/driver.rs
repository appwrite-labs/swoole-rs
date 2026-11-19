//! The bundled `pdo_pgsql` driver's database handle (upstream's
//! `thirdparty/php8*/pdo_pgsql/pgsql_driver.c`, one port of its four copies): the
//! factory, close, attributes, transactions, `quote`, `lastInsertId` and the
//! libpq-error to SQLSTATE mapping.
//!
//! Statement execution and decoding live in the statement module. The PDO method
//! tables, Zend stream adapters, COPY, large objects and notices use the opaque
//! handle through the accessors declared in the bundled driver headers.
//!
//! libpq runs the notice processor, which from PHP 8.4 calls the user's callback,
//! whenever it reads the server's messages; a bailout there jumps over every frame
//! down to PHP's. So no frame here holds a value with a destructor across a call
//! that reads them (the `swoole_pgsql_*` calls and `PQconsumeInput`).
mod copy;
pub(super) mod lob;

use crate::{
    Conn, OwnedConn, PQclear, PQresultStatus, PQstatus, Params, PgResult, Res, Value as Param,
    swoole_pgsql_finish,
};
use pq_sys::{ConnStatusType, ExecStatusType, PGTransactionStatusType};
use std::{
    cell::Cell,
    ffi::{CStr, CString, c_int, c_void},
    ptr::{self, NonNull},
};
use swoole_rs_corebase::{
    CellExt,
    lent::{Count, In, Out, lend_in, lend_out},
    sys::Text,
};
use swoole_rs_zend::{
    Owned, Value, ZStr, ZString, contain_with,
    pdo::{self, Dbh, Slot, Stmt},
    sys::{
        PHP_VERSION_ID, pdo_attribute_type_PDO_ATTR_CLIENT_VERSION as ATTR_CLIENT_VERSION,
        pdo_attribute_type_PDO_ATTR_CONNECTION_STATUS as ATTR_CONNECTION_STATUS,
        pdo_attribute_type_PDO_ATTR_DRIVER_SPECIFIC as ATTR_DISABLE_PREPARES,
        pdo_attribute_type_PDO_ATTR_EMULATE_PREPARES as ATTR_EMULATE_PREPARES,
        pdo_attribute_type_PDO_ATTR_PREFETCH as ATTR_PREFETCH,
        pdo_attribute_type_PDO_ATTR_SERVER_INFO as ATTR_SERVER_INFO,
        pdo_attribute_type_PDO_ATTR_SERVER_VERSION as ATTR_SERVER_VERSION,
        pdo_attribute_type_PDO_ATTR_TIMEOUT as ATTR_TIMEOUT, pdo_dbh_methods,
        pdo_param_event_PDO_PARAM_EVT_EXEC_POST as EVT_EXEC_POST,
        pdo_param_event_PDO_PARAM_EVT_FETCH_POST as EVT_FETCH_POST,
        pdo_param_event_PDO_PARAM_EVT_FETCH_PRE as EVT_FETCH_PRE,
        pdo_param_type_PDO_PARAM_LOB as PARAM_LOB, zend_long, zend_string,
    },
};

/// `PHP_PDO_PGSQL_CONNECTION_FAILURE_SQLSTATE`.
const CONNECTION_FAILURE: &CStr = c"08006";
/// `PG_DIAG_SQLSTATE`.
const DIAG_SQLSTATE: c_int = b'C' as c_int;

/// `pdo_pgsql_db_handle`: what the driver keeps per connection. The statement,
/// COPY, LOB and notice callbacks share this handle on the connection thread.
pub struct Handle {
    /// The connection, open until the handle drops.
    server: Conn,
    /// `einfo`: the last error's status and message.
    errcode: Cell<u32>,
    errmsg: Cell<Option<CString>>,
    /// Numbers the statements and cursors the handle names.
    stmt_counter: Cell<u32>,
    emulate_prepares: Cell<bool>,
    disable_prepares: Cell<bool>,
    /// PHP 8.5's `default_fetching_laziness`.
    lazy_fetch: Cell<bool>,
    /// Zend allocates resource IDs in creation order. Each stream unregisters on close.
    lob_streams: Cell<Vec<(zend_long, NonNull<c_void>)>>,
    notice_callback: Cell<Option<NonNull<c_void>>>,
    pub(super) running_stmt: Cell<std::rc::Weak<crate::statement::Execution>>,
}

impl Drop for Handle {
    fn drop(&mut self) {
        swoole_pgsql_finish(OwnedConn(self.server.0));
    }
}

/// A buffer libpq allocated, NUL-terminated, until `PQfreemem` takes it.
#[repr(transparent)]
struct Escaped(Text<'static>);

impl Escaped {
    fn text(&self) -> &CStr {
        self.0.get()
    }
}

// libpq's own declarations, each argument typed as what libpq requires of it. None
// parses input, so none runs the notice processor.
unsafe extern "C" {
    safe fn PQnfields(res: Res<'_>) -> c_int;
    safe fn PQntuples(res: Res<'_>) -> c_int;
    pub(crate) safe fn PQgetisnull(res: Res<'_>, row: c_int, column: c_int) -> c_int;
    /// A field of an error result, live as long as the result.
    safe fn PQresultErrorField<'a>(res: Res<'a>, field: c_int) -> Option<Text<'a>>;
    safe fn PQcmdTuples<'a>(res: Res<'a>) -> Text<'a>;
    /// A text value, live as long as the result; none out of range.
    pub(crate) safe fn PQgetvalue<'a>(res: Res<'a>, row: c_int, column: c_int) -> Option<Text<'a>>;
    safe fn PQtransactionStatus(conn: Conn) -> PGTransactionStatusType;
    safe fn PQputCopyData(conn: Conn, data: In<'_, u8>, length: Count<'_, c_int>) -> c_int;
    safe fn PQputCopyEnd(conn: Conn, error: Option<Text<'_>>) -> c_int;
    safe fn lo_creat(conn: Conn, mode: c_int) -> pq_sys::Oid;
    pub(crate) safe fn lo_open(conn: Conn, oid: pq_sys::Oid, mode: c_int) -> c_int;
    safe fn lo_unlink(conn: Conn, oid: pq_sys::Oid) -> c_int;
    safe fn PQprotocolVersion(conn: Conn) -> c_int;
    safe fn PQbackendPID(conn: Conn) -> c_int;
    /// `from`'s lent bytes escaped as a bytea literal, NUL-terminated, with the
    /// count including the NUL in `to_length`; none when libpq can't allocate.
    safe fn PQescapeByteaConn(
        conn: Conn,
        from: In<'_, u8>,
        from_length: Count<'_>,
        to_length: &mut usize,
    ) -> Option<Escaped>;
    /// Free what libpq allocated: it takes the owner.
    safe fn PQfreemem(escaped: Escaped);
}

// PDO and Zend glue in `native/hooks/pgsql_driver.c`.
unsafe extern "C" {
    /// PDO installs this version-specific table on a connected handle.
    safe static swoole_pdo_pgsql_methods: pdo_dbh_methods;
    /// Zend invokes or releases a callable cache, including its trampoline.
    safe fn pdo_pgsql_call_notice(callback: NonNull<c_void>, message: Text<'_>);
    safe fn pdo_pgsql_release_notice(callback: NonNull<c_void>);
}

#[expect(
    improper_ctypes,
    reason = "C never reads the handle: it keeps the pointer and passes it back to the accessors"
)]
unsafe extern "C" {
    /// The handle's driver data, as the factory below set it, or none.
    safe fn swr_zend_pdo_driver_data(dbh: &Dbh) -> Option<&Handle>;
    safe fn pdo_pgsql_take_handle(dbh: &Dbh) -> Option<Box<Handle>>;
}

/// The handle the factory made for `dbh`, if it is still open.
pub(crate) fn handle(dbh: &Dbh) -> Option<&Handle> {
    swr_zend_pdo_driver_data(dbh)
}

/// `dbh`'s connection, if it is still open.
pub(crate) fn server(dbh: &Dbh) -> Option<Conn> {
    handle(dbh).map(|handle| handle.server)
}

/// Install the callback on the connection owned by this database handle.
fn install_notice_processor(dbh: &Dbh) {
    type NoticeProcessor = unsafe extern "C" fn(&Dbh, Text<'_>);
    unsafe extern "C" {
        // libpq keeps the context until the connection is closed. The handle
        // owns that connection and finishes it before PDO frees the context.
        safe fn PQsetNoticeProcessor(
            conn: Conn,
            processor: NoticeProcessor,
            context: &Dbh,
        ) -> Option<NoticeProcessor>;
    }
    if let Some(conn) = server(dbh) {
        PQsetNoticeProcessor(conn, pdo_pgsql_notice, dbh);
    }
}

/// `PQerrorMessage`, copied.
fn error_message(conn: Conn) -> CString {
    unsafe extern "C" {
        /// The connection's last error, which libpq keeps until its next call on the
        /// connection: none comes before the copy below.
        safe fn PQerrorMessage<'a>(conn: Conn) -> Text<'a>;
    }
    PQerrorMessage(conn).get().to_owned()
}

/// `PQparameterStatus`, copied.
fn parameter(conn: Conn, name: &CStr) -> Option<CString> {
    unsafe extern "C" {
        /// The server parameter's value, if the server reported one, which libpq keeps
        /// until its next call on the connection: none comes before the copy below.
        safe fn PQparameterStatus<'a>(conn: Conn, name: Text<'_>) -> Option<Text<'a>>;
    }
    PQparameterStatus(conn, name.into()).map(|value| value.get().to_owned())
}

/// `PDO_ATTR_SERVER_INFO`'s text, a missing parameter printed as `%s` prints null.
fn server_info(conn: Conn) -> Vec<u8> {
    let mut info = format!("PID: {}", PQbackendPID(conn)).into_bytes();
    for (label, name) in [
        ("Client Encoding", c"client_encoding"),
        ("Is Superuser", c"is_superuser"),
        ("Session Authorization", c"session_authorization"),
        ("Date Style", c"DateStyle"),
    ] {
        info.extend_from_slice(format!("; {label}: ").as_bytes());
        let value = parameter(conn, name);
        info.extend_from_slice(value.as_deref().map_or(b"(null)", CStr::to_bytes));
    }
    info
}

impl Handle {
    fn new(server: Conn) -> Self {
        Self {
            server,
            errcode: Cell::new(0),
            errmsg: Cell::new(None),
            stmt_counter: Cell::new(0),
            emulate_prepares: Cell::new(false),
            disable_prepares: Cell::new(false),
            lazy_fetch: Cell::new(false),
            lob_streams: Cell::default(),
            notice_callback: Cell::new(None),
            running_stmt: Cell::default(),
        }
    }

    /// `PQexec`, with the hook's waits.
    pub(crate) fn exec(&self, query: &CStr) -> Option<PgResult> {
        NonNull::new(crate::swoole_pgsql_exec(self.server, query.into())).map(PgResult)
    }

    /// Read a single catalogue name, copying it before releasing its result.
    pub(crate) fn catalogue_name(
        &self,
        query: &CStr,
    ) -> Result<Option<CString>, swoole_rs_zend::bailout::Bailout> {
        let mut result = None;
        contain_with(|| result = self.exec(query))?;
        let Some(result) = result else {
            return Ok(None);
        };
        let res = result.get();
        let name = if PQresultStatus(res) == ExecStatusType::PGRES_TUPLES_OK
            && PQntuples(res) == 1
            && PQgetisnull(res, 0, 0) == 0
        {
            PQgetvalue(res, 0, 0).map(|name| name.get().to_owned())
        } else {
            None
        };
        PQclear(result);
        Ok(name)
    }

    /// `pgsql_handle_in_transaction`.
    pub(super) fn in_transaction(&self) -> bool {
        PQtransactionStatus(self.server) != PGTransactionStatusType::PQTRANS_IDLE
    }
}

/// `pdo_pgsql_sqlstate`: an error result's SQLSTATE.
pub(crate) fn sqlstate(result: Option<&PgResult>) -> Option<&CStr> {
    PQresultErrorField(result?.get(), DIAG_SQLSTATE).map(Text::get)
}

/// `PQresultStatus`, which is `PGRES_FATAL_ERROR` for no result.
fn status(result: Option<&PgResult>) -> ExecStatusType {
    result.map_or(ExecStatusType::PGRES_FATAL_ERROR, |result| {
        PQresultStatus(result.get())
    })
}

fn affected_rows(result: Res<'_>) -> zend_long {
    swoole_rs_corebase::strtol(PQcmdTuples(result).get().to_bytes()).0
}

/// Publish the shape of a successful execute, or preserve the server's SQLSTATE.
pub(super) fn execution_result(dbh: &Dbh, stmt: &Stmt, result: Option<Res<'_>>) -> bool {
    let Some(res) = result else {
        raise(
            dbh,
            Some(stmt),
            ExecStatusType::PGRES_FATAL_ERROR as c_int,
            None,
            None,
        );
        return false;
    };
    let code = PQresultStatus(res);
    let rows = match code {
        ExecStatusType::PGRES_COMMAND_OK => affected_rows(res),
        ExecStatusType::PGRES_TUPLES_OK => i64::from(PQntuples(res)),
        ExecStatusType::PGRES_SINGLE_TUPLE if PHP_VERSION_ID >= 80500 => i64::from(PQntuples(res)),
        _ => {
            raise(
                dbh,
                Some(stmt),
                code as c_int,
                PQresultErrorField(res, DIAG_SQLSTATE).map(Text::get),
                None,
            );
            return false;
        }
    };
    stmt.set_result_counts(PQnfields(res), rows);
    true
}

/// Install the statement result before reporting a PDO error, as the statement owns it.
pub(crate) fn statement_result(
    dbh: &Dbh,
    stmt: &Stmt,
    slot: &Cell<Option<PgResult>>,
    result: Option<PgResult>,
) -> Option<c_int> {
    let code = status(result.as_ref());
    if matches!(
        code,
        ExecStatusType::PGRES_COMMAND_OK
            | ExecStatusType::PGRES_TUPLES_OK
            | ExecStatusType::PGRES_SINGLE_TUPLE
    ) {
        let rows = result.as_ref().map(|result| PQntuples(result.get()));
        slot.set(result);
        return rows;
    }
    let state = sqlstate(result.as_ref()).map(CStr::to_owned);
    slot.set(result);
    raise(dbh, Some(stmt), code as c_int, state.as_deref(), None);
    None
}

/// `_pdo_pgsql_trim_message`: libpq's message without its trailing line breaks,
/// and a final period after one.
fn trim_message(message: &[u8]) -> &[u8] {
    let Some(mut i) = message.len().checked_sub(1) else {
        return message;
    };
    let at = |i: usize| message.get(i).copied().unwrap_or_default();
    if i > 1 && matches!(at(i - 1), b'\r' | b'\n') && at(i) == b'.' {
        i -= 1;
    }
    while i > 0 && matches!(at(i), b'\r' | b'\n') {
        i -= 1;
    }
    message.get(..=i).unwrap_or(message)
}

/// `_pdo_pgsql_error`: record the error on the handle and set the SQLSTATE of
/// `stmt`, or of the handle; while PDO's constructor runs (no methods yet), throw
/// it. Interrupted I/O reports itself as a connection failure.
fn raise(
    dbh: &Dbh,
    stmt: Option<&Stmt>,
    errcode: c_int,
    sqlstate: Option<&CStr>,
    message: Option<&CStr>,
) {
    let Some(handle) = handle(dbh) else {
        return;
    };
    let interrupted = crate::pdo::io_error(dbh, handle.server);
    let (sqlstate, message) = match &interrupted {
        Some(text) => (Some(CONNECTION_FAILURE), Some(&**text)),
        None => (sqlstate, message),
    };
    let errcode = errcode.cast_unsigned();
    handle.errcode.set(errcode);
    let code = sqlstate
        .filter(|state| state.count_bytes() < size_of::<swoole_rs_zend::sys::pdo_error_type>())
        .unwrap_or(c"HY000");
    match stmt {
        Some(stmt) => stmt.set_error_code(code),
        None => dbh.set_error_code(code),
    }
    let message = message.map_or_else(
        || {
            let text = error_message(handle.server);
            CString::new(trim_message(text.as_bytes())).unwrap_or_default()
        },
        CStr::to_owned,
    );
    if !dbh.has_methods() {
        dbh.throw(errcode, &message);
    }
    handle.errmsg.set(Some(message));
}

/// Close the handle's LOB streams, which may run PHP code: a bailout is deferred to
/// the export's boundary.
pub(super) fn close_lob_streams(dbh: &Dbh) -> Result<(), swoole_rs_zend::bailout::Bailout> {
    unsafe extern "C" {
        safe fn pdo_pgsql_close_resource(resource: NonNull<c_void>);
    }
    let Some(handle) = handle(dbh) else {
        return Ok(());
    };
    let mut before = None;
    loop {
        let next = handle.lob_streams.with_taken(|streams| {
            let end = before.map_or(streams.len(), |id| {
                streams.partition_point(|&(key, _)| key < id)
            });
            end.checked_sub(1)
                .and_then(|index| streams.get(index))
                .copied()
        });
        let Some((id, resource)) = next else { break };
        before = Some(id);
        contain_with(|| pdo_pgsql_close_resource(resource))?;
    }
    Ok(())
}

/// `pdo_pgsql_transaction_cmd`.
fn transaction(dbh: &Dbh, handle: &Handle, command: &CStr) -> bool {
    let result = handle.exec(command);
    let status = status(result.as_ref());
    let done = status == ExecStatusType::PGRES_COMMAND_OK;
    if !done {
        raise(dbh, None, status as c_int, sqlstate(result.as_ref()), None);
    }
    if let Some(result) = result {
        PQclear(result);
    }
    done
}

/// ` key='value'`, the value escaped as `_pdo_pgsql_escape_credentials` does
/// (`addcslashes(value, "\\'")`).
fn push_credential(info: &mut Vec<u8>, key: &str, value: &[u8]) {
    info.extend_from_slice(format!(" {key}='").as_bytes());
    for &byte in value {
        if matches!(byte, b'\\' | b'\'') {
            info.push(b'\\');
        }
        info.push(byte);
    }
    info.push(b'\'');
}

/// The connection string the factory hands libpq: the data source with PDO's
/// semicolons as spaces, the timeout and the credentials PDO was given. Before PHP
/// 8.4 the credentials always come first; from 8.4 they follow the timeout and
/// only where the data source names none.
fn conninfo(
    data_source: &[u8],
    username: Option<&[u8]>,
    password: Option<&[u8]>,
    connect_timeout: zend_long,
) -> CString {
    let mut info: Vec<u8> = data_source
        .iter()
        .map(|&byte| if byte == b';' { b' ' } else { byte })
        .collect();
    let named = |key: &[u8]| data_source.windows(key.len()).any(|window| window == key);
    let timeout = format!(" connect_timeout={connect_timeout}");
    let first = PHP_VERSION_ID < 80400;
    if !first {
        info.extend_from_slice(timeout.as_bytes());
    }
    for (key, value) in [("user", username), ("password", password)] {
        let named = !first && named(format!("{key}=").as_bytes());
        if let Some(value) = value.filter(|_| !named) {
            push_credential(&mut info, key, value);
        }
    }
    if first {
        info.extend_from_slice(timeout.as_bytes());
    }
    CString::new(info).unwrap_or_default()
}

/// `PDO_ATTR_CONNECTION_STATUS`'s text. Upstream's other cases sit behind
/// `#ifdef`s on libpq's enum constants, which no macro defines.
fn connection_status(status: ConnStatusType) -> &'static [u8] {
    match status {
        ConnStatusType::CONNECTION_STARTED => b"Waiting for connection to be made.",
        ConnStatusType::CONNECTION_MADE | ConnStatusType::CONNECTION_OK => {
            b"Connection OK; waiting to send."
        }
        ConnStatusType::CONNECTION_AWAITING_RESPONSE => b"Waiting for a response from the server.",
        ConnStatusType::CONNECTION_AUTH_OK => {
            b"Received authentication; waiting for backend start-up to finish."
        }
        ConnStatusType::CONNECTION_SETENV => b"Negotiating environment-driven parameter settings.",
        _ => b"Bad connection.",
    }
}

/// The factory's answer, as PDO reads its `int`.
#[repr(i32)]
#[derive(Clone, Copy)]
pub enum Connected {
    Failed = 0,
    Open = 1,
}

/// `get_attribute`'s answer, as PDO reads its `int`.
#[repr(i32)]
#[derive(Clone, Copy)]
pub enum Attribute {
    Unknown = 0,
    Set = 1,
}

swoole_rs_zend::exports! {
/// `pdo_pgsql_handle_factory`, given the handle's data source and credentials:
/// connect, and install the handle and its methods.
#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_connect(
    dbh: &Dbh,
    data_source: Text<'_>,
    username: Option<Text<'_>>,
    password: Option<Text<'_>>,
    driver_options: Option<&Value>,
) -> Connected {
    let skip = (1 << EVT_EXEC_POST) | (1 << EVT_FETCH_PRE) | (1 << EVT_FETCH_POST);
    dbh.set_driver_flags(skip, 2);
    let connect_timeout = pdo::attr_long(driver_options, ATTR_TIMEOUT, 30);
    let info = conninfo(
        data_source.get().to_bytes(),
        username.map(|text| text.get().to_bytes()),
        password.map(|text| text.get().to_bytes()),
        connect_timeout,
    );
    let server = Conn(crate::swoole_pgsql_connectdb(info.as_c_str().into()));
    drop(info);
    dbh.set_driver_data(Box::into_raw(Box::new(Handle::new(server))).cast());
    let connected = PQstatus(server) == ConnStatusType::CONNECTION_OK;
    if connected {
        install_notice_processor(dbh);
    } else {
        raise(
            dbh,
            None,
            ExecStatusType::PGRES_FATAL_ERROR as c_int,
            Some(CONNECTION_FAILURE),
            None,
        );
    }
    dbh.set_methods(&swoole_pdo_pgsql_methods);
    if !connected {
        close_handle(dbh);
        return Connected::Failed;
    }
    Connected::Open
}

/// `pgsql_handle_doer`: run `sql`; the rows it changed, or -1.
#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_handle_doer(dbh: &Dbh, sql: &ZStr) -> zend_long {
    let Some(handle) = handle(dbh) else {
        return -1;
    };
    let in_transaction = handle.in_transaction();
    let Some(result) = handle.exec(sql.c_str()) else {
        raise(dbh, None, ExecStatusType::PGRES_FATAL_ERROR as c_int, None, None);
        return -1;
    };
    let status = PQresultStatus(result.get());
    if !matches!(status, ExecStatusType::PGRES_COMMAND_OK | ExecStatusType::PGRES_TUPLES_OK) {
        raise(dbh, None, status as c_int, sqlstate(Some(&result)), None);
        PQclear(result);
        return -1;
    }
    let changed = if status == ExecStatusType::PGRES_COMMAND_OK {
        affected_rows(result.get())
    } else {
        0
    };
    PQclear(result);
    if in_transaction && !handle.in_transaction() {
        swoole_rs_zend::bailout::defer(close_lob_streams(dbh));
    }
    changed
}

/// `pgsql_handle_quoter`: `unquoted` as a literal, or none when libpq can't
/// escape it (PHP 8.2 and 8.5 check; 8.3 and 8.4 keep what it wrote).
#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_handle_quoter(
    dbh: &Dbh,
    unquoted: &ZStr,
    paramtype: u32,
) -> *mut zend_string {
    let Some(handle) = handle(dbh) else {
        return ptr::null_mut();
    };
    quote(handle.server, unquoted.bytes(), paramtype == PARAM_LOB)
        .map_or(ptr::null_mut(), |quoted| ZString::new(&quoted).into_raw())
}

/// `pgsql_handle_begin`.
#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_handle_begin(dbh: &Dbh) -> bool {
    handle(dbh).is_some_and(|handle| transaction(dbh, handle, c"BEGIN"))
}

/// `pgsql_handle_commit`: a commit that failed may have rolled back (deferred
/// constraints, PHP bug #67462), so PDO is told where the transaction stands.
#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_handle_commit(dbh: &Dbh) -> bool {
    let Some(handle) = handle(dbh) else {
        return false;
    };
    let committed = transaction(dbh, handle, c"COMMIT");
    if committed {
        swoole_rs_zend::bailout::defer(close_lob_streams(dbh));
    } else {
        dbh.set_in_txn(handle.in_transaction());
    }
    committed
}

/// `pgsql_handle_rollback`.
#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_handle_rollback(dbh: &Dbh) -> bool {
    let rolled_back = handle(dbh).is_some_and(|handle| transaction(dbh, handle, c"ROLLBACK"));
    if rolled_back {
        swoole_rs_zend::bailout::defer(close_lob_streams(dbh));
    }
    rolled_back
}

/// `pdo_pgsql_set_attr`.
#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_set_attr(dbh: &Dbh, attr: zend_long, value: &Value) -> bool {
    let Some(handle) = handle(dbh) else {
        return false;
    };
    let (cell, negate) = match u32::try_from(attr) {
        Ok(ATTR_EMULATE_PREPARES) => (&handle.emulate_prepares, false),
        Ok(ATTR_DISABLE_PREPARES) => (&handle.disable_prepares, false),
        // PHP 8.5's: statements fetch lazily unless they prefetch.
        Ok(ATTR_PREFETCH) if PHP_VERSION_ID >= 80500 => (&handle.lazy_fetch, true),
        _ => return false,
    };
    pdo::bool_param(value).is_some_and(|set| {
        cell.set(set != negate);
        true
    })
}

/// `pdo_pgsql_last_insert_id`: `LASTVAL()`, or `CURRVAL(name)`.
#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_last_insert_id(dbh: &Dbh, name: Option<&ZStr>) -> *mut zend_string {
    let Some(handle) = handle(dbh) else {
        return ptr::null_mut();
    };
    let result = match name {
        None => handle.exec(c"SELECT LASTVAL()"),
        Some(name) => {
            let values = [Param(name.c_str().as_ptr())];
            NonNull::new(crate::swoole_pgsql_exec_params(
                handle.server,
                Some(c"SELECT CURRVAL($1)".into()),
                1,
                Params(ptr::null()),
                Params(values.as_ptr()),
                Params(ptr::null()),
                Params(ptr::null()),
                0,
            ))
            .map(PgResult)
        }
    };
    let status = status(result.as_ref());
    let id = match &result {
        Some(result) if status == ExecStatusType::PGRES_TUPLES_OK => {
            PQgetvalue(result.get(), 0, 0).map(|id| ZString::new(id.get().to_bytes()).into_raw())
        }
        _ => {
            raise(dbh, None, status as c_int, sqlstate(result.as_ref()), None);
            None
        }
    };
    if let Some(result) = result {
        PQclear(result);
    }
    id.unwrap_or(ptr::null_mut())
}

/// `pdo_pgsql_fetch_error_func`: the driver's code (null for none) and message.
#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_fetch_error_func(dbh: &Dbh, _stmt: Option<&Stmt>, info: &Slot) {
    let Some(handle) = handle(dbh) else {
        return;
    };
    let code = handle.errcode.get();
    let message = handle.errmsg.take();
    info.push_error_info((code != 0).then_some(code), message.as_deref().map(CStr::to_bytes));
    handle.errmsg.set(message);
}

/// `pdo_pgsql_get_attribute`.
#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_get_attribute(dbh: &Dbh, attr: zend_long, value: &Slot) -> Attribute {
    let Some(handle) = handle(dbh) else {
        return Attribute::Unknown;
    };
    attribute(handle, attr, value)
}

/// `pgsql_handle_in_transaction`.
#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_handle_in_transaction(dbh: &Dbh) -> bool {
    handle(dbh).is_some_and(Handle::in_transaction)
}
}

/// The quoted literal of `unquoted`, as a bytea for a LOB; none when libpq fails
/// and the PHP version checks.
fn quote(conn: Conn, unquoted: &[u8], lob: bool) -> Option<Vec<u8>> {
    let checked = PHP_VERSION_ID < 80300 || PHP_VERSION_ID >= 80500;
    let mut quoted = vec![b'\''];
    if lob {
        let mut length = 0;
        // The count PQescapeByteaConn gives includes the NUL.
        let escaped = lend_in(unquoted, |from, count| {
            PQescapeByteaConn(conn, from, count, &mut length)
        })?;
        quoted.extend_from_slice(escaped.text().to_bytes());
        PQfreemem(escaped);
    } else {
        unsafe extern "C" {
            /// `length` bytes of `from` escaped into `to`, NUL-terminated: at most
            /// `2 * length + 1` bytes, which the one call below lends `to` for.
            safe fn PQescapeStringConn(
                conn: Conn,
                to: Out<'_, u8>,
                from: In<'_, u8>,
                length: Count<'_>,
                error: &mut c_int,
            ) -> usize;
        }
        let mut to = vec![0u8; unquoted.len() * 2 + 1];
        let mut error = 0;
        let written = lend_in(unquoted, |from, length| {
            lend_out(&mut to, |to, _: Count<'_>| {
                PQescapeStringConn(conn, to, from, length, &mut error)
            })
        });
        if checked && error != 0 {
            return None;
        }
        quoted.extend_from_slice(to.get(..written).unwrap_or_default());
    }
    quoted.push(b'\'');
    Some(quoted)
}

/// Set `slot` to the attribute `attr`'s value, unless libpq has none.
fn attribute(handle: &Handle, attr: zend_long, slot: &Slot) -> Attribute {
    let conn = handle.server;
    let text = |bytes: &[u8]| Some(Owned::string(ZString::new(bytes)));
    let Ok(attr) = u32::try_from(attr) else {
        return Attribute::Unknown;
    };
    let value = match attr {
        ATTR_EMULATE_PREPARES => Some(Owned::bool(handle.emulate_prepares.get())),
        ATTR_DISABLE_PREPARES => Some(Owned::bool(handle.disable_prepares.get())),
        ATTR_CLIENT_VERSION => text(crate::version().as_bytes()),
        // PHP 8.4 dropped the query for servers before protocol 3 (PostgreSQL 7.4).
        ATTR_SERVER_VERSION if PHP_VERSION_ID < 80400 && PQprotocolVersion(conn) < 3 => {
            let result = handle.exec(c"SELECT VERSION()");
            let version = result
                .as_ref()
                .filter(|result| PQresultStatus(result.get()) == ExecStatusType::PGRES_TUPLES_OK)
                .and_then(|result| PQgetvalue(result.get(), 0, 0))
                .and_then(|version| text(version.get().to_bytes()));
            if let Some(result) = result {
                PQclear(result);
            }
            version
        }
        ATTR_SERVER_VERSION => {
            parameter(conn, c"server_version").and_then(|version| text(version.as_bytes()))
        }
        ATTR_CONNECTION_STATUS => text(connection_status(PQstatus(conn))),
        ATTR_SERVER_INFO => text(&server_info(conn)),
        _ => return Attribute::Unknown,
    };
    if let Some(value) = value {
        slot.set(value);
    }
    Attribute::Set
}

/// `_pdo_pgsql_error` for the C statement, COPY and LOB code (the
/// `pdo_pgsql_error*` macros).
#[unsafe(no_mangle)]
pub extern "C" fn pdo_pgsql_raise(
    dbh: &Dbh,
    stmt: Option<&Stmt>,
    errcode: c_int,
    sqlstate: Option<Text<'_>>,
    message: Option<Text<'_>>,
) {
    raise(
        dbh,
        stmt,
        errcode,
        sqlstate.map(Text::get),
        message.map(Text::get),
    );
}

/// Close the stream resources before the callback and connection they can use.
fn close_handle(dbh: &Dbh) {
    let Some(handle) = handle(dbh) else { return };
    if let Err(bailout) = close_lob_streams(dbh) {
        bailout.defer();
        return;
    }
    swoole_rs_zend::bailout::defer(replace_notice(handle, None));
    drop(pdo_pgsql_take_handle(dbh));
}

swoole_rs_zend::exports! {
    preserve_pending:
    #[unsafe(no_mangle)]
    extern "C" fn pdo_pgsql_handle_closer(dbh: &Dbh) {
        close_handle(dbh);
    }
}

/// Send borrowed input to COPY, appending a missing newline for array rows.
#[unsafe(no_mangle)]
pub extern "C" fn pdo_pgsql_send_copy_data(
    handle: &Handle,
    bytes: std::io::IoSlice<'_>,
    array_row: bool,
) -> bool {
    let send = |data: &[u8]| {
        lend_in(data, |buffer, count| {
            PQputCopyData(handle.server, buffer, count) == 1
        })
    };
    bytes.chunks(c_int::MAX as usize).all(send)
        && (!array_row || bytes.is_empty() || bytes.ends_with(b"\n") || send(b"\n"))
}

/// Clear pending results before COPY starts or after COPY TO has sent its last row.
fn drain_copy_results(handle: &Handle) {
    while let Some(result) = crate::PQgetResult(handle.server) {
        PQclear(result);
    }
}

/// Build the legacy COPY command accepted by the supported PostgreSQL versions.
/// Fields and identifiers arrive as SQL fragments, exactly as PDO received them.
fn copy_query(
    table: Text<'_>,
    fields: Option<Text<'_>>,
    delimiter: u8,
    null_marker: Option<Text<'_>>,
    direction: ExecStatusType,
) -> ZString {
    let parts: &[&[u8]] = &[
        b"COPY ",
        table.get().to_bytes(),
        if fields.is_some() { b" (" } else { b"" },
        fields.map_or(&[][..], |fields| fields.get().to_bytes()),
        if fields.is_some() { b")" } else { b"" },
        if direction == ExecStatusType::PGRES_COPY_IN {
            b" FROM STDIN"
        } else {
            b" TO STDIN"
        },
        b" WITH DELIMITER E'",
        &[delimiter],
        b"' NULL AS E'",
        null_marker.map_or(br"\\N", |marker| marker.get().to_bytes()),
        b"'",
    ];
    let mut sql =
        swoole_rs_zend::string::ZBuf::with_capacity(parts.iter().map(|part| part.len()).sum());
    for part in parts {
        sql.extend_from_slice(part);
    }
    sql.into_zstring()
}

fn start_copy(
    dbh: &Dbh,
    table: Text<'_>,
    fields: Option<Text<'_>>,
    delimiter: u8,
    null_marker: Option<Text<'_>>,
    direction: ExecStatusType,
) -> bool {
    let Some(handle) = handle(dbh) else {
        return false;
    };
    if let Err(bailout) = contain_with(|| drain_copy_results(handle)) {
        bailout.defer();
        return false;
    }
    let query = copy_query(table, fields, delimiter, null_marker, direction);
    let mut result = None;
    if let Err(bailout) = contain_with(|| result = handle.exec(query.c_str())) {
        bailout.defer();
        return false;
    }
    drop(query);
    let started = status(result.as_ref()) == direction;
    if !started {
        pgsql_fatal_error(dbh, sqlstate(result.as_ref()));
    }
    if let Some(result) = result {
        PQclear(result);
    }
    started
}

/// Roll back an incomplete COPY when a PHP row producer stops with an error.
fn abort_copy_in(handle: &Handle) {
    if PQputCopyEnd(handle.server, Some(c"client aborted COPY".into())) == 1 {
        drain_copy_results(handle);
    }
}

/// Finish COPY FROM, consuming every final result and preserving server SQLSTATEs.
fn end_copy_in(dbh: &Dbh) -> bool {
    let Some(handle) = handle(dbh) else {
        return false;
    };
    if PQputCopyEnd(handle.server, None) != 1 {
        pgsql_fatal_error(dbh, None);
        return false;
    }
    let mut complete = true;
    while let Some(result) = crate::PQgetResult(handle.server) {
        if PQresultStatus(result.get()) != ExecStatusType::PGRES_COMMAND_OK {
            pgsql_fatal_error(dbh, sqlstate(Some(&result)));
            complete = false;
        }
        PQclear(result);
    }
    complete
}

// Access flags from libpq/libpq-fs.h.
pub(crate) const INV_READ: c_int = 0x40000;
const INV_WRITE: c_int = 0x20000;

fn pgsql_fatal_error(dbh: &Dbh, state: Option<&CStr>) {
    raise(
        dbh,
        None,
        ExecStatusType::PGRES_FATAL_ERROR as c_int,
        state,
        None,
    );
}

/// Retain PDO's `strtoul` conversion, including unsigned OID truncation.
fn lob_oid(text: &ZStr) -> Option<pq_sys::Oid> {
    use swoole_rs_corebase::{CCast, sys};
    let oid: pq_sys::Oid = sys::strtoul(text.c_str(), 10).0.c_cast();
    (oid != 0 || !matches!(sys::errno(), libc::ERANGE | libc::EINVAL)).then_some(oid)
}

fn lob_error(call: &swoole_rs_zend::Call<'_>, dbh: &Dbh) {
    pgsql_fatal_error(dbh, None);
    swoole_rs_zend::bailout::defer(contain_with(|| pdo_pgsql_handle_method_error(dbh)));
    call.set_bool(false);
}

fn lob_create(call: &swoole_rs_zend::Call<'_>) {
    if call.arity(0, 0).is_err() {
        return;
    }
    let Some(dbh) = method_dbh(call, true) else {
        return;
    };
    let Some(conn) = server(dbh) else { return };
    let mut oid = 0;
    if let Err(bailout) = contain_with(|| oid = lo_creat(conn, INV_READ | INV_WRITE)) {
        bailout.defer();
    } else if oid == 0 {
        lob_error(call, dbh);
    } else {
        call.set_bytes(oid.to_string().as_bytes());
    }
}

fn lob_open(call: &swoole_rs_zend::Call<'_>) {
    if call.arity(1, 2).is_err() {
        return;
    }
    let Ok(Some(text)) = call.zstr(1) else { return };
    let Ok(mode) = call.zstr(2) else { return };
    let Some(oid) = lob_oid(text) else {
        call.set_bool(false);
        return;
    };
    let Some(dbh) = method_dbh(call, true) else {
        return;
    };
    let Some(conn) = server(dbh) else { return };
    let writable = mode.is_some_and(|s| {
        s.c_str()
            .to_bytes()
            .iter()
            .any(|b| matches!(b, b'+' | b'w'))
    });
    let flags = if writable {
        INV_READ | INV_WRITE
    } else {
        INV_READ
    };
    let mut fd = -1;
    if let Err(bailout) = contain_with(|| fd = lo_open(conn, oid, flags)) {
        bailout.defer();
        return;
    }
    if fd < 0 {
        lob_error(call, dbh);
        return;
    }
    let mut stream = Owned::default();
    let mut opened = false;
    if let Err(bailout) =
        contain_with(|| opened = pdo_pgsql_lob_resource(call.call_frame(), fd, oid, &mut stream))
    {
        bailout.defer();
    } else if opened {
        call.set_owned(stream);
    } else {
        swoole_rs_zend::bailout::defer(contain_with(|| pdo_pgsql_handle_method_error(dbh)));
        call.set_bool(false);
    }
}

fn lob_unlink(call: &swoole_rs_zend::Call<'_>) {
    if call.arity(1, 1).is_err() {
        return;
    }
    let Ok(Some(text)) = call.zstr(1) else { return };
    let Some(oid) = lob_oid(text) else {
        call.set_bool(false);
        return;
    };
    let Some(dbh) = method_dbh(call, true) else {
        return;
    };
    let Some(conn) = server(dbh) else { return };
    let mut removed = false;
    if let Err(bailout) = contain_with(|| removed = lo_unlink(conn, oid) == 1) {
        bailout.defer();
    } else if removed {
        call.set_bool(true);
    } else {
        lob_error(call, dbh);
    }
}

/// `H->server`.
pub(super) fn pdo_pgsql_server(handle: &Handle) -> Conn {
    handle.server
}

/// `++H->stmt_counter`: the next statement or cursor number.
fn next_stmt_id(handle: &Handle) -> u32 {
    let id = handle.stmt_counter.get().wrapping_add(1);
    handle.stmt_counter.set(id);
    id
}

swoole_rs_zend::exports! {
/// Install the statement and select cursor, emulated or native preparation
/// before PDO rewrites its placeholders.
#[unsafe(no_mangle)]
pub extern "C" fn pgsql_handle_preparer(dbh: &Dbh, sql: &ZStr, stmt: &Stmt, options: Option<&Value>) -> bool {
    use swoole_rs_zend::sys::{
        pdo_attribute_type_PDO_ATTR_CURSOR as CURSOR, pdo_cursor_type_PDO_CURSOR_FWDONLY as FORWARD,
        pdo_cursor_type_PDO_CURSOR_SCROLL as SCROLL,
    };
    let Some(handle) = handle(dbh) else {return false};
    let state = crate::statement::install(stmt);
    let cursor = pdo::attr_long(options, CURSOR, FORWARD.into()) == i64::from(SCROLL);
    let mut emulate = cursor;
    let mut direct = false;
    if cursor {
        let name = swoole_rs_zend::message!("pdo_crsr_{:08x}", next_stmt_id(handle));
        state.execution.cursor.get_or_init(|| name.as_cstr().to_owned());
    } else {
        emulate = pdo::attr_long(options, ATTR_EMULATE_PREPARES, handle.emulate_prepares.get().into()) == 1;
        direct = pdo::attr_long(options, ATTR_DISABLE_PREPARES, handle.disable_prepares.get().into()) == 1;
    }
    emulate |= PHP_VERSION_ID < 80400 && PQprotocolVersion(handle.server) <= 2;
    let unbuffered = PHP_VERSION_ID >= 80500 && options.and_then(Value::array)
        .and_then(|opts| opts.index(ATTR_PREFETCH.into()))
        .and_then(pdo::long_param)
        .map_or_else(|| handle.lazy_fetch.get(), |value| value == 0);
    if swoole_rs_zend::bailout::pending() { return false; }
    state.execution.unbuffered.set(unbuffered && !cursor);
    let parsed = state.parse(stmt, sql, emulate);
    if parsed && !emulate && !direct {
        let name = swoole_rs_zend::message!("pdo_stmt_{:08x}", next_stmt_id(handle));
        state.execution.name.get_or_init(|| name.as_cstr().to_owned());
    }
    parsed
}
}

// Both the PDO subclass and the older dynamically registered methods enter
// through the same Rust handlers.
swoole_rs_zend::handlers! {
    pgsqlLOBCreate_internal => lob_create;
    zim_PDO_PGSql_Ext_pgsqlLOBCreate => lob_create;
    pgsqlLOBOpen_internal => lob_open;
    zim_PDO_PGSql_Ext_pgsqlLOBOpen => lob_open;
    pgsqlLOBUnlink_internal => lob_unlink;
    zim_PDO_PGSql_Ext_pgsqlLOBUnlink => lob_unlink;
    pgsqlGetNotify_internal => get_notify;
    zim_PDO_PGSql_Ext_pgsqlGetNotify => get_notify;
    pgsqlGetPid_internal => get_pid;
    zim_PDO_PGSql_Ext_pgsqlGetPid => get_pid;
    pgsql_escape_identifier => escape_identifier;
}

unsafe extern "C" {
    safe fn pdo_pgsql_method_dbh<'a>(
        frame: swoole_rs_zend::CallFrame<'a>,
        out: &mut Option<&'a Dbh>,
    );
    safe fn pdo_pgsql_lob_resource(
        frame: swoole_rs_zend::CallFrame<'_>,
        fd: c_int,
        oid: pq_sys::Oid,
        stream: &mut Owned,
    ) -> bool;
    safe fn pdo_pgsql_clear_method_error(dbh: &Dbh);
    safe fn pdo_pgsql_handle_method_error(dbh: &Dbh);
    safe fn PQescapeIdentifier(conn: Conn, bytes: In<'_, u8>, length: Count<'_>)
    -> Option<Escaped>;
}

/// Only PDO's methods call this. The frame owns the database object while the
/// borrowed handle is in use.
fn method_dbh<'a>(call: &'a swoole_rs_zend::Call<'_>, clear: bool) -> Option<&'a Dbh> {
    let mut dbh = None;
    let checked = contain_with(|| {
        pdo_pgsql_method_dbh(call.call_frame(), &mut dbh);
        if clear && let Some(dbh) = dbh {
            pdo_pgsql_clear_method_error(dbh);
        }
    });
    if let Err(bailout) = checked {
        bailout.defer();
        return None;
    }
    dbh
}

fn get_pid(call: &swoole_rs_zend::Call<'_>) {
    if call.arity(0, 0).is_ok()
        && let Some(conn) = method_dbh(call, false).and_then(server)
    {
        call.set_long(PQbackendPID(conn).into());
    }
}

fn escape_identifier(call: &swoole_rs_zend::Call<'_>) {
    if call.arity(1, 1).is_err() {
        return;
    }
    let Ok(Some(input)) = call.zstr(1) else {
        return;
    };
    let Some(dbh) = method_dbh(call, true) else {
        return;
    };
    let Some(conn) = server(dbh) else { return };
    if let Some(quoted) = lend_in(input.bytes(), |bytes, length| {
        PQescapeIdentifier(conn, bytes, length)
    }) {
        call.set_bytes(quoted.text().to_bytes());
        PQfreemem(quoted);
    } else {
        pgsql_fatal_error(dbh, None);
        swoole_rs_zend::bailout::defer(contain_with(|| pdo_pgsql_handle_method_error(dbh)));
    }
}

#[derive(Default)]
struct Notification {
    channel: Owned,
    pid: c_int,
    payload: Owned,
}

impl Notification {
    fn take(&mut self, conn: Conn) -> bool {
        unsafe extern "C" {
            safe fn pdo_pgsql_take_notify(
                conn: Conn,
                channel: &mut Owned,
                pid: &mut c_int,
                payload: &mut Owned,
            ) -> bool;
        }
        pdo_pgsql_take_notify(conn, &mut self.channel, &mut self.pid, &mut self.payload)
    }

    fn into_array(self, numeric: bool, named: bool) -> Owned {
        let mut array = swoole_rs_zend::OwnedArray::default();
        if numeric {
            array.push(self.channel.copy());
            array.push_long(self.pid.into());
            if self.payload.bytes().is_some() {
                array.push(self.payload.copy());
            }
        }
        if named {
            array.insert(b"message", self.channel);
            array.insert_long(b"pid", self.pid.into());
            if self.payload.bytes().is_some() {
                array.insert(b"payload", self.payload);
            }
        }
        array.into_owned()
    }
}

fn get_notify(call: &swoole_rs_zend::Call<'_>) {
    use swoole_rs_zend::sys::{
        pdo_fetch_type_PDO_FETCH_ASSOC as ASSOC, pdo_fetch_type_PDO_FETCH_BOTH as BOTH,
        pdo_fetch_type_PDO_FETCH_NUM as NUM, pdo_fetch_type_PDO_FETCH_USE_DEFAULT as DEFAULT,
    };
    if call.arity(0, 2).is_err() {
        return;
    }
    let Ok(mut mode) = call.long(1, i64::from(DEFAULT)) else {
        return;
    };
    let Ok(timeout) = call.long(2, 0) else { return };
    let Some(dbh) = method_dbh(call, false) else {
        return;
    };
    if mode == i64::from(DEFAULT) {
        mode = i64::from(dbh.default_fetch_type());
    }
    let numeric = mode == i64::from(NUM) || mode == i64::from(BOTH);
    let named = mode == i64::from(ASSOC) || mode == i64::from(BOTH);
    if !numeric && !named {
        call.argument_value_error(
            1,
            c"must be one of PDO::FETCH_BOTH, PDO::FETCH_ASSOC, or PDO::FETCH_NUM",
        );
        return;
    }
    if timeout < 0 {
        call.argument_value_error(2, c"must be greater than or equal to 0");
        return;
    }
    let timeout = c_int::try_from(timeout).unwrap_or_else(|_| {
        swoole_rs_zend::bailout::defer(swoole_rs_zend::report(
            swoole_rs_zend::E_WARNING,
            c"Timeout was shrunk to 2147483647",
        ));
        c_int::MAX
    });
    if swoole_rs_zend::bailout::pending() {
        return;
    }
    let Some(conn) = server(dbh) else { return };
    let mut note = Notification::default();
    let mut received = None;
    let read = contain_with(|| {
        let mut next = || conn.consume_input().then(|| note.take(conn));
        received = next();
        if received == Some(false) && timeout > 0 {
            let mut socket = libc::pollfd {
                fd: crate::PQsocket(conn),
                events: libc::POLLIN | libc::POLLERR | libc::POLLHUP,
                revents: 0,
            };
            // PDO waits for readability, then uses PQconsumeInput to decide success even after a poll error.
            let _readiness =
                swoole_rs_corebase::sys::poll(std::slice::from_mut(&mut socket), timeout);
            received = next();
        }
    });
    if let Err(bailout) = read {
        bailout.defer();
        return;
    }
    if received == Some(true) {
        call.set_owned(note.into_array(numeric, named));
    } else {
        if received.is_none() {
            pgsql_fatal_error(dbh, None);
            swoole_rs_zend::bailout::defer(contain_with(|| pdo_pgsql_handle_method_error(dbh)));
        }
        call.set_bool(false);
    }
}

swoole_rs_zend::exports! {
/// libpq calls this synchronously while receiving server messages.
extern "C" fn pdo_pgsql_notice(dbh: &Dbh, message: Text<'_>) {
    if let Some(callback) = handle(dbh).and_then(|handle| handle.notice_callback.get()) {
        swoole_rs_zend::bailout::defer(contain_with(|| pdo_pgsql_call_notice(callback, message)));
    }
}

}

fn replace_notice(
    handle: &Handle,
    next: Option<NonNull<c_void>>,
) -> Result<(), swoole_rs_zend::bailout::Bailout> {
    // A PHP destructor can replace the callback again while releasing the previous one.
    if let Some(previous) = handle.notice_callback.replace(next) {
        contain_with(|| pdo_pgsql_release_notice(previous))?;
    }
    Ok(())
}

swoole_rs_zend::exports! {
    #[unsafe(no_mangle)]
    extern "C" fn pdo_pgsql_replace_notice_callback(handle: &Handle, next: Option<NonNull<c_void>>) {
        swoole_rs_zend::bailout::defer(replace_notice(handle, next));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_lose_trailing_breaks_and_a_period_after_one() {
        assert_eq!(trim_message(b""), b"");
        assert_eq!(trim_message(b"\n"), b"\n");
        assert_eq!(trim_message(b"boom\n"), b"boom");
        assert_eq!(trim_message(b"boom\r\n"), b"boom");
        assert_eq!(trim_message(b"boom\n."), b"boom");
        assert_eq!(trim_message(b"boom."), b"boom.");
        assert_eq!(trim_message(b"a\n\n"), b"a");
    }

    #[test]
    fn credentials_escape_backslashes_and_quotes() {
        let mut info = Vec::new();
        push_credential(&mut info, "user", br"o'k\x");
        assert_eq!(info, br" user='o\'k\\x'");
    }

    #[test]
    fn connection_strings_follow_each_version() {
        let info = conninfo(b"host=h;dbname=d", Some(b"u'"), Some(b"p"), 30);
        let expected: &[u8] = if PHP_VERSION_ID < 80400 {
            b"host=h dbname=d user='u\\'' password='p' connect_timeout=30"
        } else {
            b"host=h dbname=d connect_timeout=30 user='u\\'' password='p'"
        };
        assert_eq!(info.as_bytes(), expected);
        if PHP_VERSION_ID >= 80400 {
            let named = conninfo(b"user=a password=b", Some(b"u"), Some(b"p"), 5);
            assert_eq!(named.as_bytes(), b"user=a password=b connect_timeout=5");
        }
    }
}
