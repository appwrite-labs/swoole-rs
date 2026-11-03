//! `Socket::cancel` and `Socket::close` of the coroutine socket: which bound
//! coroutine to wake, the half-close before closing, close-wait while coroutines
//! are still bound, and releasing the descriptor, over the socket's record. Native
//! code keeps the coroutines and the destructor callback, and passes them in
//! `SocketOps`.
use crate::co_socket::CoSocket;
use crate::shutdown::{READ, WRITE, swr_v1_hook_socket_bound_co};
use core::ffi::c_void;

/// The socket's effects. Every callback gets `context`.
#[repr(C)]
pub struct SocketOps {
    pub context: *mut c_void,
    pub socket: *mut CoSocket,
    /// `SW_ERROR_CO_SOCKET_CLOSE_WAIT`.
    pub close_wait_error: i32,
    /// `Socket::shutdown()` of both directions.
    pub shutdown: unsafe extern "C" fn(*mut c_void),
    pub set_err: unsafe extern "C" fn(*mut c_void, i32),
    /// Resumes the coroutine bound to `event` (read or write).
    pub resume: unsafe extern "C" fn(*mut c_void, u32),
    /// Runs the destructor callback once, after the descriptor is marked closed;
    /// the socket may be gone afterwards.
    pub release: unsafe extern "C" fn(*mut c_void),
}

impl SocketOps {
    #[expect(
        clippy::mut_from_ref,
        reason = "the table only points to the native-owned record"
    )]
    fn socket(&self) -> &mut CoSocket {
        // SAFETY: the live record the caller passes; no reference outlives a callback.
        unsafe { &mut *self.socket }
    }

    /// `has_bound(event)`.
    fn bound(&self, event: u32) -> bool {
        // SAFETY: the record's live handle.
        !unsafe { swr_v1_hook_socket_bound_co(self.socket().state, event) }.is_null()
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
        if event != READ && event != WRITE {
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
    if ops.socket().is_closed() {
        ops.set_err(libc::EBADF);
        return false;
    }
    // SAFETY: the record's live state handle, read before any effect.
    if unsafe { (*ops.socket().state).flags.connected } {
        // SAFETY: a native effect on the live socket.
        unsafe { (ops.shutdown)(ops.context) };
    }
    if ops.bound(READ | WRITE) {
        // SAFETY: an open socket's record.
        unsafe { ops.socket().record() }.output.close_wait = 1;
        ops.cancel(WRITE);
        ops.cancel(READ);
        ops.set_err(ops.close_wait_error);
        return false;
    }
    ops.socket().sock_fd = -1;
    // SAFETY: the socket's last effect here; it may destroy the socket.
    unsafe { (ops.release)(ops.context) };
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::co_socket::TestSocket;
    use crate::shutdown::swr_v1_hook_socket_bind;

    struct Fake {
        socket: TestSocket,
        events: Vec<String>,
    }

    impl Default for Fake {
        fn default() -> Self {
            Self {
                socket: TestSocket::new(),
                events: Vec::new(),
            }
        }
    }

    impl Fake {
        fn bind(&mut self, event: u32) {
            let mut co = 0u8;
            unsafe { swr_v1_hook_socket_bind(self.socket.state(), event, (&raw mut co).cast()) };
        }
    }

    unsafe fn fake<'a>(context: *mut c_void) -> &'a mut Fake {
        unsafe { &mut *context.cast::<Fake>() }
    }

    unsafe extern "C" fn shutdown(c: *mut c_void) {
        let this = unsafe { fake(c) };
        unsafe { (*this.socket.state()).flags.connected = false };
        this.events.push("shutdown".into());
    }
    unsafe extern "C" fn set_err(c: *mut c_void, error: i32) {
        unsafe { fake(c) }.events.push(format!("error:{error}"));
    }
    unsafe extern "C" fn resume(c: *mut c_void, event: u32) {
        let this = unsafe { fake(c) };
        let close_wait = unsafe { this.socket.0.record() }.output.close_wait;
        // The resumed coroutine unbinds itself.
        unsafe { swr_v1_hook_socket_bind(this.socket.state(), event, core::ptr::null_mut()) };
        this.events.push(format!("resume:{event}:{close_wait}"));
    }
    unsafe extern "C" fn release(c: *mut c_void) {
        let this = unsafe { fake(c) };
        this.events
            .push(format!("release:{}", this.socket.0.sock_fd));
    }

    fn ops(fake: &mut Fake) -> SocketOps {
        SocketOps {
            context: (&raw mut *fake).cast(),
            socket: fake.socket.ptr(),
            close_wait_error: 1777,
            shutdown,
            set_err,
            resume,
            release,
        }
    }

    #[test]
    fn cancel_wakes_only_the_bound_direction() {
        let mut fake = Fake::default();
        fake.bind(READ);
        let table = ops(&mut fake);
        assert!(!unsafe { swr_v1_hook_cancel(&raw const table, WRITE) });
        assert!(unsafe { swr_v1_hook_cancel(&raw const table, READ) });
        assert_eq!(
            fake.events,
            [
                format!("error:{}", libc::ECANCELED),
                format!("resume:{READ}:0")
            ]
        );
    }

    #[test]
    fn close_shuts_down_then_releases() {
        let mut fake = Fake::default();
        unsafe { crate::shutdown::swr_v1_hook_socket_connect(fake.socket.state()) };
        let table = ops(&mut fake);
        assert!(unsafe { swr_v1_hook_close(&raw const table) });
        assert!(!unsafe { swr_v1_hook_close(&raw const table) });
        assert_eq!(
            fake.events,
            [
                "shutdown".to_string(),
                "release:-1".into(),
                format!("error:{}", libc::EBADF)
            ]
        );
    }

    #[test]
    fn close_waits_while_coroutines_are_bound() {
        let mut fake = Fake::default();
        fake.bind(READ);
        fake.bind(WRITE);
        let table = ops(&mut fake);
        assert!(!unsafe { swr_v1_hook_close(&raw const table) });
        let canceled = format!("error:{}", libc::ECANCELED);
        assert_eq!(
            fake.events,
            [
                canceled.clone(),
                format!("resume:{WRITE}:1"),
                canceled,
                format!("resume:{READ}:1"),
                "error:1777".into()
            ]
        );
        assert_eq!(fake.socket.0.sock_fd, 100);
    }
}
