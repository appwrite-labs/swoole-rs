//! `pdo_firebird`'s error mapping (`firebird_driver.c`'s
//! `php_firebird_set_error`), under its C name: the connection's status
//! vector, or the caller's message and SQLSTATE, recorded on the connection
//! and on the statement or connection's error code. The record stays C: the
//! vector is read and the message kept through the driver's `swr_firebird_*`
//! accessors.
use std::ffi::{CString, c_char, c_int, c_uint};
use std::marker::PhantomData;
use swoole_rs_corebase::lent::{Count, In, Out, lend_out};
use swoole_rs_zend::pdo::{Dbh, Stmt};

/// `ISC_STATUS_ARRAY`: a status vector's 20 entries.
type Vector = [isize; 20];

/// `fb_interpret`'s `const ISC_STATUS **`: where the next error of a vector
/// starts, which each call moves past the error it printed.
#[repr(transparent)]
struct Cursor<'a>(*const isize, PhantomData<&'a Vector>);

unsafe extern "C" {
    /// The connection's status vector (`H->isc_status`), as the last call left it.
    safe fn swr_firebird_status_vector(dbh: &Dbh) -> &Vector;
    /// The connection's error record (`H->einfo`): `sqlcode`, and the lent
    /// bytes as its message when `has_message`, else none; the old one freed.
    safe fn swr_firebird_set_einfo<'a>(
        dbh: &Dbh,
        sqlcode: c_int,
        message: In<'a, u8>,
        len: Count<'a>,
        has_message: bool,
    );
    /// The vector's SQL code.
    safe fn isc_sqlcode(vector: &Vector) -> i32;
    /// The error at `vector`'s text, NUL-terminated, into the lent bytes: its
    /// length, 0 at the vector's end; `vector` moved to the next.
    safe fn fb_interpret<'a>(
        buffer: Out<'a, c_char>,
        len: Count<'a, c_uint>,
        vector: &mut Cursor<'_>,
    ) -> i32;
    /// The vector's SQLSTATE, NUL-terminated, into the 6 bytes.
    safe fn fb_sqlstate(state: &mut [c_char; 6], vector: &Vector);
}

/// `php_firebird_set_error`'s 512-byte message buffer.
const MESSAGE_MAX: usize = 512;
/// The SQL code of an error with no status: `-999`.
const NO_SQLCODE: c_int = -999;

/// The vector's errors' texts, each followed by a space but the last, in at
/// most 511 bytes, as C joined them.
fn interpret(vector: &Vector) -> Vec<u8> {
    let mut buffer = [0u8; MESSAGE_MAX];
    let mut cursor = Cursor(vector.as_ptr(), PhantomData);
    let mut read = 0;
    while MESSAGE_MAX > read + 1 {
        let Some(room) = buffer.get_mut(read..MESSAGE_MAX - 1) else {
            break;
        };
        let printed = lend_out(room, |out: Out<'_, u8>, len| {
            fb_interpret(out.chars(), len, &mut cursor)
        });
        let Ok(printed @ 1..) = usize::try_from(printed) else {
            break;
        };
        read += printed;
        if let Some(space) = buffer.get_mut(read) {
            *space = b' ';
        }
        read += 1;
    }
    // Without the last space.
    buffer
        .get(..read.saturating_sub(1))
        .unwrap_or_default()
        .to_vec()
}

/// `php_firebird_set_error`: the connection's error recorded from its status
/// vector when the last call failed, its SQLSTATE the statement's (with
/// `stmt`) or the connection's error code; else the lent `message`, and
/// `state` or `HY000`. Before the connection is made (no methods yet), a
/// `PDOException` of the message.
#[unsafe(no_mangle)]
pub extern "C" fn php_firebird_set_error<'s, 'm>(
    dbh: &Dbh,
    stmt: Option<&Stmt>,
    state: In<'s, u8>,
    state_len: Count<'s>,
    message: In<'m, u8>,
    message_len: Count<'m>,
) {
    let set_code = |code: &std::ffi::CStr| match stmt {
        Some(stmt) => stmt.set_error_code(code),
        None => dbh.set_error_code(code),
    };
    let vector = swr_firebird_status_vector(dbh);
    let failed = vector.first() == Some(&1) && vector.get(1).is_some_and(|&code| code > 0);
    let (sqlcode, recorded) = if failed {
        let sqlcode = isc_sqlcode(vector);
        let text = interpret(vector);
        let mut sqlstate = [0; 6];
        fb_sqlstate(&mut sqlstate, vector);
        let bytes = sqlstate.map(|c| c.to_ne_bytes()[0]);
        let code = std::ffi::CStr::from_bytes_until_nul(&bytes).unwrap_or(c"HY000");
        set_code(code);
        (sqlcode, Some(text))
    } else {
        let message = message.drain(message_len).bytes();
        let state = state.drain(state_len).bytes();
        // C's `state && state_len && state_len < sizeof(pdo_error_type)`, copied
        // as a C string.
        let code = (1..6).contains(&state.len()).then(|| {
            CString::new(state.split(|&b| b == 0).next().unwrap_or_default()).unwrap_or_default()
        });
        set_code(code.as_deref().unwrap_or(c"HY000"));
        (NO_SQLCODE, (!message.is_empty()).then(|| message.to_vec()))
    };
    let held = recorded.as_deref().unwrap_or_default();
    swoole_rs_corebase::lent::lend_in(held, |bytes, len| {
        swr_firebird_set_einfo(dbh, sqlcode, bytes, len, recorded.is_some());
    });
    if !dbh.has_methods() {
        // pdo_throw_exception of the message, a C string to its first NUL.
        let text =
            CString::new(held.split(|&b| b == 0).next().unwrap_or_default()).unwrap_or_default();
        dbh.throw(0, &text);
    }
}
