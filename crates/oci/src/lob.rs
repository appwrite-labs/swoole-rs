//! A LOB stream's operations (`oci_statement.c`'s `oci_blob_*`): reading and
//! writing at the stream's 1-based offset, flushing, seeking and closing the
//! locator. C keeps the stream itself, its state and the references it holds.
use crate::{Env, Error, Live, Lob, Svc, Sword, error::Nothing};
use swoole_rs_corebase::{
    CCast,
    lent::{Count, In, Out},
};

/// `OCI_SUCCESS`, `OCI_NEED_DATA` and `OCI_ONE_PIECE`.
const OCI_SUCCESS: Sword = 0;
const OCI_NEED_DATA: Sword = 99;
const OCI_ONE_PIECE: u8 = 0;
/// `SQLCS_IMPLICIT`: the database character set.
const SQLCS_IMPLICIT: u8 = 1;
/// `PDO_OCI_LOBMAXSIZE`: offsets past it can't be sought to.
const LOB_MAX_SIZE: i64 = 4_294_967_295;
/// What a failed read or write returns, which C's thunk turns into -1.
const FAILED: usize = usize::MAX;

unsafe extern "C" {
    /// OCI's own declaration, with the callback context and function typed as
    /// the nulls they are here: write `count` bytes of `buffer` at `offset`,
    /// leaving the amount written in `amount`.
    safe fn OCILobWrite<'a>(
        svc: Live<Svc>,
        err: Live<Error>,
        lob: Live<Lob>,
        amount: &mut u32,
        offset: u32,
        buffer: In<'a, u8>,
        count: Count<'a, u32>,
        piece: u8,
        context: Option<&'static Nothing>,
        callback: Option<&'static Nothing>,
        charset: u16,
        form: u8,
    ) -> Sword;
    /// OCI's own declaration, as [`OCILobWrite`]'s: read up to `count` bytes at
    /// `offset`, leaving the bytes and characters read in the amounts.
    safe fn OCILobRead2<'a>(
        svc: Live<Svc>,
        err: Live<Error>,
        lob: Live<Lob>,
        bytes: &mut u64,
        chars: &mut u64,
        offset: u64,
        buffer: Out<'a, u8>,
        count: Count<'a, u64>,
        piece: u8,
        context: Option<&'static Nothing>,
        callback: Option<&'static Nothing>,
        charset: u16,
        form: u8,
    ) -> Sword;
    /// OCI's own declarations: flush, close, and the locator's charset form.
    safe fn OCILobFlushBuffer(svc: Live<Svc>, err: Live<Error>, lob: Live<Lob>, flag: u32)
    -> Sword;
    safe fn OCILobClose(svc: Live<Svc>, err: Live<Error>, lob: Live<Lob>) -> Sword;
    safe fn OCILobCharSetForm(
        env: Live<Env>,
        err: Live<Error>,
        lob: Live<Lob>,
        form: &mut u8,
    ) -> Sword;
}

/// `oci_blob_write`: `count` bytes written at `*offset`, which advances by what
/// OCI wrote; that amount, or [`FAILED`].
#[unsafe(no_mangle)]
extern "C" fn swr_oci_lob_write<'a>(
    svc: Live<Svc>,
    err: Live<Error>,
    lob: Live<Lob>,
    offset: &mut u32,
    buffer: In<'a, u8>,
    count: Count<'a, u32>,
) -> usize {
    let mut amount = count.get();
    let status = OCILobWrite(
        svc,
        err,
        lob,
        &mut amount,
        *offset,
        buffer,
        count,
        OCI_ONE_PIECE,
        None,
        None,
        0,
        SQLCS_IMPLICIT,
    );
    if status != OCI_SUCCESS {
        return FAILED;
    }
    *offset = offset.wrapping_add(amount);
    usize::try_from(amount).unwrap_or(FAILED)
}

/// `oci_blob_read`: up to `count` bytes read at `*offset`, which advances by the
/// bytes read, or the characters for a character LOB (`form` nonzero), as C's
/// `ub4` offset did; `*eof` when fewer came back than asked for. The bytes read,
/// or [`FAILED`].
#[unsafe(no_mangle)]
extern "C" fn swr_oci_lob_read<'a>(
    svc: Live<Svc>,
    err: Live<Error>,
    lob: Live<Lob>,
    offset: &mut u32,
    form: u8,
    buffer: Out<'a, u8>,
    count: Count<'a, u64>,
    eof: &mut bool,
) -> usize {
    let asked = count.get();
    let (mut bytes, mut chars) = (asked, 0);
    let status = OCILobRead2(
        svc,
        err,
        lob,
        &mut bytes,
        &mut chars,
        (*offset).into(),
        buffer,
        count,
        OCI_ONE_PIECE,
        None,
        None,
        0,
        form,
    );
    if status != OCI_SUCCESS && status != OCI_NEED_DATA {
        return FAILED;
    }
    let advance = if form == 0 { bytes } else { chars };
    // C added the oraub8 amount to its ub4 offset, which truncates.
    *offset = offset.wrapping_add(advance.c_cast());
    if bytes < asked {
        *eof = true;
    }
    usize::try_from(bytes).unwrap_or(FAILED)
}

/// `oci_blob_flush`.
#[unsafe(no_mangle)]
extern "C" fn swr_oci_lob_flush(svc: Live<Svc>, err: Live<Error>, lob: Live<Lob>) {
    // Flushing is best effort, as in C.
    let _status = OCILobFlushBuffer(svc, err, lob, 0);
}

/// `oci_blob_close`'s locator close.
#[unsafe(no_mangle)]
extern "C" fn swr_oci_lob_close(svc: Live<Svc>, err: Live<Error>, lob: Live<Lob>) {
    // Closing is best effort, as in C.
    let _status = OCILobClose(svc, err, lob);
}

/// `oci_blob_seek`: PHP's 0-based `offset` as the stream's 1-based one in
/// `*position`, or false past the largest LOB.
#[unsafe(no_mangle)]
extern "C" fn swr_oci_lob_seek(offset: i64, position: &mut u32) -> bool {
    if offset >= LOB_MAX_SIZE {
        return false;
    }
    // `(ub4) offset + 1`, as C computed it.
    *position = offset.c_cast::<u32>().wrapping_add(1);
    true
}

/// The locator's charset form, which a stream reads by (0 when OCI gives none).
#[unsafe(no_mangle)]
extern "C" fn swr_oci_lob_form(env: Live<Env>, err: Live<Error>, lob: Live<Lob>) -> u8 {
    let mut form = 0;
    // A failed read leaves the form 0, as C's zeroed state did.
    let _status = OCILobCharSetForm(env, err, lob, &mut form);
    form
}
