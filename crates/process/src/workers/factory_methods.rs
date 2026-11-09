//! The context [`super::factory`]'s operations run over. crates/server registers its
//! factory call once, and runs each `Factory`, `ProcessFactory` and `Server` method
//! that was one of those operations through [`run`].
use core::ffi::{c_int, c_void};
use core::ptr;
use std::sync::OnceLock;

use super::{WorkersCall, workers_run};
use swoole_rs_corebase::DataHead;

/// The context crates/server's factory call answers over.
#[repr(C)]
pub struct FactoryContext {
    serv: *mut c_void,
    worker: *mut c_void,
    send: *mut SendData,
    status: *const c_void,
    conn: *mut c_void,
    fd: c_int,
}

const _: () = assert!(
    size_of::<FactoryContext>() == 48 && core::mem::offset_of!(FactoryContext, fd) == 40,
    "crates/server's factory_ops.rs Context layout"
);

/// `swoole::SendData`.
#[repr(C)]
pub struct SendData {
    pub info: DataHead,
    pub data: *const c_void,
}

static CALL: OnceLock<WorkersCall> = OnceLock::new();

/// Registers crates/server's factory call, which the methods below run over.
pub fn set_factory_call(call: WorkersCall) {
    // A second registration is the same function; keeping the first is equivalent.
    CALL.get_or_init(|| call);
}

/// Runs factory operation `op` (a `RUN_*` of [`super::ops`]) over a context of `serv`,
/// the worker, send and exit status it reads, and `arg`.
///
/// # Safety
/// `serv` is a live `Server`; the others are what `op` reads, live for the call.
pub unsafe fn run(
    serv: *mut c_void,
    op: u32,
    worker: *mut c_void,
    send: *mut SendData,
    status: *const c_void,
    arg: u64,
) -> i64 {
    let Some(&call) = CALL.get() else {
        // crates/server registers when a Server opens.
        std::process::abort()
    };
    let mut ctx = FactoryContext {
        serv,
        worker,
        send,
        status,
        conn: ptr::null_mut(),
        fd: 0,
    };
    // SAFETY: as the caller guarantees; the context lives for the call.
    unsafe { workers_run(op, arg, (&raw mut ctx).cast(), call) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// The context each native op saw: server, worker, status, and the send's fd and type.
    type Seen = (usize, usize, usize, i64, u8);
    static SEEN: Mutex<Vec<Seen>> = Mutex::new(Vec::new());

    unsafe extern "C-unwind" fn fake(context: *mut c_void, _op: u32, _arg: u64) -> u64 {
        // SAFETY: the methods pass their own live context.
        let ctx = unsafe { &*context.cast::<FactoryContext>() };
        let (fd, kind) = if ctx.send.is_null() {
            (0, 0)
        } else {
            // SAFETY: a non-null send is live for the call.
            let info = unsafe { &(*ctx.send).info };
            (info.fd, info.kind)
        };
        let seen = (
            ctx.serv.addr(),
            ctx.worker.addr(),
            ctx.status.addr(),
            fd,
            kind,
        );
        SEEN.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(seen);
        0
    }

    fn last() -> Seen {
        let seen = SEEN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *seen.last().unwrap_or(&(0, 0, 0, 0, 0))
    }

    #[test]
    fn run_fills_the_context() {
        set_factory_call(fake);
        let at = |a: usize| ptr::without_provenance_mut::<c_void>(a);
        let mut close = SendData {
            info: DataHead {
                fd: 9,
                kind: 4,
                ..DataHead::default()
            },
            data: ptr::null(),
        };
        unsafe { run(at(0x10), 0, at(0x20), &raw mut close, at(0x40), 0) };
        assert_eq!(
            last(),
            (0x10, 0x20, 0x40, 9, 4),
            "server, worker, status and send"
        );
    }
}
