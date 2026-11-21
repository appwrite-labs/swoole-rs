//! The connection's record (`php_pdo_oci_int.h`'s `pdo_oci_db_handle`), kept in
//! `dbh->driver_data`, and `pdo_oci_handle_factory`'s work, which fills it. The
//! C statement code reads it through the accessors here.
use crate::{
    Env, Error, Live, Server, Session, Svc, Sword, Text,
    driver::{
        OCI_HTYPE_SERVER, OCI_HTYPE_SESSION, OCI_HTYPE_SVCCTX, OCIAttrSetHandle, OCIAttrSetSession,
        OCIHandleAlloc, PREFETCH_DEFAULT,
    },
    error::{Einfo, OCI_HTYPE_ERROR, here, report, report_init},
    swoole_oci_session_begin, swoole_pdo_oci_Env,
};
use std::{cell::Cell, marker::PhantomData, ptr, sync::atomic::Ordering};
use swoole_rs_corebase::{CCast, sys::Text as CText};
use swoole_rs_zend::pdo::Dbh;

/// `OCI_SUCCESS` and `OCI_SUCCESS_WITH_INFO`.
const OCI_SUCCESS: Sword = 0;
const OCI_SUCCESS_WITH_INFO: Sword = 1;
/// `OCI_ATTR_SERVER`, `OCI_ATTR_SESSION`, `OCI_ATTR_USERNAME`, `OCI_ATTR_PASSWORD`.
const OCI_ATTR_SERVER: u32 = 6;
const OCI_ATTR_SESSION: u32 = 7;
const OCI_ATTR_USERNAME: u32 = 22;
const OCI_ATTR_PASSWORD: u32 = 23;
/// `OCI_CRED_RDBMS`: a user name and password.
const OCI_CRED_RDBMS: u32 = 1;
/// `OCI_NLS_CHARSET_MAXBYTESZ`.
const OCI_NLS_CHARSET_MAXBYTESZ: u16 = 91;

/// `pdo_oci_db_handle`: the connection's OCI handles (null until the factory
/// makes them), its environment's charset (0 for the shared environment), the
/// prefetch rows, whether the server is attached, the charset's widest
/// character, and the last error.
pub(crate) struct Handle {
    env: Cell<Live<Env>>,
    err: Cell<Live<Error>>,
    server: Cell<Live<Server>>,
    svc: Cell<Live<Svc>>,
    session: Cell<Live<Session>>,
    charset: Cell<u16>,
    prefetch: Cell<u32>,
    attached: Cell<bool>,
    max_char_width: Cell<u32>,
    pub(crate) einfo: Einfo,
}

impl Handle {
    fn new() -> Self {
        Self {
            env: Cell::new(Live(ptr::null_mut(), PhantomData)),
            err: Cell::new(Live(ptr::null_mut(), PhantomData)),
            server: Cell::new(Live(ptr::null_mut(), PhantomData)),
            svc: Cell::new(Live(ptr::null_mut(), PhantomData)),
            session: Cell::new(Live(ptr::null_mut(), PhantomData)),
            charset: Cell::new(0),
            prefetch: Cell::new(PREFETCH_DEFAULT),
            attached: Cell::new(false),
            max_char_width: Cell::new(0),
            einfo: Cell::new(None),
        }
    }
}

#[expect(
    improper_ctypes,
    reason = "C never reads the record: it keeps the pointer and passes it back to the accessors"
)]
unsafe extern "C" {
    /// The record the factory set as the driver data, or none.
    safe fn swr_zend_pdo_driver_data(dbh: &Dbh) -> Option<&Handle>;
    /// The driver data, which the handle gives up: `dbh->driver_data = NULL`.
    safe fn swr_oci_take_handle(dbh: &Dbh) -> Option<Box<Handle>>;
    /// OCI's own declaration: the id of a charset name, 0 for an unknown one.
    safe fn OCINlsCharSetNameToId(env: Live<Env>, name: CText<'_>) -> u16;
    /// OCI's own declaration, with the context, allocators and user memory
    /// typed as the nulls they are here: a new environment for `charset`.
    safe fn OCIEnvNlsCreate(
        env: &mut crate::Handle,
        mode: u32,
        context: Option<&'static crate::error::Nothing>,
        malloc: Option<&'static crate::error::Nothing>,
        realloc: Option<&'static crate::error::Nothing>,
        free: Option<&'static crate::error::Nothing>,
        extra: usize,
        user_memory: Option<&'static crate::error::Nothing>,
        charset: u16,
        ncharset: u16,
    ) -> Sword;
    /// OCI's own declaration: attach the server handle to `dbname`.
    safe fn OCIServerAttach(
        server: Live<Server>,
        err: Live<Error>,
        dbname: CText<'_>,
        length: i32,
        mode: u32,
    ) -> Sword;
    /// OCI's own declaration: one of the environment charset's numbers.
    safe fn OCINlsNumericInfoGet(
        env: Live<Env>,
        err: Live<Error>,
        value: &mut i32,
        item: u16,
    ) -> Sword;
}

/// The record of a connection the factory has started.
pub(crate) fn handle(dbh: &Dbh) -> Option<&Handle> {
    swr_zend_pdo_driver_data(dbh)
}

/// The null handle of a kind, for a record that has none.
fn none<K>() -> Live<K> {
    Live(ptr::null_mut(), PhantomData)
}

/// `dbh`'s record's field, or `default` before the factory made the record.
fn read<T>(dbh: &Dbh, default: T, field: impl FnOnce(&Handle) -> T) -> T {
    handle(dbh).map_or(default, field)
}

/// The accessors the driver's Rust and the C statement code read the record
/// through.
#[unsafe(no_mangle)]
pub(crate) extern "C" fn swr_oci_svc(dbh: &Dbh) -> Live<Svc> {
    read(dbh, none(), |h| h.svc.get())
}

#[unsafe(no_mangle)]
pub(crate) extern "C" fn swr_oci_err(dbh: &Dbh) -> Live<Error> {
    read(dbh, none(), |h| h.err.get())
}

#[unsafe(no_mangle)]
pub(crate) extern "C" fn swr_oci_env(dbh: &Dbh) -> Live<Env> {
    read(dbh, none(), |h| h.env.get())
}

#[unsafe(no_mangle)]
pub(crate) extern "C" fn swr_oci_session(dbh: &Dbh) -> Live<Session> {
    read(dbh, none(), |h| h.session.get())
}

#[unsafe(no_mangle)]
pub(crate) extern "C" fn swr_oci_server(dbh: &Dbh) -> Live<Server> {
    read(dbh, none(), |h| h.server.get())
}

#[unsafe(no_mangle)]
pub(crate) extern "C" fn swr_oci_attached(dbh: &Dbh) -> bool {
    read(dbh, false, |h| h.attached.get())
}

#[unsafe(no_mangle)]
pub(crate) extern "C" fn swr_oci_charset(dbh: &Dbh) -> u16 {
    read(dbh, 0, |h| h.charset.get())
}

#[unsafe(no_mangle)]
pub(crate) extern "C" fn swr_oci_prefetch(dbh: &Dbh) -> u32 {
    read(dbh, PREFETCH_DEFAULT, |h| h.prefetch.get())
}

#[unsafe(no_mangle)]
pub(crate) extern "C" fn swr_oci_set_prefetch(dbh: &Dbh, rows: u32) {
    if let Some(h) = handle(dbh) {
        h.prefetch.set(rows);
    }
}

/// `H->max_char_width`: the widest character of the connection's charset.
#[unsafe(no_mangle)]
pub(crate) extern "C" fn swr_oci_max_char_width(dbh: &Dbh) -> u32 {
    read(dbh, 0, |h| h.max_char_width.get())
}

/// The connection is gone: `dbh->is_closed` and the record's `attached` off.
#[unsafe(no_mangle)]
pub(crate) extern "C" fn swr_oci_closed(dbh: &Dbh) {
    dbh.set_closed();
    if let Some(h) = handle(dbh) {
        h.attached.set(false);
    }
}

/// The record given back when the handle closes, after its OCI handles are
/// freed ([`crate::driver::swr_oci_close`]).
pub(crate) fn release(dbh: &Dbh) {
    drop(swr_oci_take_handle(dbh));
}

swoole_rs_zend::exports! {
/// `pdo_oci_handle_factory` from the data source C parsed: a new record in
/// `dbh->driver_data`, its environment (its own for `charset`, else the
/// shared one), the server attached to `dbname`, a service context and a
/// session begun as `user`/`password`. False, with the error recorded (and,
/// before the methods are installed, thrown), when a step fails; the closer
/// frees what was made.
#[unsafe(no_mangle)]
extern "C" fn swr_oci_connect(
    dbh: &Dbh,
    charset: Option<CText<'_>>,
    dbname: CText<'_>,
    user: Option<CText<'_>>,
    password: Option<CText<'_>>,
) -> bool {
    dbh.set_driver_data(Box::into_raw(Box::new(Handle::new())).cast());
    let Some(h) = handle(dbh) else {
        return false;
    };
    if let Some(charset) = charset {
        let shared = Live(swoole_pdo_oci_Env.load(Ordering::Relaxed), PhantomData);
        let id = OCINlsCharSetNameToId(shared, charset);
        h.charset.set(id);
        if id == 0 {
            report_init(dbh, c"OCINlsCharSetNameToId: unknown character set name", here!());
            return false;
        }
        let mut env = ptr::null_mut();
        let mode = crate::SWOOLE_PDO_OCI_INIT_MODE;
        let status = OCIEnvNlsCreate(&mut env, mode, None, None, None, None, 0, None, id, id);
        // C created it straight into the record, so the closer frees what a
        // failure left there.
        h.env.set(Live(env, PhantomData));
        if status != OCI_SUCCESS {
            report_init(
                dbh,
                c"OCIEnvNlsCreate: Check the character set is valid and that PHP has access to Oracle libraries and NLS data",
                here!(),
            );
            return false;
        }
    }
    if h.env.get().is_null() {
        h.env.set(Live(swoole_pdo_oci_Env.load(Ordering::Relaxed), PhantomData));
    }
    let env = h.env.get();
    let alloc = |kind| {
        let mut handle = ptr::null_mut();
        let status = OCIHandleAlloc(env, &mut handle, kind, 0, None);
        (status, handle)
    };
    // As in C, the error and server handles' allocations aren't checked: a
    // null one fails the attach below.
    h.err.set(Live(alloc(OCI_HTYPE_ERROR).1, PhantomData));
    h.server.set(Live(alloc(OCI_HTYPE_SERVER).1, PhantomData));
    let err = h.err.get();
    let length = dbname.get().to_bytes().len().c_cast();
    let status = OCIServerAttach(h.server.get(), err, dbname, length, 0);
    if status != OCI_SUCCESS {
        report(dbh, err, c"pdo_oci_handle_factory", status, here!());
        return false;
    }
    h.attached.set(true);
    let (status, svc) = alloc(OCI_HTYPE_SVCCTX);
    if status != OCI_SUCCESS {
        report(dbh, err, c"OCIHandleAlloc: OCI_HTYPE_SVCCTX", status, here!());
        return false;
    }
    h.svc.set(Live(svc, PhantomData));
    let (status, session) = alloc(OCI_HTYPE_SESSION);
    if status != OCI_SUCCESS {
        report(dbh, err, c"OCIHandleAlloc: OCI_HTYPE_SESSION", status, here!());
        return false;
    }
    h.session.set(Live(session, PhantomData));
    let (svc, session) = (h.svc.get(), h.session.get());
    let status = OCIAttrSetHandle(svc, OCI_HTYPE_SVCCTX, h.server.get().any(), 0, OCI_ATTR_SERVER, err);
    if status != OCI_SUCCESS {
        report(dbh, err, c"OCIAttrSet: OCI_ATTR_SERVER", status, here!());
        return false;
    }
    for (value, attribute, what) in [
        (user, OCI_ATTR_USERNAME, c"OCIAttrSet: OCI_ATTR_USERNAME"),
        (password, OCI_ATTR_PASSWORD, c"OCIAttrSet: OCI_ATTR_PASSWORD"),
    ] {
        let Some(value) = value else {
            continue;
        };
        let bytes = value.get().to_bytes();
        let status = OCIAttrSetSession(
            session,
            OCI_HTYPE_SESSION,
            Text::of(bytes),
            bytes.len().c_cast(),
            attribute,
            err,
        );
        if status != OCI_SUCCESS {
            report(dbh, err, what, status, here!());
            return false;
        }
    }
    let status = swoole_oci_session_begin(svc, err, session, OCI_CRED_RDBMS, 0);
    if status != OCI_SUCCESS {
        report(dbh, err, c"OCISessionBegin", status, here!());
        // An expired password still opens the session, with this warning.
        if status != OCI_SUCCESS_WITH_INFO {
            return false;
        }
    }
    let status = OCIAttrSetHandle(svc, OCI_HTYPE_SVCCTX, session.any(), 0, OCI_ATTR_SESSION, err);
    if status != OCI_SUCCESS {
        report(dbh, err, c"OCIAttrSet: OCI_ATTR_SESSION", status, here!());
        return false;
    }
    let mut width = 0;
    let status = OCINlsNumericInfoGet(env, err, &mut width, OCI_NLS_CHARSET_MAXBYTESZ);
    if status != OCI_SUCCESS {
        report(dbh, err, c"OCINlsNumericInfoGet: OCI_NLS_CHARSET_MAXBYTESZ", status, here!());
        return false;
    }
    // A charset's widest character is a positive byte count.
    h.max_char_width.set(width.cast_unsigned());
    true
}
}
