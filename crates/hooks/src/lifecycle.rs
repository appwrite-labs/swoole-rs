//! `Socket::cancel` and `Socket::close` of the coroutine socket: which bound
//! coroutine to wake, the half-close before closing, close-wait while coroutines
//! are still bound, and releasing the descriptor. Native code keeps the
//! coroutines, the destructor callback and the socket's fields, and passes them
//! in `SocketOps`.
use crate::shutdown::SocketState;
use core::ffi::c_void;

/// The socket's effects and the event values it uses. Every callback gets `context`.
#[repr(C)]
pub struct SocketOps {
    pub context: *mut c_void,
    /// The socket's Rust-owned connection state.
    pub state: *mut SocketState,
    /// `SW_EVENT_READ` and `SW_EVENT_WRITE`.
    pub read: u32,
    pub write: u32,
    /// `SW_ERROR_CO_SOCKET_CLOSE_WAIT`.
    pub close_wait_error: i32,
    pub closed: unsafe extern "C" fn(*mut c_void) -> bool,
    /// `Socket::shutdown()` of both directions.
    pub shutdown: unsafe extern "C" fn(*mut c_void),
    /// `has_bound(event)`.
    pub bound: unsafe extern "C" fn(*mut c_void, u32) -> bool,
    pub set_err: unsafe extern "C" fn(*mut c_void, i32),
    /// Resumes the coroutine bound to `event` (read or write).
    pub resume: unsafe extern "C" fn(*mut c_void, u32),
    /// `socket->close_wait = 1`.
    pub close_wait: unsafe extern "C" fn(*mut c_void),
    /// Marks the descriptor closed and runs the destructor callback once; the
    /// socket may be gone afterwards.
    pub release: unsafe extern "C" fn(*mut c_void),
}

impl SocketOps {
    fn bound(&self, event: u32) -> bool {
        // SAFETY: a read of the live socket in `context`.
        unsafe { (self.bound)(self.context, event) }
    }

    fn set_err(&self, error: i32) {
        // SAFETY: a native effect on the live socket in `context`.
        unsafe { (self.set_err)(self.context, error) };
    }

    /// `Socket::cancel(event)`.
    fn cancel(&self, event: u32) -> bool {
        if !self.bound(event) {
            return false;
        }
        if event != self.read && event != self.write {
            self.set_err(libc::EINVAL);
            return false;
        }
        self.set_err(libc::ECANCELED);
        // SAFETY: wakes the coroutine bound to `event`, which `bound` just reported.
        unsafe { (self.resume)(self.context, event) };
        true
    }
}

/// `Socket::cancel(event)`: wake the coroutine bound to `event` with `ECANCELED`.
///
/// # Safety
/// `ops` is live for the call; its callbacks act on the socket in `context`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_cancel(ops: *const SocketOps, event: u32) -> bool {
    // SAFETY: the caller passes a live table (see above).
    unsafe { &*ops }.cancel(event)
}

/// `Socket::close()`: true once the descriptor is released, false when already
/// closed or while coroutines are still bound (close-wait).
///
/// # Safety
/// `ops` is live for the call; its callbacks act on the socket in `context`, which
/// `release` may destroy, so nothing touches it afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_close(ops: *const SocketOps) -> bool {
    // SAFETY: the caller passes a live table (see above).
    let ops = unsafe { &*ops };
    // SAFETY: a read of the live socket.
    if unsafe { (ops.closed)(ops.context) } {
        ops.set_err(libc::EBADF);
        return false;
    }
    // SAFETY: the socket's live state handle, read before any effect.
    if unsafe { (*ops.state).flags.connected } {
        // SAFETY: a native effect on the live socket.
        unsafe { (ops.shutdown)(ops.context) };
    }
    if ops.bound(ops.read | ops.write) {
        // SAFETY: a native effect on the live socket.
        unsafe { (ops.close_wait)(ops.context) };
        ops.cancel(ops.write);
        ops.cancel(ops.read);
        ops.set_err(ops.close_wait_error);
        return false;
    }
    // SAFETY: the socket's last effect here; it may destroy the socket.
    unsafe { (ops.release)(ops.context) };
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const READ: u32 = 512;
    const WRITE: u32 = 1024;

    struct Fake {
        state: *mut SocketState,
        closed: bool,
        read_bound: bool,
        write_bound: bool,
        events: Vec<String>,
    }

    impl Default for Fake {
        fn default() -> Self {
            Self {
                state: crate::shutdown::swr_v1_hook_socket_state_new(),
                closed: false,
                read_bound: false,
                write_bound: false,
                events: Vec::new(),
            }
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            unsafe { crate::shutdown::swr_v1_hook_socket_state_free(self.state) };
        }
    }

    unsafe fn fake<'a>(context: *mut c_void) -> &'a mut Fake {
        unsafe { &mut *context.cast::<Fake>() }
    }

    unsafe extern "C" fn closed(c: *mut c_void) -> bool {
        unsafe { fake(c) }.closed
    }
    unsafe extern "C" fn shutdown(c: *mut c_void) {
        let this = unsafe { fake(c) };
        unsafe { (*this.state).flags.connected = false };
        this.events.push("shutdown".into());
    }
    unsafe extern "C" fn bound(c: *mut c_void, event: u32) -> bool {
        let this = unsafe { fake(c) };
        event & READ != 0 && this.read_bound || event & WRITE != 0 && this.write_bound
    }
    unsafe extern "C" fn set_err(c: *mut c_void, error: i32) {
        unsafe { fake(c) }.events.push(format!("error:{error}"));
    }
    unsafe extern "C" fn resume(c: *mut c_void, event: u32) {
        let this = unsafe { fake(c) };
        // The resumed coroutine unbinds itself.
        if event == READ {
            this.read_bound = false;
        } else {
            this.write_bound = false;
        }
        this.events.push(format!("resume:{event}"));
    }
    unsafe extern "C" fn close_wait(c: *mut c_void) {
        unsafe { fake(c) }.events.push("close-wait".into());
    }
    unsafe extern "C" fn release(c: *mut c_void) {
        let this = unsafe { fake(c) };
        this.closed = true;
        this.events.push("release".into());
    }

    fn ops(fake: &mut Fake) -> SocketOps {
        SocketOps {
            context: (&raw mut *fake).cast(),
            state: fake.state,
            read: READ,
            write: WRITE,
            close_wait_error: 1777,
            closed,
            shutdown,
            bound,
            set_err,
            resume,
            close_wait,
            release,
        }
    }

    #[test]
    fn cancel_wakes_only_the_bound_direction() {
        let mut fake = Fake::default();
        fake.read_bound = true;
        let table = ops(&mut fake);
        assert!(!unsafe { swr_v1_hook_cancel(&raw const table, WRITE) });
        assert!(unsafe { swr_v1_hook_cancel(&raw const table, READ) });
        assert_eq!(
            fake.events,
            [
                format!("error:{}", libc::ECANCELED),
                format!("resume:{READ}")
            ]
        );
    }

    #[test]
    fn close_shuts_down_then_releases() {
        let mut fake = Fake::default();
        unsafe { crate::shutdown::swr_v1_hook_socket_connect(fake.state) };
        let table = ops(&mut fake);
        assert!(unsafe { swr_v1_hook_close(&raw const table) });
        assert!(!unsafe { swr_v1_hook_close(&raw const table) });
        assert_eq!(
            fake.events,
            [
                "shutdown".to_string(),
                "release".into(),
                format!("error:{}", libc::EBADF)
            ]
        );
    }

    #[test]
    fn close_waits_while_coroutines_are_bound() {
        let mut fake = Fake::default();
        fake.read_bound = true;
        fake.write_bound = true;
        let table = ops(&mut fake);
        assert!(!unsafe { swr_v1_hook_close(&raw const table) });
        let canceled = format!("error:{}", libc::ECANCELED);
        assert_eq!(
            fake.events,
            [
                "close-wait".to_string(),
                canceled.clone(),
                format!("resume:{WRITE}"),
                canceled,
                format!("resume:{READ}"),
                "error:1777".into()
            ]
        );
    }
}
