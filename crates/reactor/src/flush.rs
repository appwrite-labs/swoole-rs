//! The reactor's writable handler and drain (upstream `Reactor::_writable_callback` and
//! `drain_write_buffer`) over the socket's write-path block and its output queue.
//! `crates/tls`'s `reactor_write.rs` supplies the effects.
use crate::write::SocketOutput;
use std::ffi::c_int;
use swoole_rs_buffer::{Buffer, swr_v1_buffer_stats};

const SW_OK: c_int = 0;
const SW_ERR: c_int = -1;
/// `BufferChunk::TYPE_SENDFILE` and `TYPE_CLOSE` (the typed kinds use the same numbers).
const SENDFILE: c_int = 1;
const CLOSE: c_int = 2;

/// The flush's effects (`crates/tls`'s `reactor_write.rs`).
pub trait FlushEffects {
    /// The front entry's kind (`typed_front().kind` or `front()->type`).
    fn front_kind(&self) -> c_int;
    fn handle_send(&self) -> c_int;
    fn handle_sendfile(&self) -> c_int;
    /// `reactor->close(reactor, socket)` and `reactor->trigger_close_event(event)`.
    fn close(&self) -> c_int;
    fn trigger_close(&self) -> c_int;
    /// `swoole_timer_del(socket->send_timer)`.
    fn delete_timer(&self);
    fn remove_write_event(&self);
    /// drain only: `socket->wait_event(write timeout, SW_EVENT_WRITE)`, and `socket->removed`.
    fn wait_writable(&self) -> c_int;
    fn removed(&self) -> bool;
}

/// `Buffer::empty`: null, no entries, or a handle that can't be read.
fn empty(buffer: Option<&Buffer>) -> bool {
    let Some(buffer) = buffer else {
        return true;
    };
    let (mut length, mut count) = (0u32, 0usize);
    swr_v1_buffer_stats(Some(buffer), Some(&mut length), Some(&mut count)) != 0 || count == 0
}

/// `Reactor::_writable_callback`: send queued entries until the queue empties or a send
/// can't proceed, then drop the send timer and, once empty, write interest.
///
/// # Safety
/// `output` is the socket's embedded block; `buffer` is its `out_buffer`'s Rust handle
/// or null. Both stay live across the effects, which may change them.
pub unsafe fn swr_v1_reactor_flush(
    output: *mut SocketOutput,
    buffer: *mut Buffer,
    ops: &impl FlushEffects,
) -> c_int {
    // SAFETY: as the caller promises; the borrow ends with the check.
    while !empty(unsafe { buffer.as_ref() }) {
        let result = match ops.front_kind() {
            CLOSE => return ops.close(),
            SENDFILE => ops.handle_sendfile(),
            _ => ops.handle_send(),
        };
        if result < 0 {
            // SAFETY: the socket's block, read fresh after each effect.
            let output = unsafe { &*output };
            if output.close_wait != 0 {
                return ops.trigger_close();
            }
            if output.send_wait != 0 {
                return SW_OK;
            }
        }
    }
    // SAFETY: as above; no reference to the block spans the effect.
    if !unsafe { (*output).send_timer }.is_null() {
        ops.delete_timer();
        // SAFETY: as above.
        unsafe { (*output).send_timer = std::ptr::null_mut() };
    }
    // SAFETY: as above.
    if empty(unsafe { buffer.as_ref() }) {
        ops.remove_write_event();
    }
    SW_OK
}

/// `Reactor::drain_write_buffer`: wait for writability and flush until the queue empties,
/// the wait fails, or the socket starts closing or leaves the reactor.
///
/// # Safety
/// As for [`swr_v1_reactor_flush`].
pub unsafe fn swr_v1_reactor_drain(
    output: *mut SocketOutput,
    buffer: *mut Buffer,
    ops: &impl FlushEffects,
) {
    // SAFETY: as the caller promises; the borrow ends with the check.
    while !empty(unsafe { buffer.as_ref() }) {
        if ops.wait_writable() == SW_ERR {
            break;
        }
        // SAFETY: as the caller promises.
        unsafe { swr_v1_reactor_flush(output, buffer, ops) };
        // SAFETY: the socket's block, read fresh after the flush.
        if unsafe { (*output).close_wait } != 0 || ops.removed() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::ffi::c_void;
    use swoole_rs_buffer::{
        TypedDetached, swr_v1_buffer_append, swr_v1_buffer_release, swr_v1_buffer_typed_create,
        swr_v1_buffer_typed_pop,
    };

    #[derive(Default)]
    struct Native {
        kinds: Vec<c_int>,
        sends: Vec<c_int>,
        trace: Vec<&'static str>,
        buffer: usize,
        close_wait_on_fail: bool,
        output: usize,
    }
    thread_local! { static NATIVE: RefCell<Native> = RefCell::default(); }

    fn record(name: &'static str) {
        NATIVE.with_borrow_mut(|n| n.trace.push(name));
    }
    struct Ops;
    impl FlushEffects for Ops {
        fn front_kind(&self) -> c_int {
            NATIVE.with_borrow_mut(|n| n.kinds.remove(0))
        }
        fn handle_send(&self) -> c_int {
            record("send");
            let (result, buffer, close_wait, output) = NATIVE
                .with_borrow_mut(|n| (n.sends.remove(0), n.buffer, n.close_wait_on_fail, n.output));
            if result >= 0 {
                let mut detached = TypedDetached::default();
                swr_v1_buffer_typed_pop(
                    unsafe { (buffer as *mut Buffer).as_mut() },
                    Some(&mut detached),
                );
            } else if close_wait {
                unsafe { (*(output as *mut SocketOutput)).close_wait = 1 };
            }
            result
        }
        fn handle_sendfile(&self) -> c_int {
            record("sendfile");
            -1
        }
        fn close(&self) -> c_int {
            record("close");
            7
        }
        fn trigger_close(&self) -> c_int {
            record("trigger_close");
            8
        }
        fn delete_timer(&self) {
            record("delete_timer");
        }
        fn remove_write_event(&self) {
            record("remove_write_event");
        }
        fn wait_writable(&self) -> c_int {
            unreachable!("a flush never waits")
        }
        fn removed(&self) -> bool {
            unreachable!("a flush never asks")
        }
    }

    fn block(timer: bool) -> SocketOutput {
        SocketOutput {
            out_buffer: std::ptr::null_mut(),
            send_timer: if timer {
                std::ptr::dangling_mut::<c_void>()
            } else {
                std::ptr::null_mut()
            },
            buffer_size: 0,
            close_wait: 0,
            nonblock: 0,
            ssl_send: 0,
            send_wait: 0,
        }
    }

    fn run(
        output: &mut SocketOutput,
        entries: usize,
        kinds: Vec<c_int>,
        sends: Vec<c_int>,
        close_wait: bool,
    ) -> (c_int, Vec<&'static str>) {
        let mut buffer = std::ptr::null_mut();
        assert_eq!(swr_v1_buffer_typed_create(1, Some(&mut buffer)), 0);
        for _ in 0..entries {
            assert_eq!(
                unsafe { swr_v1_buffer_append(buffer.as_mut(), b"x".as_ptr(), 1) },
                0
            );
        }
        NATIVE.with_borrow_mut(|n| {
            *n = Native {
                kinds,
                sends,
                buffer: buffer as usize,
                close_wait_on_fail: close_wait,
                output: std::ptr::from_mut(output) as usize,
                ..Native::default()
            }
        });
        let result = unsafe { swr_v1_reactor_flush(output, buffer, &Ops) };
        unsafe { swr_v1_buffer_release(buffer) };
        (
            result,
            NATIVE.with_borrow_mut(|n| std::mem::take(&mut n.trace)),
        )
    }

    #[test]
    fn upstream_writable_callback_order() {
        // Two data entries sent, then the timer and write interest go.
        let (r, trace) = run(&mut block(true), 2, vec![0, 0], vec![1, 1], false);
        assert_eq!(
            (r, trace),
            (
                0,
                vec!["send", "send", "delete_timer", "remove_write_event"]
            )
        );
        // A queued close closes at once.
        assert_eq!(
            run(&mut block(true), 1, vec![CLOSE], vec![], false),
            (7, vec!["close"])
        );
        // A failed send while closing triggers the close event.
        assert_eq!(
            run(&mut block(false), 1, vec![0], vec![-1], true),
            (8, vec!["send", "trigger_close"])
        );
        // A failed send while send_wait stops there, timer and interest untouched.
        let mut waiting = SocketOutput {
            send_wait: 1,
            ..block(true)
        };
        assert_eq!(
            run(&mut waiting, 1, vec![0], vec![-1], false),
            (0, vec!["send"])
        );
        // An empty queue only drops interest; a sendfile entry goes to handle_sendfile.
        assert_eq!(
            run(&mut block(false), 0, vec![], vec![], false),
            (0, vec!["remove_write_event"])
        );
        let mut waiting = SocketOutput {
            send_wait: 1,
            ..block(false)
        };
        assert_eq!(
            run(&mut waiting, 1, vec![SENDFILE], vec![], false),
            (0, vec!["sendfile"])
        );
    }

    /// The former flush gate (tests/modules/reactor/flush.py): upstream's
    /// `_writable_callback` and `drain_write_buffer` over a facade whose sends pop the
    /// front entry and whose close marks the socket removed and clears the queue.
    mod gate {
        use super::super::*;
        use std::cell::RefCell;
        use std::collections::VecDeque;
        use std::ffi::c_void;
        use swoole_rs_buffer::{
            TypedDetached, swr_v1_buffer_append, swr_v1_buffer_release, swr_v1_buffer_typed_create,
            swr_v1_buffer_typed_pop,
        };

        #[derive(Default)]
        struct Facade {
            kinds: VecDeque<c_int>,
            trace: Vec<String>,
            removed: bool,
            buffer: usize,
        }
        thread_local! { static FACADE: RefCell<Facade> = RefCell::default(); }

        fn effect(name: &str) {
            FACADE.with_borrow_mut(|f| f.trace.push(name.into()));
        }
        fn pop() {
            let buffer = FACADE.with_borrow_mut(|f| {
                f.kinds.pop_front();
                f.buffer
            });
            let mut detached = TypedDetached::default();
            assert_eq!(
                swr_v1_buffer_typed_pop(
                    unsafe { (buffer as *mut Buffer).as_mut() },
                    Some(&mut detached)
                ),
                0
            );
        }
        struct Ops;
        impl FlushEffects for Ops {
            fn front_kind(&self) -> c_int {
                FACADE.with_borrow(|f| f.kinds[0])
            }
            fn handle_send(&self) -> c_int {
                effect("data");
                pop();
                SW_OK
            }
            fn handle_sendfile(&self) -> c_int {
                effect("sendfile");
                pop();
                SW_OK
            }
            fn close(&self) -> c_int {
                effect("close");
                FACADE.with_borrow_mut(|f| f.removed = true);
                while !FACADE.with_borrow(|f| f.kinds.is_empty()) {
                    pop();
                }
                SW_OK
            }
            fn trigger_close(&self) -> c_int {
                effect("trigger-close");
                SW_OK
            }
            fn delete_timer(&self) {
                effect("timer");
            }
            fn remove_write_event(&self) {
                effect("remove-write");
            }
            fn wait_writable(&self) -> c_int {
                effect("wait");
                SW_OK
            }
            fn removed(&self) -> bool {
                FACADE.with_borrow(|f| f.removed)
            }
        }

        fn run(kinds: &[c_int], timer: bool, drain: bool) -> Vec<String> {
            let mut buffer = std::ptr::null_mut();
            assert_eq!(swr_v1_buffer_typed_create(1, Some(&mut buffer)), 0);
            for _ in kinds {
                assert_eq!(
                    unsafe { swr_v1_buffer_append(buffer.as_mut(), b"x".as_ptr(), 1) },
                    0
                );
            }
            FACADE.with_borrow_mut(|f| {
                *f = Facade {
                    kinds: kinds.iter().copied().collect(),
                    buffer: buffer as usize,
                    ..Facade::default()
                }
            });
            let mut output = SocketOutput {
                out_buffer: std::ptr::null_mut(),
                send_timer: if timer {
                    std::ptr::dangling_mut::<c_void>()
                } else {
                    std::ptr::null_mut()
                },
                buffer_size: 0,
                close_wait: 0,
                nonblock: 1,
                ssl_send: 0,
                send_wait: 0,
            };
            unsafe {
                if drain {
                    swr_v1_reactor_drain(&raw mut output, buffer, &Ops);
                } else {
                    assert_eq!(swr_v1_reactor_flush(&raw mut output, buffer, &Ops), SW_OK);
                }
                swr_v1_buffer_release(buffer);
            }
            let mut trace = FACADE.with_borrow_mut(|f| {
                let remaining = f.kinds.len();
                let removed = u8::from(f.removed);
                let mut trace = std::mem::take(&mut f.trace);
                trace.push(format!("remaining={remaining}"));
                trace.push(format!("removed={removed}"));
                trace
            });
            trace.push(format!("timer={}", u8::from(!output.send_timer.is_null())));
            trace
        }

        const D: c_int = 0;
        const F: c_int = SENDFILE;
        const C: c_int = CLOSE;
        /// Each row is upstream's trace for (timer, drain, queued kinds), as the gate's
        /// original variant produced it.
        #[rustfmt::skip]
        const ROWS: [(bool, bool, &[c_int], &[&str]); 24] = [
            (false, false, &[], &["remove-write", "remaining=0", "removed=0", "timer=0"]),
            (false, false, &[D], &["data", "remove-write", "remaining=0", "removed=0", "timer=0"]),
            (false, false, &[F], &["sendfile", "remove-write", "remaining=0", "removed=0", "timer=0"]),
            (false, false, &[D, F, D], &["data", "sendfile", "data", "remove-write", "remaining=0", "removed=0", "timer=0"]),
            (false, false, &[C], &["close", "remaining=0", "removed=1", "timer=0"]),
            (false, false, &[D, C], &["data", "close", "remaining=0", "removed=1", "timer=0"]),
            (false, true, &[], &["remaining=0", "removed=0", "timer=0"]),
            (false, true, &[D], &["wait", "data", "remove-write", "remaining=0", "removed=0", "timer=0"]),
            (false, true, &[F], &["wait", "sendfile", "remove-write", "remaining=0", "removed=0", "timer=0"]),
            (false, true, &[D, F, D], &["wait", "data", "sendfile", "data", "remove-write", "remaining=0", "removed=0", "timer=0"]),
            (false, true, &[C], &["wait", "close", "remaining=0", "removed=1", "timer=0"]),
            (false, true, &[D, C], &["wait", "data", "close", "remaining=0", "removed=1", "timer=0"]),
            (true, false, &[], &["timer", "remove-write", "remaining=0", "removed=0", "timer=0"]),
            (true, false, &[D], &["data", "timer", "remove-write", "remaining=0", "removed=0", "timer=0"]),
            (true, false, &[F], &["sendfile", "timer", "remove-write", "remaining=0", "removed=0", "timer=0"]),
            (true, false, &[D, F, D], &["data", "sendfile", "data", "timer", "remove-write", "remaining=0", "removed=0", "timer=0"]),
            (true, false, &[C], &["close", "remaining=0", "removed=1", "timer=1"]),
            (true, false, &[D, C], &["data", "close", "remaining=0", "removed=1", "timer=1"]),
            (true, true, &[], &["remaining=0", "removed=0", "timer=1"]),
            (true, true, &[D], &["wait", "data", "timer", "remove-write", "remaining=0", "removed=0", "timer=0"]),
            (true, true, &[F], &["wait", "sendfile", "timer", "remove-write", "remaining=0", "removed=0", "timer=0"]),
            (true, true, &[D, F, D], &["wait", "data", "sendfile", "data", "timer", "remove-write", "remaining=0", "removed=0", "timer=0"]),
            (true, true, &[C], &["wait", "close", "remaining=0", "removed=1", "timer=1"]),
            (true, true, &[D, C], &["wait", "data", "close", "remaining=0", "removed=1", "timer=1"]),
        ];

        #[test]
        fn upstream_writable_callback_and_drain_matrix() {
            for (timer, drain, kinds, expected) in ROWS {
                assert_eq!(
                    run(kinds, timer, drain),
                    expected,
                    "timer={timer} drain={drain} kinds={kinds:?}"
                );
            }
        }
    }
}
