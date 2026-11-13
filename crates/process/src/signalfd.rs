//! `src/os/signal.cc`'s signalfd path (Linux, `enable_signalfd` with a reactor): the
//! blocked mask, the descriptor and the process that created it, registration and
//! reading one `signalfd_siginfo`. Native code keeps the reactor `Socket` wrapping the
//! descriptor, its event handler and callbacks, and the warnings.
use crate::signal::{SIGNALS, Slot, signal_reset};
use std::ffi::c_int;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering::Relaxed};
use swoole_rs_corebase::sys;

const _: () = assert!(
    std::mem::offset_of!(libc::signalfd_siginfo, ssi_signo) == 0,
    "signalfd_receive reads ssi_signo from the record's first bytes"
);

const SIG_IGN: usize = 1;
const FLAGS: c_int = libc::SFD_NONBLOCK | libc::SFD_CLOEXEC;

/// The signals routed to the descriptor (upstream's `signalfd_mask`), one bit each.
static MASK: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
static FD: AtomicI32 = AtomicI32::new(-1);
/// The process that created `FD` (`signalfd_create_pid`).
static PID: AtomicI32 = AtomicI32::new(0);

fn bit(signo: c_int) -> Option<(&'static AtomicU64, u64)> {
    let signo = usize::try_from(signo).ok()?;
    Some((MASK.get(signo / 64)?, 1 << (signo % 64)))
}

/// `MASK` as a `sigset_t`.
fn sigset() -> libc::sigset_t {
    let mut set: libc::sigset_t = sys::zeroed();
    sys::sigemptyset(&mut set);
    for signo in 0..128 {
        if bit(signo).is_some_and(|(word, bit)| word.load(Relaxed) & bit != 0) {
            sys::sigaddset(&mut set, signo);
        }
    }
    set
}

/// `sigprocmask(how, mask, NULL)`.
fn sigprocmask(how: c_int, mask: &libc::sigset_t) -> c_int {
    sys::sigprocmask(how, Some(mask), None)
}

/// `signalfd(fd, mask, FLAGS)`: a new signalfd for -1, else `fd` with the new mask.
fn signalfd(fd: c_int, mask: &libc::sigset_t) -> c_int {
    sys::signalfd(fd, mask, FLAGS)
}

fn clear_mask() {
    for word in &MASK {
        word.store(0, Relaxed);
    }
}

fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

fn slot(signo: c_int) -> Option<&'static Slot> {
    SIGNALS.get(usize::try_from(signo).ok()?)
}

/// `void swoole_signalfd_init()`: an empty mask and table.
pub fn swoole_signalfd_init() {
    clear_mask();
    signal_reset();
}

/// The signalfd descriptor, or -1 (`swoole_signalfd_is_available`).
pub fn signalfd_fd() -> c_int {
    FD.load(Relaxed)
}

/// Whether this process created the descriptor (a forked child must not remove it
/// from the parent's reactor).
pub fn signalfd_owned() -> bool {
    PID.load(Relaxed) == crate::os::getpid()
}

/// `swoole_signalfd_set` before the reactor setup: a null `handler` unregisters an
/// active `signo`; otherwise it is registered. The descriptor, if any, and the
/// process mask follow. Returns the previous handler.
pub fn signalfd_set(signo: c_int, handler: usize) -> usize {
    let mut previous = 0;
    let slot = slot(signo);
    if handler == 0 && slot.is_some_and(|slot| slot.activated.load(Relaxed)) {
        if let Some((word, bit)) = bit(signo) {
            word.fetch_and(!bit, Relaxed);
        }
        slot.into_iter().for_each(Slot::reset);
    } else {
        if let Some((word, bit)) = bit(signo) {
            word.fetch_or(bit, Relaxed);
        }
        if let Some(slot) = slot {
            previous = slot.handler.swap(handler, Relaxed);
            slot.signo.store(u16::try_from(signo).unwrap_or(0), Relaxed);
            slot.activated.store(true, Relaxed);
        }
    }
    let fd = FD.load(Relaxed);
    if fd != -1 {
        let mask = sigset();
        sigprocmask(libc::SIG_SETMASK, &mask);
        signalfd(fd, &mask);
    }
    previous
}

/// Why creating the descriptor failed, with the errno where there is one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreateError {
    /// One already exists.
    Exists,
    /// signalfd failed.
    Signalfd(i32),
    /// Blocking the mask failed; the descriptor is closed again.
    Mask(i32),
}

/// `swoole_signalfd_create`: a non-blocking signalfd for the mask, which is then
/// blocked for the process.
///
/// # Errors
/// The [`CreateError`] when there is one already or a call failed.
pub fn signalfd_create() -> Result<c_int, CreateError> {
    if FD.load(Relaxed) != -1 {
        return Err(CreateError::Exists);
    }
    let mask = sigset();
    let fd = signalfd(-1, &mask);
    if fd < 0 {
        return Err(CreateError::Signalfd(errno()));
    }
    if sigprocmask(libc::SIG_BLOCK, &mask) == -1 {
        let error = errno();
        // `fd` was just opened here.
        sys::close(fd);
        return Err(CreateError::Mask(error));
    }
    FD.store(fd, Relaxed);
    PID.store(crate::os::getpid(), Relaxed);
    Ok(fd)
}

/// `swoole_signalfd_close` once native has released its `Socket`: closes the descriptor.
pub fn signalfd_close() {
    let fd = FD.swap(-1, Relaxed);
    if fd != -1 {
        // `fd` was this module's signalfd.
        sys::close(fd);
    }
}

/// `swoole_signalfd_clear` before the close: unblocks the mask and empties it and the
/// table. Whether the unblock succeeded.
#[must_use]
pub fn signalfd_clear() -> bool {
    let mask = sigset();
    let result = sigprocmask(libc::SIG_UNBLOCK, &mask);
    signal_reset();
    clear_mask();
    result >= 0
}

/// What one readable event on the descriptor asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Received {
    /// Call `handler` with `signo`.
    Handler { signo: u32, handler: usize },
    /// The read failed with this errno.
    ReadFailed(i32),
    /// The signal number is out of range.
    OutOfRange(u32),
    /// Nothing to do: not registered, or `SIG_IGN`.
    Nothing,
    /// Registered without a handler.
    NoHandler(u32),
}

/// `swoole_signalfd_event_callback`: reads one `signalfd_siginfo` from `fd`.
pub fn signalfd_receive(fd: c_int) -> Received {
    // One `signalfd_siginfo`, which begins with its `ssi_signo`.
    let mut info = [0_u8; size_of::<libc::signalfd_siginfo>()];
    if sys::read(fd, &mut info) < 0 {
        return Received::ReadFailed(errno());
    }
    let [a, b, c, d, ..] = info;
    let signo = u32::from_ne_bytes([a, b, c, d]);
    let Some(slot) = c_int::try_from(signo).ok().and_then(slot) else {
        return Received::OutOfRange(signo);
    };
    match (slot.activated.load(Relaxed), slot.handler.load(Relaxed)) {
        (false, _) | (true, SIG_IGN) => Received::Nothing,
        (true, 0) => Received::NoHandler(signo),
        (true, handler) => Received::Handler { signo, handler },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" fn handler(_: c_int) {}

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no sigemptyset")]
    fn registers_reads_and_clears() {
        let _state = crate::signal_state_lock();
        let handler = handler as *const () as usize;
        swoole_signalfd_init();
        assert_eq!(signalfd_set(libc::SIGURG, handler), 0);
        let fd = signalfd_create().unwrap();
        assert!(signalfd_owned());
        assert_eq!(signalfd_create(), Err(CreateError::Exists));
        // SAFETY: SIGURG is blocked, so it waits on the signalfd (and is ignored by default).
        unsafe { libc::raise(libc::SIGURG) };
        assert_eq!(
            signalfd_receive(fd),
            Received::Handler {
                signo: libc::SIGURG.cast_unsigned(),
                handler
            }
        );
        assert!(matches!(signalfd_receive(fd), Received::ReadFailed(_)));
        assert!(signalfd_clear());
        signalfd_close();
        assert_eq!(signalfd_fd(), -1);
    }
}
