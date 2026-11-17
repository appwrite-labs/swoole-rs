//! The coroutine socket's `accept` (the would-block wait and retry). Its caller
//! passes the timer with its coroutine wait, `set_err` and the `network::Socket`
//! call as a [`Setup`].
use core::ffi::c_int;
use std::io;
use swoole_rs_corebase::reactor::Events;

/// Connection setup's native effects.
pub trait Setup {
    /// Publishes the failed accept's captured OS error.
    fn fail(&mut self, error: c_int);
    /// Starts the operation's timer and waits for `event`; false on timeout or close.
    fn wait(&mut self, event: Events) -> bool;
    /// Accepts and keeps a connection.
    ///
    /// # Errors
    /// Returns the OS error captured when accepting fails.
    fn accept(&mut self) -> io::Result<()>;
}

use swoole_rs_corebase::sys::errno;

/// `Socket::accept(timeout)` after its availability and TLS context checks: accept,
/// and when that would block, wait for the socket and accept once more.
pub(crate) fn swr_v1_hook_accept(ops: &mut impl Setup) -> bool {
    let mut accepted = ops.accept();
    if accepted
        .as_ref()
        .is_err_and(|error| error.raw_os_error() == Some(libc::EAGAIN))
    {
        if !ops.wait(Events::READ) {
            return false;
        }
        accepted = ops.accept();
    }
    if let Err(error) = &accepted {
        ops.fail(error.raw_os_error().unwrap_or_else(errno));
    }
    accepted.is_ok()
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

    struct Ops;

    impl Setup for Ops {
        fn fail(&mut self, error: c_int) {
            log(format!("fail {error}"));
        }
        fn wait(&mut self, event: Events) -> bool {
            log(format!("wait {event}"));
            FAKE.with(|fake| fake.borrow().wait)
        }
        /// Results queued in `calls` (as `-errno` for a failure), first to last.
        fn accept(&mut self) -> io::Result<()> {
            log("call 0".to_owned());
            let result = FAKE.with(|fake| fake.borrow_mut().calls.remove(0));
            if result < 0 {
                set_errno(-result);
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
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

    #[test]
    fn accept_rules() {
        let mut ops = Ops;
        reset(true, &[-libc::EAGAIN, 0]);
        assert!(swr_v1_hook_accept(&mut ops));
        assert_eq!(logged(), ["call 0", "wait 512", "call 0"]);
        // A second would-block is published without another wait/retry.
        reset(true, &[-libc::EAGAIN, -libc::EAGAIN]);
        assert!(!swr_v1_hook_accept(&mut ops));
        assert_eq!(
            logged(),
            [
                "call 0".to_owned(),
                "wait 512".to_owned(),
                "call 0".to_owned(),
                format!("fail {}", libc::EAGAIN)
            ]
        );
        reset(false, &[-libc::EAGAIN]);
        assert!(!swr_v1_hook_accept(&mut ops));
        assert_eq!(logged(), ["call 0", "wait 512"]);
        reset(true, &[-libc::EMFILE]);
        assert!(!swr_v1_hook_accept(&mut ops));
        assert_eq!(
            logged(),
            ["call 0".to_string(), format!("fail {}", libc::EMFILE)]
        );
    }
}
