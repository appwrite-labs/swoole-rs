//! The coroutine socket's server setup as whole operations: `bind` of a host and
//! `accept` (the would-block wait and retry). Native code keeps the availability
//! check, the timer with its coroutine wait, `set_err` and the `network::Socket`
//! calls that update that object's fields, and passes them in [`SetupOps`].
use core::ffi::{c_int, c_void};

/// `SW_EVENT_NULL`, `SW_EVENT_READ` and `SW_EVENT_WRITE`.
const EVENT_NULL: c_int = 0;
const EVENT_READ: c_int = 1 << 9;
/// `fail` codes besides an error number.
const LAST_ERROR: c_int = -1;

/// Connection setup's native effects. Every callback gets `context`.
#[repr(C)]
pub struct SetupOps {
    pub context: *mut c_void,
    /// `is_available(event)`, which sets the error itself when false.
    pub available: unsafe extern "C" fn(*mut c_void, c_int) -> bool,
    /// `set_err(error)`; `-1` is `set_err()` (the last error, else errno).
    pub fail: unsafe extern "C" fn(*mut c_void, c_int),
    /// Starts the operation's timer and waits for `event`; false on timeout or close.
    pub wait: unsafe extern "C" fn(*mut c_void, c_int) -> bool,
    /// `network::Socket::get_name`, which records the local address.
    pub name: unsafe extern "C" fn(*mut c_void) -> c_int,
    /// The operation's `network::Socket` call: `bind` of its address, or `accept`
    /// (0 with a connection kept, else -1 with errno); it sets errno.
    pub call: unsafe extern "C" fn(*mut c_void, c_int) -> c_int,
}

impl SetupOps {
    fn available(&self, event: c_int) -> bool {
        // SAFETY: the live socket's availability check.
        unsafe { (self.available)(self.context, event) }
    }
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
    fn name(&self) -> c_int {
        // SAFETY: records the live socket's local address.
        unsafe { (self.name)(self.context) }
    }
}

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// `Socket::bind(host, port)`: checks availability, has `call` set `SO_REUSEADDR`
/// and bind, then records the bound address.
///
/// # Safety
/// `ops` is live for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_bind(ops: *const SetupOps) -> bool {
    // SAFETY: the caller passes a live table (see above).
    let ops = unsafe { &*ops };
    if !ops.available(EVENT_NULL) {
        return false;
    }
    if ops.call(0) < 0 || ops.name() < 0 {
        ops.fail(LAST_ERROR);
        return false;
    }
    true
}

/// `Socket::accept(timeout)` after its availability and TLS context checks: accept,
/// and when that would block, wait for the socket and accept once more.
///
/// # Safety
/// `ops` is live for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_accept(ops: *const SetupOps) -> bool {
    // SAFETY: the caller passes a live table (see above).
    let ops = unsafe { &*ops };
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
        available: bool,
        wait: bool,
        calls: Vec<c_int>,
        name: c_int,
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
    unsafe extern "C" fn available(_: *mut c_void, event: c_int) -> bool {
        log(format!("available {event}"));
        FAKE.with(|fake| fake.borrow().available)
    }
    unsafe extern "C" fn fail(_: *mut c_void, error: c_int) {
        log(format!("fail {error}"));
    }
    unsafe extern "C" fn wait(_: *mut c_void, event: c_int) -> bool {
        log(format!("wait {event}"));
        FAKE.with(|fake| fake.borrow().wait)
    }
    unsafe extern "C" fn name(_: *mut c_void) -> c_int {
        log("name".into());
        FAKE.with(|fake| fake.borrow().name)
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

    fn reset(available: bool, wait: bool, calls: &[c_int]) {
        FAKE.with(|fake| {
            *fake.borrow_mut() = Fake {
                available,
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
            available,
            fail,
            wait,
            name,
            call,
        }
    }

    #[test]
    fn bind_and_accept_rules() {
        let ops = ops();
        reset(true, true, &[0]);
        // SAFETY: local table.
        assert!(unsafe { swr_v1_hook_bind(&raw const ops) });
        assert_eq!(logged(), ["available 0", "call 0", "name"]);
        reset(true, true, &[-libc::EADDRINUSE]);
        // SAFETY: as above.
        assert!(!unsafe { swr_v1_hook_bind(&raw const ops) });
        assert_eq!(logged(), ["available 0", "call 0", "fail -1"]);

        reset(true, true, &[-libc::EAGAIN, 0]);
        // SAFETY: local table.
        assert!(unsafe { swr_v1_hook_accept(&raw const ops) });
        assert_eq!(logged(), ["call 0", "wait 512", "call 0"]);
        reset(true, false, &[-libc::EAGAIN]);
        // SAFETY: as above.
        assert!(!unsafe { swr_v1_hook_accept(&raw const ops) });
        assert_eq!(logged(), ["call 0", "wait 512"]);
        reset(true, true, &[-libc::EMFILE]);
        // SAFETY: as above.
        assert!(!unsafe { swr_v1_hook_accept(&raw const ops) });
        assert_eq!(
            logged(),
            ["call 0".to_string(), format!("fail {}", libc::EMFILE)]
        );
    }
}
