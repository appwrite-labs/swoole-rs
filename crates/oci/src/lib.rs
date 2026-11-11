//! Swoole's coroutine OCI calls (upstream's `swoole_oracle.cc`, built with
//! `--with-swoole-oracle`): the `OCI*` replacements its bundled `pdo_oci`
//! driver calls. While the runtime hook is on and a coroutine runs, each call
//! runs on Swoole's thread pool and the coroutine waits for it (native
//! `async`, through the hooks' host). RINIT creates the driver's shared
//! environment; with `php`, MINIT ([`minit`]) registers the driver with PDO.
//!
//! The exports keep upstream's names: `native/hooks/php_swoole_oracle.h`
//! declares them and the driver's macros call them. The extension links the
//! Instant Client's `libclntsh` itself (configure); nothing here links it.
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block
    )
)]

#[cfg(feature = "php")]
pub mod minit;

use std::{
    ffi::{c_int, c_uchar, c_void},
    ptr,
    sync::atomic::{AtomicBool, AtomicPtr, Ordering},
};

/// `sword`.
type Sword = c_int;
/// `ub4`.
type Ub4 = u32;
/// `ub2`.
type Ub2 = u16;
/// `sb4`.
type Sb4 = i32;
/// Any OCI handle or descriptor (`OCISvcCtx`, `OCIError`, `OCIStmt`, …),
/// only ever passed by pointer here.
pub type Handle = *mut c_void;

unsafe extern "C" {
    fn OCISessionBegin(svc: Handle, err: Handle, user: Handle, credt: Ub4, mode: Ub4) -> Sword;
    fn OCIServerDetach(server: Handle, err: Handle, mode: Ub4) -> Sword;
    fn OCIStmtPrepare(
        stmt: Handle,
        err: Handle,
        text: *const c_uchar,
        text_len: Ub4,
        language: Ub4,
        mode: Ub4,
    ) -> Sword;
    fn OCIStmtExecute(
        svc: Handle,
        stmt: Handle,
        err: Handle,
        iters: Ub4,
        rowoff: Ub4,
        snap_in: *const c_void,
        snap_out: Handle,
        mode: Ub4,
    ) -> Sword;
    fn OCIStmtFetch(stmt: Handle, err: Handle, rows: Ub4, orientation: Ub2, mode: Ub4) -> Sword;
    fn OCIStmtFetch2(
        stmt: Handle,
        err: Handle,
        rows: Ub4,
        orientation: Ub2,
        offset: Sb4,
        mode: Ub4,
    ) -> Sword;
    fn OCITransCommit(svc: Handle, err: Handle, flags: Ub4) -> Sword;
    fn OCITransRollback(svc: Handle, err: Handle, flags: Ub4) -> Sword;
    fn OCIPing(svc: Handle, err: Handle, mode: Ub4) -> Sword;
    fn OCIEnvCreate(
        env: *mut Handle,
        mode: Ub4,
        context: *const c_void,
        malloc: *const c_void,
        realloc: *const c_void,
        free: *const c_void,
        extra: usize,
        user_memory: *mut *mut c_void,
    ) -> Sword;
}

/// `OCI_DEFAULT | OCI_THREADED | OCI_OBJECT`: the mode every environment is
/// created with, here and in the driver's per-charset `OCIEnvNlsCreate`.
#[unsafe(no_mangle)]
pub static SWOOLE_PDO_OCI_INIT_MODE: Ub4 = 0x1 | 0x2;

/// The environment every `pdo_oci` connection shares, as the driver reads it
/// (an `OCIEnv *`: `AtomicPtr` has a pointer's layout).
#[unsafe(no_mangle)]
pub static swoole_pdo_oci_Env: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

static BLOCKING: AtomicBool = AtomicBool::new(true);

/// `swoole_oracle_set_blocking`: the runtime hook flag.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_oracle_set_blocking(blocking: bool) {
    BLOCKING.store(blocking, Ordering::Relaxed);
}

/// `php_swoole_async(swoole_oracle_blocking, call)`: the call's result, or 0
/// (upstream's untouched `result`) when the pool did not run it.
fn offload(call: impl FnMut() -> Sword) -> Sword {
    let pool = swoole_rs_hooks::stdio::offload().filter(|_| !BLOCKING.load(Ordering::Relaxed));
    // SAFETY: native `async` inside a coroutine, or none; the call waits for the job.
    unsafe { swoole_rs_hooks::stdio::offloaded(pool, 0, call) }
}

/// `swoole_oci_session_begin`.
/// # Safety
/// As `OCISessionBegin`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_oci_session_begin(
    svc: Handle,
    err: Handle,
    user: Handle,
    credt: Ub4,
    mode: Ub4,
) -> Sword {
    // SAFETY: per the caller.
    offload(|| unsafe { OCISessionBegin(svc, err, user, credt, mode) })
}

/// `swoole_oci_server_detach`.
/// # Safety
/// As `OCIServerDetach`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_oci_server_detach(server: Handle, err: Handle, mode: Ub4) -> Sword {
    // SAFETY: per the caller.
    offload(|| unsafe { OCIServerDetach(server, err, mode) })
}

/// `swoole_oci_stmt_prepare`.
/// # Safety
/// As `OCIStmtPrepare`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_oci_stmt_prepare(
    stmt: Handle,
    err: Handle,
    text: *const c_uchar,
    text_len: Ub4,
    language: Ub4,
    mode: Ub4,
) -> Sword {
    // SAFETY: per the caller.
    offload(|| unsafe { OCIStmtPrepare(stmt, err, text, text_len, language, mode) })
}

/// `swoole_oci_stmt_execute`.
/// # Safety
/// As `OCIStmtExecute`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_oci_stmt_execute(
    svc: Handle,
    stmt: Handle,
    err: Handle,
    iters: Ub4,
    rowoff: Ub4,
    snap_in: *const c_void,
    snap_out: Handle,
    mode: Ub4,
) -> Sword {
    // SAFETY: per the caller.
    offload(|| unsafe { OCIStmtExecute(svc, stmt, err, iters, rowoff, snap_in, snap_out, mode) })
}

/// `swoole_oci_stmt_fetch`.
/// # Safety
/// As `OCIStmtFetch`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_oci_stmt_fetch(
    stmt: Handle,
    err: Handle,
    rows: Ub4,
    orientation: Ub2,
    mode: Ub4,
) -> Sword {
    // SAFETY: per the caller.
    offload(|| unsafe { OCIStmtFetch(stmt, err, rows, orientation, mode) })
}

/// `swoole_oci_stmt_fetch2`.
/// # Safety
/// As `OCIStmtFetch2`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_oci_stmt_fetch2(
    stmt: Handle,
    err: Handle,
    rows: Ub4,
    orientation: Ub2,
    offset: Sb4,
    mode: Ub4,
) -> Sword {
    // SAFETY: per the caller.
    offload(|| unsafe { OCIStmtFetch2(stmt, err, rows, orientation, offset, mode) })
}

/// `swoole_oci_trans_commit`.
/// # Safety
/// As `OCITransCommit`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_oci_trans_commit(svc: Handle, err: Handle, flags: Ub4) -> Sword {
    // SAFETY: per the caller.
    offload(|| unsafe { OCITransCommit(svc, err, flags) })
}

/// `swoole_oci_trans_rollback`.
/// # Safety
/// As `OCITransRollback`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_oci_trans_rollback(svc: Handle, err: Handle, flags: Ub4) -> Sword {
    // SAFETY: per the caller.
    offload(|| unsafe { OCITransRollback(svc, err, flags) })
}

/// `swoole_oci_ping`.
/// # Safety
/// As `OCIPing`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_oci_ping(svc: Handle, err: Handle, mode: Ub4) -> Sword {
    // SAFETY: per the caller.
    offload(|| unsafe { OCIPing(svc, err, mode) })
}

/// `php_swoole_oracle_rinit`: the shared environment, once per process.
/// # Safety
/// Called at RINIT, on the request thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn php_swoole_oracle_rinit() {
    if swoole_pdo_oci_Env.load(Ordering::Relaxed).is_null() {
        let null = ptr::null();
        // SAFETY: OCI fills the environment, which RINIT alone writes, on the
        // request thread; no custom allocators or user memory.
        unsafe {
            OCIEnvCreate(
                swoole_pdo_oci_Env.as_ptr(),
                SWOOLE_PDO_OCI_INIT_MODE,
                null,
                null,
                null,
                null,
                0,
                ptr::null_mut(),
            )
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offload_calls_through_without_host() {
        BLOCKING.store(false, Ordering::Relaxed);
        assert_eq!(offload(|| 7), 7);
        swoole_oracle_set_blocking(true);
        assert_eq!(offload(|| -1), -1);
    }
}
