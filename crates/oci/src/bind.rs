//! The statement's bound parameters (`oci_statement.c`'s `oci_stmt_param_hook`
//! and its dynamic-bind callbacks): each parameter's record, bound by name or
//! position for data at execution, its value prepared before each execution
//! and applied after it.
//!
//! OCI calls the callbacks during `OCIStmtExecute`, which the coroutine hook
//! runs on Swoole's thread pool, so they read and write only the record: what
//! they hand OCI (the input's bytes, whether it is NULL, the output's buffer)
//! is prepared on the PHP thread before the execution, and what OCI wrote is
//! applied to the PHP variable after it. C keeps the callbacks' `sb4` thunks,
//! the hook's `int` wrapper, and the LOB streams.
use crate::{
    Buffer, Env, Error, Handle, Live, Lob, Stmt, Svc, Sword, Text,
    driver::OCIAttrSetLob,
    error::{Nothing, here, stmt_call},
    handle::{swr_oci_env, swr_oci_err, swr_oci_svc},
    statement::{swr_oci_stmt_err, swr_oci_stmt_handle},
};
use std::{cell::Cell, ffi::CStr, marker::PhantomData, ptr};
use swoole_rs_corebase::CCast;
use swoole_rs_zend::{
    Owned, ZStr, ZString,
    pdo::{Dbh, Slot, Stmt as PdoStmt},
    sys::{
        pdo_param_event, pdo_param_event_PDO_PARAM_EVT_ALLOC as EVT_ALLOC,
        pdo_param_event_PDO_PARAM_EVT_EXEC_POST as EVT_EXEC_POST,
        pdo_param_event_PDO_PARAM_EVT_EXEC_PRE as EVT_EXEC_PRE,
        pdo_param_event_PDO_PARAM_EVT_FREE as EVT_FREE, pdo_param_type,
        pdo_param_type_PDO_PARAM_LOB as PARAM_LOB, pdo_param_type_PDO_PARAM_STMT as PARAM_STMT,
        zend_long,
    },
};

/// `SQLT_CHR` and `SQLT_BLOB`, the types parameters are bound as.
const SQLT_CHR: u16 = 1;
const SQLT_BLOB: u16 = 113;
/// `OCI_DATA_AT_EXEC`: the value comes from the dynamic-bind callbacks.
const OCI_DATA_AT_EXEC: u32 = 0x02;
/// `OCI_ONE_PIECE`.
const OCI_ONE_PIECE: u8 = 0;
/// `OCI_DTYPE_LOB` and `OCI_ATTR_LOBEMPTY`.
const OCI_DTYPE_LOB: u32 = 50;
const OCI_ATTR_LOBEMPTY: u32 = 45;
/// The largest string bound without a length before Oracle reads it as a LONG.
const DEFAULT_VALUE_SIZE: i32 = 1332;

/// One bound parameter's record (`pdo_oci_bound_param`), boxed in PDO's
/// `driver_data` from the parameter's allocation to its release, so what OCI
/// was handed stays at its address for every execution meanwhile. During an
/// execution only OCI and the callbacks touch it, on the pool's thread while
/// the PHP thread waits for the coroutine; otherwise only the PHP thread does.
pub(crate) struct Bound {
    /// `OCIBind *`, allocated by OCI with the statement.
    bind: Handle,
    /// Whether it is bound as a LOB (`PDO_PARAM_LOB`).
    lob_param: bool,
    /// What OCI reports through: the value's NULL indicator, its return code
    /// and the output's length.
    indicator: Cell<i16>,
    retcode: Cell<u16>,
    actual_len: Cell<u32>,
    /// A LOB parameter's locator for this execution, or null.
    lob: Cell<Handle>,
    /// Whether OCI wrote the output buffer this execution.
    used_for_output: Cell<bool>,
    /// The input value as a string, None for NULL (the PHP thread's
    /// conversion before the execution).
    input: Option<Box<[u8]>>,
    /// Where an output value goes: `max_value_len` bytes.
    output: Box<[Cell<u8>]>,
}

/// A dynamic-bind callback's address, as `OCIBindDynamic` takes it.
pub(crate) type Callback = unsafe extern "C" fn();

unsafe extern "C" {
    /// `oci_statement.c`'s `sb4` thunks over [`swr_oci_bind_input`] and
    /// [`swr_oci_bind_output`]: declared without their parameters, since only
    /// their addresses are taken here, never a call.
    fn swr_oci_bind_input_cb();
    fn swr_oci_bind_output_cb();
    /// OCI's own declarations, with the value and the null arguments typed as
    /// what they are here. OCI keeps `bind`'s indicator and return code
    /// buffers for the statement's executions: the parameter's [`Bound`],
    /// alive until it is freed.
    safe fn OCIBindByName(
        stmt: Live<Stmt>,
        bind: &mut Handle,
        err: Live<Error>,
        name: Text,
        name_len: i32,
        value: Option<&'static Nothing>,
        value_size: i32,
        kind: u16,
        indicator: Buffer,
        length: Option<&'static Nothing>,
        code: Buffer,
        max_array: u32,
        current: Option<&'static Nothing>,
        mode: u32,
    ) -> Sword;
    safe fn OCIBindByPos(
        stmt: Live<Stmt>,
        bind: &mut Handle,
        err: Live<Error>,
        position: u32,
        value: Option<&'static Nothing>,
        value_size: i32,
        kind: u16,
        indicator: Buffer,
        length: Option<&'static Nothing>,
        code: Buffer,
        max_array: u32,
        current: Option<&'static Nothing>,
        mode: u32,
    ) -> Sword;
    /// OCI's own declaration: the bind's callbacks, each called with its
    /// [`Bound`] during an execution.
    #[expect(
        improper_ctypes,
        reason = "OCI hands the opaque Rust record back to the callbacks only by address"
    )]
    safe fn OCIBindDynamic(
        bind: Handle,
        err: Live<Error>,
        input_context: &Bound,
        input: Callback,
        output_context: &Bound,
        output: Callback,
    ) -> Sword;
    /// OCI's own declarations for a LOB locator: allocate one into `locator`,
    /// mark it empty, and free a temporary LOB it holds.
    safe fn OCIDescriptorAlloc(
        env: Live<Env>,
        locator: &mut Handle,
        kind: u32,
        extra: usize,
        user: Option<&'static Nothing>,
    ) -> Sword;
    safe fn OCILobIsTemporary(
        env: Live<Env>,
        err: Live<Error>,
        lob: Live<Lob>,
        temporary: &mut i32,
    ) -> Sword;
    safe fn OCILobFreeTemporary(svc: Live<Svc>, err: Live<Error>, lob: Live<Lob>) -> Sword;
    #[link_name = "OCIDescriptorFree"]
    safe fn OCIDescriptorFreeLob(descriptor: Live<Lob>, kind: u32) -> Sword;
    /// `S->have_blobs = 1`.
    safe fn swr_oci_set_have_blobs(stmt: &PdoStmt);
    /// The parameter released and left undefined, as C's NULL output did.
    safe fn swr_oci_param_undef(value: &Slot);
    /// A stream over the LOB `lob`, opened read-write, into `value`; nothing
    /// when the stream can't be made.
    safe fn swr_oci_lob_stream(stmt: &PdoStmt, lob: Live<Lob>, value: &Slot);
    /// The LOB `lob` filled from `value`: a stream (read to its end) or a
    /// string, as `php_stream_from_zval_no_verify` fetches it.
    safe fn swr_oci_lob_fill(stmt: &PdoStmt, lob: Live<Lob>, value: &Slot);
}

impl Bound {
    /// This execution's LOB locator.
    fn locator(&self) -> Live<Lob> {
        Live(self.lob.get(), PhantomData)
    }

    /// `OCI_TEMPLOB_CLOSE` and `OCIDescriptorFree` of the LOB locator, if any.
    fn free_lob(&self, dbh: &Dbh) {
        if self.lob.get().is_null() {
            return;
        }
        let (env, svc, err) = (swr_oci_env(dbh), swr_oci_svc(dbh), swr_oci_err(dbh));
        let mut temporary = 0;
        // Best effort, as in C: a failed check leaves it 0.
        let _status = OCILobIsTemporary(env, err, self.locator(), &mut temporary);
        if temporary != 0 {
            // Freeing is best effort, as in C.
            let _status = OCILobFreeTemporary(svc, err, self.locator());
        }
        // Freeing fails only for a null or foreign descriptor.
        let _status = OCIDescriptorFreeLob(self.locator(), OCI_DTYPE_LOB);
        self.lob.set(ptr::null_mut());
    }
}

/// The input callback's work: the LOB locator, NULL, or the input's bytes for
/// OCI to read, in one piece. Its thunk always continues: C's `OCI_ERROR` for a
/// non-NULL value with no bytes can't happen now that the PHP thread decides
/// both before the execution.
#[unsafe(no_mangle)]
extern "C" fn swr_oci_bind_input<'b>(
    bound: &'b Bound,
    buffer: &mut Buffer,
    length: &mut u32,
    piece: &mut u8,
    indicator: &mut Option<&'b Cell<i16>>,
) {
    *indicator = Some(&bound.indicator);
    if !bound.lob.get().is_null() {
        *buffer = Buffer::handle(bound.locator());
        *length = size_of::<Handle>().c_cast();
    } else if let Some(input) = &bound.input {
        *buffer = Buffer::read(input);
        *length = input.len().c_cast();
    } else {
        bound.indicator.set(-1);
        *buffer = Buffer(ptr::null_mut());
        // `(ub4) -1`, as C passed it.
        *length = u32::MAX;
    }
    *piece = OCI_ONE_PIECE;
}

/// The output callback's work: the LOB locator, or the output buffer, for OCI
/// to write in one piece, with the length, indicator and return code it
/// reports through.
#[unsafe(no_mangle)]
extern "C" fn swr_oci_bind_output<'b>(
    bound: &'b Bound,
    buffer: &mut Buffer,
    length: &mut Option<&'b Cell<u32>>,
    piece: &mut u8,
    indicator: &mut Option<&'b Cell<i16>>,
    code: &mut Option<&'b Cell<u16>>,
) {
    if bound.lob_param {
        bound.actual_len.set(size_of::<Handle>().c_cast());
        *buffer = Buffer::handle(bound.locator());
    } else {
        bound.used_for_output.set(true);
        bound.actual_len.set(bound.output.len().c_cast());
        *buffer = Buffer::cells(&bound.output);
    }
    *length = Some(&bound.actual_len);
    *piece = OCI_ONE_PIECE;
    *code = Some(&bound.retcode);
    *indicator = Some(&bound.indicator);
}

swoole_rs_zend::exports! {
/// `oci_stmt_param_hook` for a bound parameter (`is_param`; a bound column
/// needs nothing): its record made and bound when allocated, its value
/// prepared before each execution and applied after it, and its record freed.
/// `data` is the parameter's `driver_data`, `kind` its `PDO_PARAM_TYPE`,
/// `value` its value (through its reference). False where C returned 0: an
/// OCI call that recorded an error, or a statement parameter.
#[expect(clippy::too_many_arguments, reason = "PDO's bound-parameter record, field by field")]
#[unsafe(no_mangle)]
extern "C" fn swr_oci_param_event(stmt: &PdoStmt, dbh: &Dbh, event: pdo_param_event, kind: pdo_param_type,
    max_value_len: zend_long, name: Option<&ZStr>, paramno: zend_long, data: &mut Option<Box<Bound>>,
    value: &Slot) -> bool {
    let lob = kind == PARAM_LOB;
    let err = swr_oci_stmt_err(stmt);
    let call = |what: &CStr, status| stmt_call(dbh, stmt, err, what, status, here!()) == 0;
    match event {
        EVT_FREE => {
            if let Some(bound) = data.take() {
                bound.free_lob(dbh);
            }
            true
        }
        EVT_ALLOC => {
            let bound = data.insert(Box::new(Bound {
                bind: ptr::null_mut(),
                lob_param: lob,
                indicator: Cell::new(0),
                retcode: Cell::new(0),
                actual_len: Cell::new(0),
                lob: Cell::new(ptr::null_mut()),
                used_for_output: Cell::new(false),
                input: None,
                output: Box::default(),
            }));
            if kind == PARAM_STMT {
                return false;
            }
            let (oci_type, size) = if lob {
                (SQLT_BLOB, size_of::<Handle>().c_cast())
            } else if max_value_len == 0 {
                (SQLT_CHR, DEFAULT_VALUE_SIZE)
            } else {
                // `(sb4) max_value_len`, as C truncated it.
                (SQLT_CHR, max_value_len.c_cast())
            };
            let handle = swr_oci_stmt_handle(stmt);
            let (indicator, code) = (Buffer::cell(&bound.indicator), Buffer::cell(&bound.retcode));
            let (what, status) = if let Some(name) = name {
                let status = OCIBindByName(handle, &mut bound.bind, err, Text::of(name.bytes()),
                    name.bytes().len().c_cast(), None, size, oci_type, indicator, None, code, 0, None,
                    OCI_DATA_AT_EXEC);
                (c"OCIBindByName", status)
            } else {
                // `((ub4) param->paramno) + 1`.
                let position = paramno.c_cast::<u32>().wrapping_add(1);
                let status = OCIBindByPos(handle, &mut bound.bind, err, position, None, size, oci_type,
                    indicator, None, code, 0, None, OCI_DATA_AT_EXEC);
                (c"OCIBindByPos", status)
            };
            if !call(what, status) {
                return false;
            }
            let (input, output): (Callback, Callback) = (swr_oci_bind_input_cb, swr_oci_bind_output_cb);
            call(c"OCIBindDynamic", OCIBindDynamic(bound.bind, err, bound, input, bound, output))
        }
        EVT_EXEC_PRE => {
            let Some(bound) = data.as_deref_mut() else {
                return true;
            };
            bound.indicator.set(0);
            bound.used_for_output.set(false);
            if lob {
                let mut locator = ptr::null_mut();
                let status = OCIDescriptorAlloc(swr_oci_env(dbh), &mut locator, OCI_DTYPE_LOB, 0, None);
                bound.lob.set(locator);
                if !call(c"OCIDescriptorAlloc", status) {
                    return false;
                }
                let status = OCIAttrSetLob(bound.locator(), OCI_DTYPE_LOB, &0, 0, OCI_ATTR_LOBEMPTY, err);
                if !call(c"OCIAttrSet", status) {
                    return false;
                }
                swr_oci_set_have_blobs(stmt);
                return true;
            }
            // On the PHP thread: the value as the string the input hands OCI
            // (`convert_to_string`, NULL left as it is), and room for an output.
            bound.input = if value.value().is_null() {
                None
            } else {
                let string = value.value().try_string().unwrap_or_else(|| ZString::new(b""));
                let bytes = Box::from(string.bytes());
                if value.value().str().is_none() {
                    value.set(Owned::string(string));
                }
                Some(bytes)
            };
            let room = usize::try_from(max_value_len).unwrap_or(0);
            bound.output = (0..room).map(|_| Cell::new(0)).collect();
            true
        }
        EVT_EXEC_POST => {
            let Some(bound) = data.as_deref_mut() else {
                return true;
            };
            if bound.used_for_output.get() {
                if bound.indicator.get() == -1 {
                    swr_oci_param_undef(value);
                } else {
                    let length = usize::try_from(bound.actual_len.get()).unwrap_or(usize::MAX);
                    let written: Vec<u8> = bound.output.iter().take(length).map(Cell::get).collect();
                    value.set(Owned::string(ZString::new(&written)));
                }
            } else if lob && !bound.lob.get().is_null() {
                if value.value().is_null() {
                    // A locator bound to a NULL: the query's LOB, as a stream.
                    swr_oci_lob_stream(stmt, bound.locator(), value);
                } else {
                    swr_oci_lob_fill(stmt, bound.locator(), value);
                    bound.free_lob(dbh);
                }
            }
            bound.input = None;
            bound.output = Box::default();
            true
        }
        _ => true,
    }
}
}
