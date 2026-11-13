//! The reactor's writable handler and drain (upstream `Reactor::_writable_callback` and
//! `drain_write_buffer`) over the socket's write-path block and its output queue.
//! `crates/tls`'s `reactor_write.rs` supplies the effects.
use crate::Socket;
use std::ffi::c_int;
use swoole_rs_buffer::cpp::{swr_cpp_buffer_count, swr_cpp_buffer_typed_front};

const SW_OK: c_int = swoole_rs_corebase::constants::ResultCode::SW_OK as c_int;
const SW_ERR: c_int = swoole_rs_corebase::constants::ResultCode::SW_ERR as c_int;
/// `BufferChunk::TYPE_SENDFILE` and `TYPE_CLOSE` (the typed kinds use the same numbers).
const SENDFILE: c_int = 1;
const CLOSE: c_int = 2;

/// The flush's effects (`crates/tls`'s `reactor_write.rs`).
pub trait FlushEffects {
    fn handle_send(&self) -> c_int;
    fn handle_sendfile(&self) -> c_int;
    /// `reactor->close(reactor, socket)` and `reactor->trigger_close_event(event)`.
    fn close(&self) -> c_int;
    fn trigger_close(&self) -> c_int;
    /// `swoole_timer_del(socket->send_timer)`.
    fn delete_timer(&self);
    fn remove_write_event(&self);
    /// drain only: `socket->wait_event(write timeout, SW_EVENT_WRITE)`.
    fn wait_writable(&self) -> c_int;
}

/// `Reactor::_writable_callback`: send queued entries until the queue empties or a send
/// can't proceed, then drop the send timer and, once empty, write interest.
pub fn swr_v1_reactor_flush(socket: Socket<'_>, ops: &impl FlushEffects) -> c_int {
    // One read per pass: the front entry's kind (`typed_front().kind`), or the timer
    // once the queue is empty.
    let timer = loop {
        let (kind, timer) = socket.read(|record| {
            let kind = record
                .output
                .buffer()
                .filter(|&buffer| swr_cpp_buffer_count(buffer) != 0)
                .map(|buffer| swr_cpp_buffer_typed_front(buffer).kind.cast_signed());
            (kind, record.output.send_timer)
        });
        let Some(kind) = kind else {
            break timer;
        };
        let result = match kind {
            CLOSE => return ops.close(),
            SENDFILE => ops.handle_sendfile(),
            _ => ops.handle_send(),
        };
        if result < 0 {
            let output = socket.read(|record| record.output);
            if output.close_wait {
                return ops.trigger_close();
            }
            if output.send_wait {
                return SW_OK;
            }
        }
    };
    // Empty, unless deleting the timer queued output.
    let mut empty = true;
    if !timer.is_null() {
        ops.delete_timer();
        empty = socket.edit(|record| {
            record.output.send_timer = std::ptr::null_mut();
            record.output.empty()
        });
    }
    if empty {
        ops.remove_write_event();
    }
    SW_OK
}

/// `Reactor::drain_write_buffer`: wait for writability and flush until the queue empties,
/// the wait fails, or the socket starts closing or leaves the reactor.
pub fn swr_v1_reactor_drain(socket: Socket<'_>, ops: &impl FlushEffects) {
    while !socket.read(|record| record.output.empty()) {
        if ops.wait_writable() == SW_ERR {
            break;
        }
        swr_v1_reactor_flush(socket, ops);
        if socket.read(|record| record.output.close_wait || record.removed) {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SocketOutput, SocketRecord};
    use std::cell::RefCell;
    use std::ffi::c_void;
    use swoole_rs_buffer::cpp::{
        CppBuffer, delete_buffer, new_buffer, swr_cpp_buffer_append, swr_cpp_buffer_pop,
        swr_cpp_buffer_typed_close,
    };

    /// A buffer holding one entry of each kind: a byte of data, an empty sendfile
    /// request or a close marker.
    fn queue(kinds: &[c_int]) -> *mut CppBuffer {
        let buffer = new_buffer(1);
        for &kind in kinds {
            match kind {
                CLOSE => swr_cpp_buffer_typed_close(unsafe { &*buffer }),
                SENDFILE => {
                    unsafe extern "C" fn keep(_: *mut c_void) {}
                    // `Buffer`'s first member is its Rust queue handle.
                    let handle = unsafe { *buffer.cast::<*mut swoole_rs_buffer::Buffer>() };
                    let status = unsafe { (*handle).push(1, std::ptr::dangling_mut(), Some(keep)) };
                    assert_eq!(status, 0);
                }
                _ => unsafe { swr_cpp_buffer_append(&*buffer, b"x".as_ptr(), 1) },
            }
        }
        buffer
    }

    #[derive(Default)]
    struct Native {
        sends: Vec<c_int>,
        trace: Vec<&'static str>,
        buffer: usize,
        close_wait_on_fail: bool,
        record: usize,
    }
    thread_local! { static NATIVE: RefCell<Native> = RefCell::default(); }

    fn record(name: &'static str) {
        NATIVE.with_borrow_mut(|n| n.trace.push(name));
    }
    struct Ops;
    impl FlushEffects for Ops {
        fn handle_send(&self) -> c_int {
            record("send");
            let (result, buffer, close_wait, record) = NATIVE
                .with_borrow_mut(|n| (n.sends.remove(0), n.buffer, n.close_wait_on_fail, n.record));
            if result >= 0 {
                swr_cpp_buffer_pop(unsafe { &*(buffer as *const CppBuffer) });
            } else if close_wait {
                let socket = unsafe { Socket::from_ptr(record as *mut SocketRecord) }.unwrap();
                socket.edit(|record| record.output.close_wait = true);
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
            close_wait: false,
            nonblock: false,
            ssl_send: false,
            send_wait: false,
        }
    }

    fn run(
        output: &mut SocketOutput,
        kinds: &[c_int],
        sends: Vec<c_int>,
        close_wait: bool,
    ) -> (c_int, Vec<&'static str>) {
        let buffer = queue(kinds);
        let mut record = SocketRecord::new();
        record.output = SocketOutput {
            out_buffer: buffer.cast(),
            ..*output
        };
        let socket = unsafe { Socket::from_ptr(&raw mut record) }.unwrap();
        NATIVE.with_borrow_mut(|n| {
            *n = Native {
                sends,
                buffer: buffer as usize,
                close_wait_on_fail: close_wait,
                record: socket.as_ptr() as usize,
                ..Native::default()
            }
        });
        let result = swr_v1_reactor_flush(socket, &Ops);
        unsafe { delete_buffer(buffer) };
        (
            result,
            NATIVE.with_borrow_mut(|n| std::mem::take(&mut n.trace)),
        )
    }

    #[test]
    fn upstream_writable_callback_order() {
        // Two data entries sent, then the timer and write interest go.
        let (r, trace) = run(&mut block(true), &[0, 0], vec![1, 1], false);
        assert_eq!(
            (r, trace),
            (
                0,
                vec!["send", "send", "delete_timer", "remove_write_event"]
            )
        );
        // A queued close closes at once.
        assert_eq!(
            run(&mut block(true), &[CLOSE], vec![], false),
            (7, vec!["close"])
        );
        // A failed send while closing triggers the close event.
        assert_eq!(
            run(&mut block(false), &[0], vec![-1], true),
            (8, vec!["send", "trigger_close"])
        );
        // A failed send while send_wait stops there, timer and interest untouched.
        let mut waiting = SocketOutput {
            send_wait: true,
            ..block(true)
        };
        assert_eq!(run(&mut waiting, &[0], vec![-1], false), (0, vec!["send"]));
        // An empty queue only drops interest; a sendfile entry goes to handle_sendfile.
        assert_eq!(
            run(&mut block(false), &[], vec![], false),
            (0, vec!["remove_write_event"])
        );
        let mut waiting = SocketOutput {
            send_wait: true,
            ..block(false)
        };
        assert_eq!(
            run(&mut waiting, &[SENDFILE], vec![], false),
            (0, vec!["sendfile"])
        );
    }

    /// The former flush gate (tests/modules/reactor/flush.py): upstream's
    /// `_writable_callback` and `drain_write_buffer` over a facade whose sends pop the
    /// front entry and whose close marks the socket removed and clears the queue.
    mod gate {
        use super::super::*;
        use crate::{SocketOutput, SocketRecord};
        use std::cell::RefCell;
        use std::collections::VecDeque;
        use std::ffi::c_void;
        use swoole_rs_buffer::cpp::{
            CppBuffer, delete_buffer, swr_cpp_buffer_count, swr_cpp_buffer_pop,
        };

        #[derive(Default)]
        struct Facade {
            kinds: VecDeque<c_int>,
            trace: Vec<String>,
            buffer: usize,
            record: usize,
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
            let buffer = unsafe { &*(buffer as *const CppBuffer) };
            assert_ne!(swr_cpp_buffer_count(buffer), 0);
            swr_cpp_buffer_pop(buffer);
        }
        struct Ops;
        impl FlushEffects for Ops {
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
                let record = FACADE.with_borrow(|f| f.record) as *mut SocketRecord;
                let socket = unsafe { Socket::from_ptr(record) }.unwrap();
                socket.edit(|record| record.removed = true);
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
        }

        fn run(kinds: &[c_int], timer: bool, drain: bool) -> Vec<String> {
            let buffer = super::queue(kinds);
            let mut record = SocketRecord::new();
            record.output = SocketOutput {
                out_buffer: buffer.cast(),
                send_timer: if timer {
                    std::ptr::dangling_mut::<c_void>()
                } else {
                    std::ptr::null_mut()
                },
                buffer_size: 0,
                close_wait: false,
                nonblock: true,
                ssl_send: false,
                send_wait: false,
            };
            let socket = unsafe { Socket::from_ptr(&raw mut record) }.unwrap();
            FACADE.with_borrow_mut(|f| {
                *f = Facade {
                    kinds: kinds.iter().copied().collect(),
                    buffer: buffer as usize,
                    record: socket.as_ptr() as usize,
                    ..Facade::default()
                }
            });
            if drain {
                swr_v1_reactor_drain(socket, &Ops);
            } else {
                assert_eq!(swr_v1_reactor_flush(socket, &Ops), SW_OK);
            }
            unsafe { delete_buffer(buffer) };
            let output = record.output;
            let removed = u8::from(record.removed);
            let mut trace = FACADE.with_borrow_mut(|f| {
                let remaining = f.kinds.len();
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
