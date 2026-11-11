//! The coroutine socket's `accept` (the would-block wait and retry). Its caller
//! passes the timer with its coroutine wait, `set_err` and the `network::Socket`
//! call in [`SetupOps`].
use core::ffi::{c_int, c_void};

/// `SW_EVENT_READ`.
const EVENT_READ: c_int = 1 << 9;

/// Connection setup's native effects. Every callback gets `context`.
#[repr(C)]
pub struct SetupOps {
    pub context: *mut c_void,
    /// `set_err(error)`; `-1` is `set_err()` (the last error, else errno).
    pub fail: unsafe extern "C" fn(*mut c_void, c_int),
    /// Starts the operation's timer and waits for `event`; false on timeout or close.
    pub wait: unsafe extern "C" fn(*mut c_void, c_int) -> bool,
    /// The `network::Socket` call `accept` (0 with a connection kept, else -1 with
    /// errno).
    pub call: unsafe extern "C" fn(*mut c_void, c_int) -> c_int,
}

impl SetupOps {
    fn fail(&self, error: c_int) {
        // SAFETY: sets the live socket's error.
        unsafe { (self.fail)(self.context, error) }
    }
    fn wait(&self, event: c_int) -> bool {
        // SAFETY: the live socket's timer and coroutine wait.
        unsafe { (self.wait)(self.context, event) }
    }
    fn call(&self, argument: c_int) -> c_int {
        // SAFETY: the operation's network::Socket call on the live socket.
        unsafe { (self.call)(self.context, argument) }
    }
}

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// `Socket::accept(timeout)` after its availability and TLS context checks: accept,
/// and when that would block, wait for the socket and accept once more.
pub(crate) fn swr_v1_hook_accept(ops: &SetupOps) -> bool {
    let mut accepted = ops.call(0) == 0;
    if !accepted && errno() == libc::EAGAIN {
        if !ops.wait(EVENT_READ) {
            return false;
        }
        accepted = ops.call(0) == 0;
    }
    if !accepted {
        ops.fail(errno());
    }
    accepted
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Fake {
        log: Vec<String>,
        wait: bool,
        calls: Vec<c_int>,
    }

    thread_local! {
        static FAKE: RefCell<Fake> = RefCell::new(Fake::default());
    }

    fn set_errno(value: c_int) {
        #[cfg(target_os = "linux")]
        // SAFETY: this thread's errno slot.
        unsafe {
            *libc::__errno_location() = value;
        }
        #[cfg(not(target_os = "linux"))]
        // SAFETY: this thread's errno slot.
        unsafe {
            *libc::__error() = value;
        }
    }

    fn log(entry: String) {
        FAKE.with(|fake| fake.borrow_mut().log.push(entry));
    }
    unsafe extern "C" fn fail(_: *mut c_void, error: c_int) {
        log(format!("fail {error}"));
    }
    unsafe extern "C" fn wait(_: *mut c_void, event: c_int) -> bool {
        log(format!("wait {event}"));
        FAKE.with(|fake| fake.borrow().wait)
    }
    /// Results queued in `calls` (as `-errno` for a failure), first to last.
    unsafe extern "C" fn call(_: *mut c_void, argument: c_int) -> c_int {
        log(format!("call {argument}"));
        let result = FAKE.with(|fake| fake.borrow_mut().calls.remove(0));
        if result < 0 {
            set_errno(-result);
            -1
        } else {
            result
        }
    }

    fn reset(wait: bool, calls: &[c_int]) {
        FAKE.with(|fake| {
            *fake.borrow_mut() = Fake {
                wait,
                calls: calls.to_vec(),
                ..Fake::default()
            }
        });
    }
    fn logged() -> Vec<String> {
        FAKE.with(|fake| fake.borrow().log.clone())
    }

    fn ops() -> SetupOps {
        SetupOps {
            context: std::ptr::null_mut(),
            fail,
            wait,
            call,
        }
    }

    #[test]
    fn accept_rules() {
        let ops = ops();
        reset(true, &[-libc::EAGAIN, 0]);
        // SAFETY: local table.
        assert!(swr_v1_hook_accept(&ops));
        assert_eq!(logged(), ["call 0", "wait 512", "call 0"]);
        reset(false, &[-libc::EAGAIN]);
        // SAFETY: as above.
        assert!(!swr_v1_hook_accept(&ops));
        assert_eq!(logged(), ["call 0", "wait 512"]);
        reset(true, &[-libc::EMFILE]);
        // SAFETY: as above.
        assert!(!swr_v1_hook_accept(&ops));
        assert_eq!(
            logged(),
            ["call 0".to_string(), format!("fail {}", libc::EMFILE)]
        );
    }
}
