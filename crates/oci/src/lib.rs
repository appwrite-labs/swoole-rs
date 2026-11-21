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
mod driver;
#[cfg(feature = "php")]
mod error;
#[cfg(feature = "php")]
pub mod minit;

use std::{
    ffi::{c_int, c_uchar, c_void},
    marker::PhantomData,
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

/// An OCI handle of kind `K` the driver handed over: null, which OCI rejects
/// without touching memory (or, for a snapshot, takes as none), or live for as
/// long as the value is used. The exports take it in place of the `dvoid *` it
/// has the layout of. Nothing here makes one, and it is neither `Send` nor
/// `Sync`: it reaches the pool only through [`offload`].
#[repr(transparent)]
pub struct Live<K>(Handle, PhantomData<K>);

impl<K> Clone for Live<K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K> Copy for Live<K> {}

/// The kinds: `OCISvcCtx`, `OCIError`, `OCISession`, `OCIServer`, `OCIStmt` and
/// `OCISnapshot`. A statement's binds and defines, the buffers and the driver's
/// callbacks over them, stay live with it.
pub enum Svc {}
pub enum Error {}
pub enum Session {}
pub enum Server {}
pub enum Stmt {}
pub enum Snapshot {}

/// Statement text the driver handed over: readable for the length passed beside it.
/// As with [`Live`], only the exports' parameters make one.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Text(*const c_uchar);

// OCI's own declarations of the calls that take only handles and plain values,
// with each handle typed as the kind it is. They drive that connection alone,
// run no callback of PHP's (a fetch runs only the driver's define callback, which
// points OCI at the column's own buffer), and report through the error handle.
unsafe extern "C" {
    safe fn OCISessionBegin(
        svc: Live<Svc>,
        err: Live<Error>,
        user: Live<Session>,
        credt: Ub4,
        mode: Ub4,
    ) -> Sword;
    safe fn OCIServerDetach(server: Live<Server>, err: Live<Error>, mode: Ub4) -> Sword;
    safe fn OCITransCommit(svc: Live<Svc>, err: Live<Error>, flags: Ub4) -> Sword;
    safe fn OCITransRollback(svc: Live<Svc>, err: Live<Error>, flags: Ub4) -> Sword;
    safe fn OCIPing(svc: Live<Svc>, err: Live<Error>, mode: Ub4) -> Sword;
    safe fn OCIStmtFetch(
        stmt: Live<Stmt>,
        err: Live<Error>,
        rows: Ub4,
        orientation: Ub2,
        mode: Ub4,
    ) -> Sword;
    safe fn OCIStmtFetch2(
        stmt: Live<Stmt>,
        err: Live<Error>,
        rows: Ub4,
        orientation: Ub2,
        offset: Sb4,
        mode: Ub4,
    ) -> Sword;
    /// Reads the text, keeping a copy with the statement.
    safe fn OCIStmtPrepare(
        stmt: Live<Stmt>,
        err: Live<Error>,
        text: Text,
        text_len: Ub4,
        language: Ub4,
        mode: Ub4,
    ) -> Sword;
    /// Runs the statement over its binds and defines.
    safe fn OCIStmtExecute(
        svc: Live<Svc>,
        stmt: Live<Stmt>,
        err: Live<Error>,
        iters: Ub4,
        rowoff: Ub4,
        snap_in: Live<Snapshot>,
        snap_out: Live<Snapshot>,
        mode: Ub4,
    ) -> Sword;
}

/// What no value has: a [`Null`] is always `None`.
enum Nothing {}

/// A pointer argument only null can fill.
type Null = Option<&'static Nothing>;

unsafe extern "C" {
    /// OCI's own declaration, with the optional context, allocators and user memory
    /// typed as the nulls they are here: it writes the new environment (or null) to
    /// `env` and keeps nothing of the caller's.
    safe fn OCIEnvCreate(
        env: &mut Handle,
        mode: Ub4,
        context: Null,
        malloc: Null,
        realloc: Null,
        free: Null,
        extra: usize,
        user_memory: Null,
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

/// `swoole_oci_session_begin`: `OCISessionBegin`, whose whole contract the handles' type carries.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_oci_session_begin(
    svc: Live<Svc>,
    err: Live<Error>,
    user: Live<Session>,
    credt: Ub4,
    mode: Ub4,
) -> Sword {
    offload(|| OCISessionBegin(svc, err, user, credt, mode))
}

/// `swoole_oci_server_detach`: `OCIServerDetach`, whose whole contract the handles' type carries.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_oci_server_detach(
    server: Live<Server>,
    err: Live<Error>,
    mode: Ub4,
) -> Sword {
    offload(|| OCIServerDetach(server, err, mode))
}

/// `swoole_oci_stmt_prepare`: `OCIStmtPrepare`, whose whole contract the arguments'
/// types carry.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_oci_stmt_prepare(
    stmt: Live<Stmt>,
    err: Live<Error>,
    text: Text,
    text_len: Ub4,
    language: Ub4,
    mode: Ub4,
) -> Sword {
    offload(|| OCIStmtPrepare(stmt, err, text, text_len, language, mode))
}

/// `swoole_oci_stmt_execute`: `OCIStmtExecute`, whose whole contract the handles' type
/// carries.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_oci_stmt_execute(
    svc: Live<Svc>,
    stmt: Live<Stmt>,
    err: Live<Error>,
    iters: Ub4,
    rowoff: Ub4,
    snap_in: Live<Snapshot>,
    snap_out: Live<Snapshot>,
    mode: Ub4,
) -> Sword {
    offload(|| OCIStmtExecute(svc, stmt, err, iters, rowoff, snap_in, snap_out, mode))
}

/// `swoole_oci_stmt_fetch`: `OCIStmtFetch`, whose whole contract the handles' type carries.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_oci_stmt_fetch(
    stmt: Live<Stmt>,
    err: Live<Error>,
    rows: Ub4,
    orientation: Ub2,
    mode: Ub4,
) -> Sword {
    offload(|| OCIStmtFetch(stmt, err, rows, orientation, mode))
}

/// `swoole_oci_stmt_fetch2`: `OCIStmtFetch2`, whose whole contract the handles' type
/// carries.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_oci_stmt_fetch2(
    stmt: Live<Stmt>,
    err: Live<Error>,
    rows: Ub4,
    orientation: Ub2,
    offset: Sb4,
    mode: Ub4,
) -> Sword {
    offload(|| OCIStmtFetch2(stmt, err, rows, orientation, offset, mode))
}

/// `swoole_oci_trans_commit`: `OCITransCommit`, whose whole contract the handles' type carries.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_oci_trans_commit(svc: Live<Svc>, err: Live<Error>, flags: Ub4) -> Sword {
    offload(|| OCITransCommit(svc, err, flags))
}

/// `swoole_oci_trans_rollback`: `OCITransRollback`, whose whole contract the handles' type carries.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_oci_trans_rollback(svc: Live<Svc>, err: Live<Error>, flags: Ub4) -> Sword {
    offload(|| OCITransRollback(svc, err, flags))
}

/// `swoole_oci_ping`: `OCIPing`, whose whole contract the handles' type carries.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_oci_ping(svc: Live<Svc>, err: Live<Error>, mode: Ub4) -> Sword {
    offload(|| OCIPing(svc, err, mode))
}

/// `php_swoole_oracle_rinit`: the shared environment, created once per process.
/// Upstream's unlocked check let concurrent first requests each create one and
/// keep the last; here one does, and a failed create (null) is retried by the
/// next request, as upstream's was.
pub extern "C" fn php_swoole_oracle_rinit() {
    shared_env(|| {
        let mut env = ptr::null_mut();
        OCIEnvCreate(
            &mut env,
            SWOOLE_PDO_OCI_INIT_MODE,
            None,
            None,
            None,
            None,
            0,
            None,
        );
        env
    });
}

/// Sets [`swoole_pdo_oci_Env`] from `create` unless one is set: the check, the create
/// and the store under one lock, so concurrent callers make one between them.
fn shared_env(create: impl FnOnce() -> Handle) {
    static CREATING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    if !swoole_pdo_oci_Env.load(Ordering::Acquire).is_null() {
        return;
    }
    // A panic can't leave the guarded state half-written: it is the atomic, set last.
    let _creating = CREATING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if swoole_pdo_oci_Env.load(Ordering::Acquire).is_null() {
        swoole_pdo_oci_Env.store(create(), Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_first_requests_create_one_environment() {
        use std::sync::atomic::AtomicUsize;
        static CREATED: AtomicUsize = AtomicUsize::new(0);
        swoole_pdo_oci_Env.store(ptr::null_mut(), Ordering::Release);
        let barrier = std::sync::Barrier::new(8);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    barrier.wait();
                    shared_env(|| {
                        CREATED.fetch_add(1, Ordering::SeqCst);
                        std::thread::sleep(std::time::Duration::from_millis(5));
                        ptr::without_provenance_mut(0x1000)
                    });
                });
            }
        });
        assert_eq!(CREATED.load(Ordering::SeqCst), 1);
        assert_eq!(swoole_pdo_oci_Env.load(Ordering::Acquire).addr(), 0x1000);
        // A failed create leaves it null, and the next request tries again.
        swoole_pdo_oci_Env.store(ptr::null_mut(), Ordering::Release);
        shared_env(ptr::null_mut);
        shared_env(|| ptr::without_provenance_mut(0x2000));
        assert_eq!(swoole_pdo_oci_Env.load(Ordering::Acquire).addr(), 0x2000);
        swoole_pdo_oci_Env.store(ptr::null_mut(), Ordering::Release);
    }

    #[test]
    fn offload_calls_through_without_host() {
        BLOCKING.store(false, Ordering::Relaxed);
        assert_eq!(offload(|| 7), 7);
        swoole_oracle_set_blocking(true);
        assert_eq!(offload(|| -1), -1);
    }
}
