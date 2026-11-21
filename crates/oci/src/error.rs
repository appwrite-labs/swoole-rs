//! The driver's error records (`oci_driver.c`'s `_oci_error` and
//! `pdo_oci_fetch_error_func`): the last error of a connection and of each
//! statement, which `errorInfo()` reads. The handle and statement hold theirs in a
//! slot C declares as `struct swr_oci_error *einfo`; C's `_oci_error` wrapper calls
//! [`swr_oci_error`] and does what this cannot (a bailout, the handle's bit field).
use crate::{Error, Live, Sword};
use std::{
    cell::Cell,
    ffi::{CStr, CString},
};
use swoole_rs_corebase::sys::Text;
use swoole_rs_zend::pdo::{Dbh, Slot, Stmt};

/// `pdo_oci_error_info` without its file and line, which nothing read: the
/// last error's Oracle code (0 for none) and message.
pub(crate) struct Record {
    code: i32,
    message: Option<CString>,
}

/// Where a handle or statement keeps its last error: null until its first.
pub(crate) type Einfo = Cell<Option<Box<Record>>>;

const OCI_SUCCESS: Sword = 0;
const OCI_SUCCESS_WITH_INFO: Sword = 1;
const OCI_NEED_DATA: Sword = 99;
const OCI_NO_DATA: Sword = 100;
const OCI_ERROR: Sword = -1;
const OCI_INVALID_HANDLE: Sword = -2;
const OCI_STILL_EXECUTING: Sword = -3123;
const OCI_CONTINUE: Sword = -24200;
/// `OCI_HTYPE_ERROR`.
pub(crate) const OCI_HTYPE_ERROR: u32 = 2;
/// `slprintf`'s `tmp_buf[2048]`: a message is cut to 2047 bytes.
const MESSAGE_LIMIT: usize = 2047;

/// What no value has: the SQLSTATE `OCIErrorGet` is never asked for.
pub(crate) enum Nothing {}

#[expect(
    improper_ctypes,
    reason = "C never reads a record: it keeps the pointer in its slot and passes the slot back"
)]
unsafe extern "C" {
    /// `stmt ? &S->einfo : &H->einfo`, which live as long as the handle or
    /// statement does.
    safe fn swr_oci_einfo<'a>(dbh: &'a Dbh, stmt: Option<&'a Stmt>) -> &'a Einfo;
    /// `dbh->is_closed = 1` and the handle's `attached = 0`: the connection is gone.
    safe fn swr_oci_closed(dbh: &Dbh);
    /// OCI's own declaration, with the SQLSTATE it can also return typed as the
    /// null it is here: record `record`'s code and, given a buffer, message (cut
    /// to `size`, NUL included) from the error handle.
    pub(crate) safe fn OCIErrorGet(
        err: Live<Error>,
        record: u32,
        sqlstate: Option<&'static Nothing>,
        code: &mut i32,
        message: Option<&mut [u8; 1024]>,
        size: u32,
        kind: u32,
    ) -> Sword;
}

/// Oracle errors after which the connection is gone: PDO reports `01002` and
/// treats the handle as closed.
fn disconnects(code: i32) -> bool {
    matches!(
        code,
        22 | 378
            | 602
            | 603
            | 604
            | 609
            | 1012
            | 1033
            | 1041
            | 1043
            | 1089
            | 1090
            | 1092
            | 3113
            | 3114
            | 3122
            | 3135
            | 12153
            | 27146
            | 28511
    )
}

/// `_oci_error`: record `status` of the call named `what` as the statement's
/// error, or the handle's without one, and set the matching SQLSTATE. A
/// statement's error is copied to its handle too, so a `query()` that fails in
/// `execute` reports it on the handle (PHP bug #33707). Before the driver's
/// methods are installed (the constructor), anything but `OCI_SUCCESS_WITH_INFO`
/// throws a `PDOException`. Returns the Oracle code, with `*closed` set when the
/// error ends the connection and `*cancelled` for ORA-01013 (the user cancelled
/// the call), which the caller turns into a bailout before anything else of the
/// error is recorded on the handle.
#[unsafe(no_mangle)]
extern "C" fn swr_oci_error(
    err: Live<Error>,
    dbh: &Dbh,
    stmt: Option<&Stmt>,
    what: Text<'_>,
    status: Sword,
    init: bool,
    file: Text<'_>,
    line: i32,
    closed: &mut bool,
    cancelled: &mut bool,
) -> u32 {
    let slot = swr_oci_einfo(dbh, stmt);
    let set_state = |sqlstate: &CStr| match stmt {
        Some(stmt) => stmt.set_error_code(sqlstate),
        None => dbh.set_error_code(sqlstate),
    };
    let what = what.get().to_bytes();
    let at = format!(" ({}:{line})", file.get().to_string_lossy());
    let mut record = Record {
        code: 0,
        message: None,
    };
    let message = |parts: &[&[u8]]| {
        let mut text = parts.concat();
        text.extend_from_slice(at.as_bytes());
        text.truncate(MESSAGE_LIMIT);
        // The parts are C strings and the location has no NUL.
        CString::new(text).ok()
    };
    if init {
        set_state(c"HY000");
        record.message = message(&[what]);
    } else {
        let mut detail = |prefix: &[u8]| {
            let mut buffer = [0u8; 1024];
            buffer[..12].copy_from_slice(b"<<Unknown>>\0");
            // A failed lookup leaves the code 0 and the buffer "<<Unknown>>", as in C.
            let _status = OCIErrorGet(
                err,
                1,
                None,
                &mut record.code,
                Some(&mut buffer),
                1024,
                OCI_HTYPE_ERROR,
            );
            let text = CStr::from_bytes_until_nul(&buffer).map_or(&[][..], CStr::to_bytes);
            message(&[what, b": ", prefix, text])
        };
        let named = |name: &[u8]| message(&[what, b": ", name]);
        record.message = match status {
            OCI_SUCCESS => {
                set_state(c"00000");
                None
            }
            OCI_ERROR => detail(b""),
            OCI_SUCCESS_WITH_INFO => detail(b"OCI_SUCCESS_WITH_INFO: "),
            OCI_NEED_DATA => named(b"OCI_NEED_DATA"),
            OCI_NO_DATA => named(b"OCI_NO_DATA"),
            OCI_INVALID_HANDLE => named(b"OCI_INVALID_HANDLE"),
            OCI_STILL_EXECUTING => named(b"OCI_STILL_EXECUTING"),
            OCI_CONTINUE => named(b"OCI_CONTINUE"),
            _ => None,
        };
        match record.code {
            0 => {}
            1013 => {
                slot.set(Some(Box::new(record)));
                *cancelled = true;
                return 1013;
            }
            12154 => set_state(c"42S02"),
            code if disconnects(code) => {
                *closed = true;
                set_state(c"01002");
            }
            _ => set_state(c"HY000"),
        }
        if let Some(stmt) = stmt {
            let copy = Record {
                code: record.code,
                message: record.message.clone(),
            };
            swr_oci_einfo(dbh, None).set(Some(Box::new(copy)));
            dbh.set_error_code(&error_state(stmt.error_code()));
        }
    }
    let code = record.code;
    if !dbh.has_methods() && status != OCI_SUCCESS_WITH_INFO {
        let sqlstate = match stmt {
            Some(stmt) => stmt.error_code(),
            None => dbh.error_code(),
        };
        let text = record
            .message
            .as_deref()
            .map_or(&b"(null)"[..], CStr::to_bytes);
        let mut thrown =
            format!("SQLSTATE[{}]: ", error_state(sqlstate).to_string_lossy()).into_bytes();
        thrown.extend_from_slice(text);
        // The SQLSTATE and message are C strings.
        if let Ok(thrown) = CString::new(thrown) {
            dbh.throw_exception(code.into(), &thrown);
        }
    }
    slot.set(Some(Box::new(record)));
    code.cast_unsigned()
}

/// Where a Rust caller of [`report`] is, as C's `__FILE__` and `__LINE__` are in
/// the message.
macro_rules! here {
    () => {
        (
            const {
                match ::std::ffi::CStr::from_bytes_with_nul(concat!(file!(), "\0").as_bytes()) {
                    Ok(file) => file,
                    Err(_) => c"",
                }
            },
            line!(),
        )
    };
}
pub(crate) use here;

/// `oci_drv_error(what)` from Rust: [`swr_oci_error`] of the handle's error,
/// with ORA-01013's bailout deferred to the export's native boundary and a lost
/// connection marked closed, as C's `_oci_error` does them.
pub(crate) fn report(
    dbh: &Dbh,
    err: Live<Error>,
    what: &CStr,
    status: Sword,
    (file, line): (&'static CStr, u32),
) -> u32 {
    let (mut closed, mut cancelled) = (false, false);
    let code = swr_oci_error(
        err,
        dbh,
        None,
        what.into(),
        status,
        false,
        file.into(),
        line.cast_signed(),
        &mut closed,
        &mut cancelled,
    );
    if cancelled {
        swoole_rs_zend::bailout::Bailout.defer();
    } else if closed {
        swr_oci_closed(dbh);
    }
    code
}

/// A PDO error code as the C string it holds.
fn error_state(code: swoole_rs_zend::sys::pdo_error_type) -> CString {
    let bytes: Vec<u8> = code
        .iter()
        .map(|&c| c.to_ne_bytes()[0])
        .take_while(|&b| b != 0)
        .collect();
    // `take_while` stopped at the first NUL.
    CString::new(bytes).unwrap_or_default()
}

/// `pdo_oci_fetch_error_func`: the statement's last error if it has a message,
/// else the handle's, as `errorInfo()`'s code and message when it has a code.
#[unsafe(no_mangle)]
extern "C" fn pdo_oci_fetch_error_func(dbh: &Dbh, stmt: Option<&Stmt>, info: &Slot) {
    let own = stmt.map(|stmt| swr_oci_einfo(dbh, Some(stmt)));
    let record = own
        .filter(|slot| peek(slot, |record| record.message.is_some()))
        .unwrap_or_else(|| swr_oci_einfo(dbh, None));
    peek(record, |record| {
        if record.code != 0 {
            let message = record.message.as_deref().map(CStr::to_bytes);
            info.push_error_info(Some(record.code.cast_unsigned()), message);
        }
        false
    });
}

/// `f` of the slot's record, or false when it has none yet.
fn peek(slot: &Einfo, f: impl FnOnce(&Record) -> bool) -> bool {
    let held = slot.take();
    let answer = held.as_deref().is_some_and(f);
    slot.set(held);
    answer
}

/// `pefree(einfo.errmsg)` of a handle's or statement's record, when it closes.
#[unsafe(no_mangle)]
extern "C" fn swr_oci_einfo_free(dbh: &Dbh, stmt: Option<&Stmt>) {
    drop(swr_oci_einfo(dbh, stmt).take());
}
