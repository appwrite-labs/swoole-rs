//! Per-send loans: bookkeeping runs over the canonical records after native effects.
use crate::ops::{OP_SC_TOUCH_SEND, OP_ST_COUNT_SEND};
use crate::{PortCounters, ServerCounters};
use core::sync::atomic::Ordering;

/// A native invocation lends these cells after verification or successful finish.
/// Loans are refreshed after the callback, so direct native changes remain visible.
#[repr(C)]
#[derive(Default)]
pub struct ServerSendBookkeeping {
    pub last_send_time: *mut f64,
    pub server: *mut ServerCounters,
    pub port: *mut PortCounters,
    pub worker_response_count: *mut i64,
    pub length: u32,
    /// Bit 0: verified timestamp cell; bit 1: successful finish counter loans.
    pub ready: u32,
}

const _: () = {
    use core::mem::{align_of, offset_of, size_of};
    assert!(
        size_of::<ServerSendBookkeeping>() == 40 && align_of::<ServerSendBookkeeping>() == 8,
        "send bookkeeping size/alignment must match the native loan record"
    );
    assert!(
        offset_of!(ServerSendBookkeeping, server) == 8,
        "send bookkeeping server offset must match native"
    );
    assert!(
        offset_of!(ServerSendBookkeeping, port) == 16,
        "send bookkeeping port offset must match native"
    );
    assert!(
        offset_of!(ServerSendBookkeeping, worker_response_count) == 24,
        "send bookkeeping worker_response_count offset must match native"
    );
    assert!(
        offset_of!(ServerSendBookkeeping, length) == 32,
        "send bookkeeping length offset must match native"
    );
    assert!(
        offset_of!(ServerSendBookkeeping, ready) == 36,
        "send bookkeeping ready offset must match native"
    );
};

/// # Safety
/// The invocation lends live, synchronized cells for every set readiness bit.
#[inline(always)]
pub(crate) unsafe fn native_op(record: *mut ServerSendBookkeeping, op: u32) -> Option<u64> {
    if record.is_null() || !matches!(op, OP_SC_TOUCH_SEND | OP_ST_COUNT_SEND) {
        return None;
    }
    // SAFETY: the caller lends this invocation's record; no borrow crosses a callback.
    let record = unsafe { &*record };
    match op {
        OP_SC_TOUCH_SEND if record.ready & 1 != 0 => {
            let now = swoole_rs_corebase::microtime();
            // SAFETY: verification lent this connection's timestamp under native synchronization.
            unsafe { record.last_send_time.write(now) };
        }
        OP_ST_COUNT_SEND if record.ready & 2 != 0 => {
            // SAFETY: successful finish lent the server's mapped atomic counter record.
            let server = unsafe { &*record.server };
            server.response_count.fetch_add(1, Ordering::SeqCst);
            server
                .total_send_bytes
                .fetch_add(u64::from(record.length), Ordering::SeqCst);
            if !record.port.is_null() {
                // SAFETY: the native session lookup lent this mapped port counter record.
                let port = unsafe { &*record.port };
                port.response_count.fetch_add(1, Ordering::SeqCst);
                port.total_send_bytes
                    .fetch_add(u64::from(record.length), Ordering::SeqCst);
            }
            if !record.worker_response_count.is_null() {
                // SAFETY: this worker's response cell is owned by the executing native thread.
                let count = unsafe { record.worker_response_count.read_volatile() };
                // SAFETY: the same exclusive cell; preserve the native counter's machine width.
                unsafe {
                    record
                        .worker_response_count
                        .write_volatile(count.wrapping_add(1));
                };
            }
        }
        _ => return None,
    }
    Some(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ServerReply, ops, swr_v1_server_run_guarded_send};
    use core::ffi::c_void;

    struct Finish {
        record: ServerSendBookkeeping,
        old: ServerCounters,
        current: ServerCounters,
        port: PortCounters,
        worker: i64,
        fault: u32,
        success: bool,
        calls: Vec<u32>,
    }

    unsafe extern "C" fn finish(context: *mut c_void, op: u32, _: u64) -> ServerReply {
        let context = unsafe { &mut *context.cast::<Finish>() };
        context.calls.push(op);
        assert_eq!(
            op,
            ops::OP_ST_FINISH,
            "bookkeeping must not call native again"
        );
        // The callback publishes different live loans, as a native callback may.
        context.record.server = &raw mut context.current;
        context.record.port = &raw mut context.port;
        context.record.worker_response_count = &raw mut context.worker;
        context.record.length = 23;
        context.record.ready = 2;
        ServerReply {
            value: u64::from(context.success),
            fault: context.fault,
        }
    }

    fn run(fault: u32, success: bool) -> (Finish, ServerReply) {
        let mut context = Finish {
            record: ServerSendBookkeeping::default(),
            old: ServerCounters::default(),
            current: ServerCounters::default(),
            port: PortCounters::default(),
            worker: 41,
            fault,
            success,
            calls: Vec::new(),
        };
        context.record.server = &raw mut context.old;
        let reply = unsafe {
            swr_v1_server_run_guarded_send(
                ops::RUN_ST_SEND,
                0,
                (&raw mut context).cast(),
                finish,
                core::ptr::null(),
                &raw mut context.record,
            )
        };
        (context, reply)
    }

    #[test]
    fn finish_counts_the_post_callback_loans_once() {
        let (context, reply) = run(0, true);
        assert_eq!((reply.value, reply.fault), (1, 0));
        assert_eq!(context.old.response_count.load(Ordering::Relaxed), 0);
        assert_eq!(context.current.response_count.load(Ordering::Relaxed), 1);
        assert_eq!(context.current.total_send_bytes.load(Ordering::Relaxed), 23);
        assert_eq!(context.port.response_count.load(Ordering::Relaxed), 1);
        assert_eq!(context.port.total_send_bytes.load(Ordering::Relaxed), 23);
        assert_eq!(context.worker, 42);
        assert_eq!(context.calls, [ops::OP_ST_FINISH]);
    }

    #[test]
    fn every_typed_fault_and_a_refused_finish_stop_before_counting() {
        for (fault, success) in [(0, false), (1, true), (2, true), (3, true), (4, true)] {
            let (context, reply) = run(fault, success);
            assert_eq!(reply.fault, fault);
            assert_eq!(context.current.response_count.load(Ordering::Relaxed), 0);
            assert_eq!(context.current.total_send_bytes.load(Ordering::Relaxed), 0);
            assert_eq!(context.port.response_count.load(Ordering::Relaxed), 0);
            assert_eq!(context.worker, 41);
            assert_eq!(context.calls, [ops::OP_ST_FINISH]);
        }
    }

    #[test]
    fn absent_optional_counters_and_timestamp_use_the_same_loans() {
        let mut server = ServerCounters::default();
        let mut stamp = 0.0;
        let mut record = ServerSendBookkeeping {
            server: &raw mut server,
            last_send_time: &raw mut stamp,
            length: 11,
            ready: 3,
            ..ServerSendBookkeeping::default()
        };
        assert_eq!(
            unsafe { native_op(&raw mut record, OP_SC_TOUCH_SEND) },
            Some(0)
        );
        assert!(stamp.is_finite() && stamp > 0.0);
        assert_eq!(
            unsafe { native_op(&raw mut record, OP_ST_COUNT_SEND) },
            Some(0)
        );
        assert_eq!(server.response_count.load(Ordering::Relaxed), 1);
        assert_eq!(server.total_send_bytes.load(Ordering::Relaxed), 11);
        record.ready = 0;
        assert_eq!(
            unsafe { native_op(&raw mut record, OP_ST_COUNT_SEND) },
            None
        );
    }
}
