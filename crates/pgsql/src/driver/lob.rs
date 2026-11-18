//! Zend streams keep the PDO connection alive until their libpq descriptor closes.
use super::{Handle, handle};
use crate::Conn;
use pq_sys::Oid;
use std::{ffi::c_int, ptr::NonNull};
use swoole_rs_corebase::{
    CellExt,
    lent::{Count, In, Out, Wrote},
};
use swoole_rs_zend::{Owned, Value, bailout, contain_with, pdo::Dbh, sys::zend_long};

pub(crate) struct LargeObject {
    database: Owned,
    conn: Conn,
    fd: c_int,
    pub(crate) oid: Oid,
}

unsafe extern "C" {
    safe fn pdo_pgsql_lob_dbh(database: &Value) -> &Dbh;
}

#[expect(
    clippy::unnecessary_box_returns,
    reason = "the Zend stream takes this owner until its close callback"
)]
#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_lob_owner(
    handle: &Handle,
    database: &Value,
    fd: c_int,
    oid: Oid,
) -> Box<LargeObject> {
    Box::new(LargeObject {
        database: database.copy(),
        conn: handle.server,
        fd,
        oid,
    })
}

#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_lob_write<'a>(
    lob: &LargeObject,
    buf: In<'a, u8>,
    count: Count<'a>,
) -> isize {
    unsafe extern "C" {
        safe fn lo_write<'a>(conn: Conn, fd: c_int, buf: In<'a, u8>, count: Count<'a>) -> c_int;
    }
    lo_write(lob.conn, lob.fd, buf, count) as isize
}

#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_lob_read<'a>(
    lob: &LargeObject,
    buf: Out<'a, u8>,
    count: Count<'a>,
) -> Wrote<'a, c_int> {
    unsafe extern "C" {
        safe fn lo_read<'a>(
            conn: Conn,
            fd: c_int,
            buf: Out<'a, u8>,
            count: Count<'a>,
        ) -> Wrote<'a, c_int>;
    }
    lo_read(lob.conn, lob.fd, buf, count)
}

#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_lob_seek(
    lob: &LargeObject,
    offset: i64,
    whence: c_int,
    position: &mut i64,
) -> bool {
    unsafe extern "C" {
        safe fn pdo_pgsql_seek_descriptor(conn: Conn, fd: c_int, offset: i64, whence: c_int)
        -> i64;
    }
    *position = pdo_pgsql_seek_descriptor(lob.conn, lob.fd, offset, whence);
    *position >= 0
}

swoole_rs_zend::exports! {
    preserve_pending:
    #[unsafe(no_mangle)]
    extern "C" fn pdo_pgsql_lob_close(lob: Option<Box<LargeObject>>, close: bool, id: zend_long) {
        unsafe extern "C" {
            safe fn lo_close(conn: Conn, fd: c_int) -> c_int;
        }
        let Some(lob) = lob else { return };
        if close {
            bailout::defer(contain_with(|| { lo_close(lob.conn, lob.fd); }));
        }
        if let Some(handle) = handle(pdo_pgsql_lob_dbh(&lob.database)) {
            handle.lob_streams.with_taken(|streams| {
                if let Ok(index) = streams.binary_search_by_key(&id, |&(key, _)| key) {
                    streams.remove(index);
                }
            });
        }
        drop(lob);
    }
}

#[unsafe(no_mangle)]
extern "C" fn pdo_pgsql_register_lob(
    handle: &Handle,
    id: zend_long,
    resource: NonNull<std::ffi::c_void>,
) {
    handle
        .lob_streams
        .with_taken(|streams| streams.push((id, resource)));
}
