//! The coroutine socket's `connect(host)` address loop: assign the address, and
//! when that rejects a bad host address, resolve the name and try once more. Native
//! code keeps the address, the resolver and the error text, and passes them in
//! [`ResolveOps`].
use core::ffi::{c_int, c_void};

/// Outcomes besides 0 (assigned, or the second try fell through as upstream's does).
const ASSIGN_FAILED: c_int = 1;
const RESOLVE_FAILED: c_int = 2;

#[repr(C)]
pub struct ResolveOps {
    pub context: *mut c_void,
    /// `Address::assign` of the current host and port.
    pub assign: unsafe extern "C" fn(*mut c_void) -> bool,
    /// Whether the last Swoole error is `SW_ERROR_BAD_HOST_ADDR`.
    pub bad_host: unsafe extern "C" fn() -> bool,
    /// Resolves the host name into the current host; false when it found none.
    pub resolve: unsafe extern "C" fn(*mut c_void) -> bool,
}

/// Up to two assignments with a resolve between them. Returns 0, or 1 when an
/// assignment failed other than with a bad host address, or 2 when the resolve
/// failed.
///
/// # Safety
/// `ops` is live for the call.
#[must_use]
pub unsafe fn swr_v1_hook_resolve(ops: *const ResolveOps) -> c_int {
    // SAFETY: the caller passes a live table (see above).
    let ops = unsafe { &*ops };
    for _ in 0..2 {
        // SAFETY: the caller's address assignment.
        if unsafe { (ops.assign)(ops.context) } {
            break;
        }
        // SAFETY: reads the thread's last Swoole error.
        if !unsafe { (ops.bad_host)() } {
            return ASSIGN_FAILED;
        }
        // SAFETY: the caller's resolver.
        if !unsafe { (ops.resolve)(ops.context) } {
            return RESOLVE_FAILED;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    thread_local! {
        static ASSIGNS: RefCell<Vec<bool>> = const { RefCell::new(Vec::new()) };
        static RESOLVES: RefCell<Vec<bool>> = const { RefCell::new(Vec::new()) };
        static BAD: RefCell<bool> = const { RefCell::new(false) };
        static LOG: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
    }

    unsafe extern "C" fn assign(_: *mut c_void) -> bool {
        LOG.with(|log| log.borrow_mut().push("assign"));
        ASSIGNS.with(|a| a.borrow_mut().remove(0))
    }
    unsafe extern "C" fn bad_host() -> bool {
        BAD.with(|b| *b.borrow())
    }
    unsafe extern "C" fn resolve(_: *mut c_void) -> bool {
        LOG.with(|log| log.borrow_mut().push("resolve"));
        RESOLVES.with(|r| r.borrow_mut().remove(0))
    }

    fn run(assigns: &[bool], resolves: &[bool], bad: bool) -> (c_int, Vec<&'static str>) {
        ASSIGNS.with(|a| *a.borrow_mut() = assigns.to_vec());
        RESOLVES.with(|r| *r.borrow_mut() = resolves.to_vec());
        BAD.with(|b| *b.borrow_mut() = bad);
        LOG.with(|log| log.borrow_mut().clear());
        let ops = ResolveOps {
            context: std::ptr::null_mut(),
            assign,
            bad_host,
            resolve,
        };
        // SAFETY: local table.
        (
            unsafe { swr_v1_hook_resolve(&raw const ops) },
            LOG.with(|log| log.borrow().clone()),
        )
    }

    #[test]
    fn assign_resolve_and_retry() {
        assert_eq!(run(&[true], &[], false), (0, vec!["assign"]), "an address");
        assert_eq!(
            run(&[false, true], &[true], true),
            (0, vec!["assign", "resolve", "assign"]),
            "a name"
        );
        assert_eq!(
            run(&[false], &[], false),
            (ASSIGN_FAILED, vec!["assign"]),
            "another error"
        );
        assert_eq!(
            run(&[false], &[false], true),
            (RESOLVE_FAILED, vec!["assign", "resolve"])
        );
        // A second bad host address resolves again and falls through, as upstream's loop does.
        assert_eq!(
            run(&[false, false], &[true, true], true),
            (0, vec!["assign", "resolve", "assign", "resolve"])
        );
    }
}
