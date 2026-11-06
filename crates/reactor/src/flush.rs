//! The reactor's writable handler and drain (upstream `Reactor::_writable_callback` and
//! `drain_write_buffer`) over the socket's write-path block and its output queue.
//! Native code performs the effects that need its objects.
use crate::write::SocketOutput;
use std::ffi::{c_int, c_void};
use swoole_rs_buffer::{Buffer, swr_v1_buffer_stats};

const SW_OK: c_int = 0;
const SW_ERR: c_int = -1;
/// `BufferChunk::TYPE_SENDFILE` and `TYPE_CLOSE` (the typed kinds use the same numbers).
const SENDFILE: c_int = 1;
const CLOSE: c_int = 2;

/// Native effects. Each receives `context`.
#[repr(C)]
pub struct FlushOps {
    pub context: *mut c_void,
    /// The front entry's kind (`typed_front().kind` or `front()->type`).
    pub front_kind: unsafe extern "C" fn(*mut c_void) -> c_int,
    pub handle_send: unsafe extern "C" fn(*mut c_void) -> c_int,
    pub handle_sendfile: unsafe extern "C" fn(*mut c_void) -> c_int,
    /// `reactor->close(reactor, socket)` and `reactor->trigger_close_event(event)`.
    pub close: unsafe extern "C" fn(*mut c_void) -> c_int,
    pub trigger_close: unsafe extern "C" fn(*mut c_void) -> c_int,
    /// `swoole_timer_del(socket->send_timer)`.
    pub delete_timer: unsafe extern "C" fn(*mut c_void),
    pub remove_write_event: unsafe extern "C" fn(*mut c_void),
    /// drain only: `socket->wait_event(write timeout, SW_EVENT_WRITE)`, and `socket->removed`.
    pub wait_writable: Option<unsafe extern "C" fn(*mut c_void) -> c_int>,
    pub removed: Option<unsafe extern "C" fn(*mut c_void) -> bool>,
}

/// `Buffer::empty`: null, no entries, or a handle that can't be read.
///
/// # Safety
/// `buffer` is null or a live Rust buffer handle.
unsafe fn empty(buffer: *mut Buffer) -> bool {
    if buffer.is_null() {
        return true;
    }
    let (mut length, mut count) = (0u32, 0usize);
    // SAFETY: as the caller promises; both outputs are local.
    unsafe { swr_v1_buffer_stats(buffer, &raw mut length, &raw mut count) != 0 || count == 0 }
}

/// `Reactor::_writable_callback`: send queued entries until the queue empties or a send
/// can't proceed, then drop the send timer and, once empty, write interest.
///
/// # Safety
/// `output` is the socket's embedded block; `buffer` is its `out_buffer`'s Rust handle
/// or null; `ops` is valid for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_flush(
    output: *mut SocketOutput,
    buffer: *mut Buffer,
    ops: *const FlushOps,
) -> c_int {
    // SAFETY: as the caller promises.
    let ops = unsafe { &*ops };
    let context = ops.context;
    // SAFETY: as above.
    while !unsafe { empty(buffer) } {
        // SAFETY: the native effects accept `context`.
        let result = match unsafe { (ops.front_kind)(context) } {
            // SAFETY: as above.
            CLOSE => return unsafe { (ops.close)(context) },
            // SAFETY: as above.
            SENDFILE => unsafe { (ops.handle_sendfile)(context) },
            // SAFETY: as above.
            _ => unsafe { (ops.handle_send)(context) },
        };
        if result < 0 {
            // SAFETY: the socket's block, read fresh after each native effect.
            let output = unsafe { &*output };
            if output.close_wait != 0 {
                // SAFETY: as above.
                return unsafe { (ops.trigger_close)(context) };
            }
            if output.send_wait != 0 {
                return SW_OK;
            }
        }
    }
    // SAFETY: as above.
    if !unsafe { (*output).send_timer }.is_null() {
        // SAFETY: as above.
        unsafe { (ops.delete_timer)(context) };
        // SAFETY: as above.
        unsafe { (*output).send_timer = std::ptr::null_mut() };
    }
    // SAFETY: as above.
    if unsafe { empty(buffer) } {
        // SAFETY: as above.
        unsafe { (ops.remove_write_event)(context) };
    }
    SW_OK
}

/// `Reactor::drain_write_buffer`: wait for writability and flush until the queue empties,
/// the wait fails, or the socket starts closing or leaves the reactor.
///
/// # Safety
/// As for [`swr_v1_reactor_flush`]; `ops.wait_writable` and `ops.removed` are set.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_drain(
    output: *mut SocketOutput,
    buffer: *mut Buffer,
    ops: *const FlushOps,
) {
    // SAFETY: as the caller promises.
    let ops_ref = unsafe { &*ops };
    let (Some(wait_writable), Some(removed)) = (ops_ref.wait_writable, ops_ref.removed) else {
        return;
    };
    let context = ops_ref.context;
    // SAFETY: as above.
    while !unsafe { empty(buffer) } {
        // SAFETY: the native effect accepts `context`.
        if unsafe { wait_writable(context) } == SW_ERR {
            break;
        }
        // SAFETY: as the caller promises.
        unsafe { swr_v1_reactor_flush(output, buffer, ops) };
        // SAFETY: the socket's block, read fresh after the flush.
        if unsafe { (*output).close_wait } != 0 || unsafe { removed(context) } {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
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
    unsafe extern "C" fn front_kind(_: *mut c_void) -> c_int {
        NATIVE.with_borrow_mut(|n| n.kinds.remove(0))
    }
    unsafe extern "C" fn handle_send(_: *mut c_void) -> c_int {
        record("send");
        let (result, buffer, close_wait, output) = NATIVE
            .with_borrow_mut(|n| (n.sends.remove(0), n.buffer, n.close_wait_on_fail, n.output));
        if result >= 0 {
            let mut detached = TypedDetached::default();
            unsafe { swr_v1_buffer_typed_pop(buffer as *mut Buffer, &raw mut detached) };
        } else if close_wait {
            unsafe { (*(output as *mut SocketOutput)).close_wait = 1 };
        }
        result
    }
    unsafe extern "C" fn handle_sendfile(_: *mut c_void) -> c_int {
        record("sendfile");
        -1
    }
    unsafe extern "C" fn close(_: *mut c_void) -> c_int {
        record("close");
        7
    }
    unsafe extern "C" fn trigger_close(_: *mut c_void) -> c_int {
        record("trigger_close");
        8
    }
    unsafe extern "C" fn delete_timer(_: *mut c_void) {
        record("delete_timer");
    }
    unsafe extern "C" fn remove_write_event(_: *mut c_void) {
        record("remove_write_event");
    }

    fn ops() -> FlushOps {
        FlushOps {
            context: std::ptr::null_mut(),
            front_kind,
            handle_send,
            handle_sendfile,
            close,
            trigger_close,
            delete_timer,
            remove_write_event,
            wait_writable: None,
            removed: None,
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
        assert_eq!(unsafe { swr_v1_buffer_typed_create(1, &raw mut buffer) }, 0);
        for _ in 0..entries {
            assert_eq!(unsafe { swr_v1_buffer_append(buffer, b"x".as_ptr(), 1) }, 0);
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
        let ops = ops();
        let result = unsafe { swr_v1_reactor_flush(output, buffer, &raw const ops) };
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
}
