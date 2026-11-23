//! The Firebird client API calls Swoole's bundled `pdo_firebird` makes, as
//! `ibase.h` (Firebird 3 and later, which `config.m4` requires) declares them,
//! each argument typed as what the call requires of it. The extension links the
//! system `libfbclient` itself (`--with-swoole-firebird`, through `fb_config`);
//! nothing here links it, so the workspace builds and tests on machines without
//! Firebird.
//!
//! Nothing here makes an argument: only the C caller's, passed through an
//! export's parameters (`crates/firebird`), where its contract (the call's, as
//! `ibase.h` declares it) stands for the constructor. None is `Send` or `Sync`.
#![expect(
    non_camel_case_types,
    reason = "the C type names, as ibase.h spells them"
)]
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

use std::ffi::{CStr, c_char, c_int, c_short, c_uint, c_ushort, c_void};
use std::marker::PhantomData;

/// `ISC_STATUS`: `intptr_t`.
pub type ISC_STATUS = isize;
/// `FB_API_HANDLE`, the type of every `isc_*_handle`.
pub type FB_API_HANDLE = c_uint;
/// `ISC_SCHAR`.
pub type ISC_SCHAR = c_char;

/// The caller's status vector (`ISC_STATUS_ARRAY`): writable for its 20 entries.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Status(*mut ISC_STATUS);

/// A database, transaction, statement or blob handle the caller keeps
/// (`isc_*_handle *`): readable and writable, holding a live handle of the kind
/// the call takes there, or zero where the call makes one.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Handle(*mut FB_API_HANDLE);

/// Bytes the caller handed over: readable for the length passed beside them, or
/// up to their NUL where the call takes no length or it is zero, and not
/// written for `'a`.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Bytes<'a>(*const ISC_SCHAR, PhantomData<&'a [u8]>);

impl Bytes<'static> {
    /// Bytes that live for the program, which any call may read.
    #[must_use]
    pub const fn from_static(bytes: &'static [u8]) -> Self {
        Self(bytes.as_ptr().cast(), PhantomData)
    }
}

impl<'a> Bytes<'a> {
    /// A C string's bytes, up to its NUL, for a call that takes no length.
    #[must_use]
    pub const fn from_c_str(text: &'a CStr) -> Self {
        Self(text.as_ptr(), PhantomData)
    }
}

/// Where the call writes its `T`s: writable for the capacity passed beside it, or for
/// one `T` when none is.
#[repr(transparent)]
pub struct Out<T>(*mut T);

impl<T> Clone for Out<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Out<T> {}

/// An `XSQLDA *` descriptor area: null where the call takes none, else allocated for its own
/// `sqln` entries, whose data and indicator buffers are live for the call.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Sqlda(*mut c_void);

/// An `ISC_QUAD *` blob id, which the call reads (open) or writes (create).
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Quad(*mut c_void);

/// The caller's `ISC_VERSION_CALLBACK`, which takes the [`VersionArg`] passed beside
/// it and each version line, and only touches what that argument lends it.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct VersionCallback(*const c_void);

/// What the caller's [`VersionCallback`] accepts.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct VersionArg(*mut c_void);

/// One database's part of a transaction (`why.cpp`'s `TEB`, the vector
/// `isc_start_multiple` reads): its handle, and its TPB with the TPB's length.
// `repr(C)`: Firebird's transaction existence block, which `isc_start_multiple` reads.
#[repr(C)]
pub struct Teb<'a> {
    pub database: Handle,
    pub tpb_length: c_int,
    pub tpb: Bytes<'a>,
}

const _: () = assert!(
    std::mem::offset_of!(Teb<'static>, tpb) == 2 * size_of::<*const u8>(),
    "why.cpp's TEB: a pointer, an int, a pointer"
);

/// How many [`Teb`]s `isc_start_multiple` reads: the one it is given.
#[repr(i16)]
pub enum TebCount {
    One = 1,
}

// None of these keeps an argument past the call or runs a callback but
// `isc_version`'s, and each reports through its status vector.
unsafe extern "C" {
    pub safe fn isc_attach_database(
        status: Status,
        name_length: c_short,
        name: Bytes,
        db: Handle,
        dpb_length: c_short,
        dpb: Bytes,
    ) -> ISC_STATUS;
    pub safe fn isc_detach_database(status: Status, db: Handle) -> ISC_STATUS;
    pub safe fn isc_dsql_execute(
        status: Status,
        tr: Handle,
        stmt: Handle,
        dialect: c_ushort,
        sqlda: Sqlda,
    ) -> ISC_STATUS;
    pub safe fn isc_dsql_execute2(
        status: Status,
        tr: Handle,
        stmt: Handle,
        dialect: c_ushort,
        in_sqlda: Sqlda,
        out_sqlda: Sqlda,
    ) -> ISC_STATUS;
    pub safe fn isc_dsql_sql_info(
        status: Status,
        stmt: Handle,
        items_length: c_short,
        items: Bytes,
        buffer_length: c_short,
        buffer: Out<ISC_SCHAR>,
    ) -> ISC_STATUS;
    pub safe fn isc_dsql_free_statement(
        status: Status,
        stmt: Handle,
        option: c_ushort,
    ) -> ISC_STATUS;
    /// What the variadic `isc_start_transaction` calls with the vector it builds.
    pub safe fn isc_start_multiple(
        status: Status,
        tr: Handle,
        count: TebCount,
        tebs: &Teb,
    ) -> ISC_STATUS;
    pub safe fn isc_commit_retaining(status: Status, tr: Handle) -> ISC_STATUS;
    pub safe fn isc_commit_transaction(status: Status, tr: Handle) -> ISC_STATUS;
    pub safe fn isc_rollback_transaction(status: Status, tr: Handle) -> ISC_STATUS;
    pub safe fn isc_dsql_allocate_statement(status: Status, db: Handle, stmt: Handle)
    -> ISC_STATUS;
    pub safe fn isc_dsql_prepare(
        status: Status,
        tr: Handle,
        stmt: Handle,
        length: c_ushort,
        statement: Bytes,
        dialect: c_ushort,
        sqlda: Sqlda,
    ) -> ISC_STATUS;
    pub safe fn isc_dsql_fetch(
        status: Status,
        stmt: Handle,
        dialect: c_ushort,
        sqlda: Sqlda,
    ) -> ISC_STATUS;
    pub safe fn isc_open_blob(
        status: Status,
        db: Handle,
        tr: Handle,
        blob: Handle,
        id: Quad,
    ) -> ISC_STATUS;
    pub safe fn isc_blob_info(
        status: Status,
        blob: Handle,
        items_length: c_short,
        items: Bytes,
        buffer_length: c_short,
        buffer: Out<ISC_SCHAR>,
    ) -> ISC_STATUS;
    pub safe fn isc_get_segment(
        status: Status,
        blob: Handle,
        length: Out<c_ushort>,
        buffer_length: c_ushort,
        buffer: Out<ISC_SCHAR>,
    ) -> ISC_STATUS;
    pub safe fn isc_put_segment(
        status: Status,
        blob: Handle,
        length: c_ushort,
        buffer: Bytes,
    ) -> ISC_STATUS;
    pub safe fn isc_create_blob(
        status: Status,
        db: Handle,
        tr: Handle,
        blob: Handle,
        id: Quad,
    ) -> ISC_STATUS;
    pub safe fn isc_close_blob(status: Status, blob: Handle) -> ISC_STATUS;
    pub safe fn isc_dsql_set_cursor_name(
        status: Status,
        stmt: Handle,
        name: Bytes,
        kind: c_ushort,
    ) -> ISC_STATUS;
    pub safe fn fb_ping(status: Status, db: Handle) -> ISC_STATUS;
    /// Hands each version line to `callback` with `arg`.
    pub safe fn isc_version(db: Handle, callback: VersionCallback, arg: VersionArg) -> c_int;
}

/// ibase.h's descriptor layouts and SQL type codes, bindgen'd (`ibase`).
// bindgen's output, and so which lints fire on it, differs per Firebird version,
// so an expectation would go unfulfilled on some: odbc-sys's exception, for the
// same reason.
#[cfg(feature = "ibase")]
pub mod ibase {
    #![allow(
        non_camel_case_types,
        non_upper_case_globals,
        non_snake_case,
        dead_code,
        unsafe_op_in_unsafe_fn,
        clippy::all,
        clippy::pedantic,
        clippy::restriction,
        clippy::nursery,
        reason = "generated by bindgen from Firebird's ibase.h"
    )]
    include!(concat!(env!("OUT_DIR"), "/ibase.rs"));
}
