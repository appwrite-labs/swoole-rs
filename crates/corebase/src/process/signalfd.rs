//! `src/os/signal.cc`'s signalfd path (Linux, `enable_signalfd` with a reactor): the
//! blocked mask, the descriptor and the process that created it, registration and
//! reading one `signalfd_siginfo`. Native code keeps the reactor `Socket` wrapping the
//! descriptor, its event handler and callbacks, and the warnings.
use crate::process::signal::{Handler, SIGNALS, Slot, signal_reset};
use crate::sys;
use std::ffi::c_int;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering::Relaxed};

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

/// Route a guarded signal to the descriptor or the native handler.
pub(super) fn route(signo: c_int, member: bool) -> bool {
    bit(signo).is_some_and(|(word, bit)| {
        let old = if member {
            word.fetch_or(bit, Relaxed)
        } else {
            word.fetch_and(!bit, Relaxed)
        };
        old & bit != 0
    })
}

pub(super) fn update_mask() {
    let fd = signalfd_fd();
    if fd != -1 {
        _ = signalfd(fd, &sigset());
    }
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

/// `sigprocmask(how, mask, NULL)`, or the errno it failed with.
fn sigprocmask(how: c_int, mask: &libc::sigset_t) -> Result<(), i32> {
    if sys::sigprocmask(how, Some(mask), None) == -1 {
        return Err(errno());
    }
    Ok(())
}

/// `signalfd(fd, mask, FLAGS)`: a new signalfd for -1, else `fd` with the new mask;
/// or the errno it failed with.
fn signalfd(fd: c_int, mask: &libc::sigset_t) -> Result<c_int, i32> {
    let fd = sys::signalfd(fd, mask, FLAGS);
    if fd < 0 {
        return Err(errno());
    }
    Ok(fd)
}

fn clear_mask() {
    for word in &MASK {
        word.store(0, Relaxed);
    }
}

use crate::sys::errno;

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
    PID.load(Relaxed) == crate::process::os::getpid()
}

/// `swoole_signalfd_set` before the reactor setup: a null `handler` unregisters an
/// active `signo`; otherwise it is registered. The descriptor, if any, and the
/// process mask follow. Returns the previous handler.
pub fn signalfd_set(signo: c_int, handler: Handler) -> Handler {
    let handler = handler.0;
    let mut previous = 0;
    let slot = slot(signo);
    if handler == 0 && slot.is_some_and(|slot| slot.activated.load(Relaxed)) {
        route(signo, false);
        slot.into_iter().for_each(Slot::reset);
    } else {
        route(signo, true);
        if let Some(slot) = slot {
            previous = slot.handler.swap(handler, Relaxed);
            slot.signo.store(u16::try_from(signo).unwrap_or(0), Relaxed);
            slot.activated.store(true, Relaxed);
        }
    }
    let fd = FD.load(Relaxed);
    if fd != -1 {
        let mask = sigset();
        _ = sigprocmask(libc::SIG_SETMASK, &mask);
        _ = signalfd(fd, &mask);
    }
    Handler(previous)
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
    let fd = signalfd(-1, &mask).map_err(CreateError::Signalfd)?;
    if let Err(error) = sigprocmask(libc::SIG_BLOCK, &mask) {
        // `fd` was just opened here.
        _ = sys::close(fd);
        return Err(CreateError::Mask(error));
    }
    FD.store(fd, Relaxed);
    PID.store(crate::process::os::getpid(), Relaxed);
    Ok(fd)
}

/// `swoole_signalfd_close` once native has released its `Socket`: closes the descriptor.
pub fn signalfd_close() {
    let fd = FD.swap(-1, Relaxed);
    if fd != -1 {
        // `fd` was this module's signalfd.
        _ = sys::close(fd);
    }
}

/// `swoole_signalfd_clear` before the close: unblocks the mask and empties it and the
/// table. Whether the unblock succeeded.
#[must_use]
pub fn signalfd_clear() -> bool {
    let mask = sigset();
    crate::process::signal::clear_actions(|signo| {
        sys::sigismember(&mask, signo) == 0 || crate::process::signal::has_native_handler(signo)
    });
    let result = sigprocmask(libc::SIG_UNBLOCK, &mask);
    signal_reset();
    clear_mask();
    result.is_ok()
}

/// What one readable event on the descriptor asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Received {
    /// Call `handler` with `signo`.
    Handler { signo: u32, handler: Handler },
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
    if sys::read(fd, &mut info).is_none() {
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
        (true, handler) => Received::Handler {
            signo,
            handler: Handler(handler),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" fn handler(_: c_int) {}

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no sigemptyset")]
    fn registers_reads_and_clears() {
        let _state = crate::process::signal_state_lock();
        let handler = handler as *const () as usize;
        swoole_signalfd_init();
        assert_eq!(signalfd_set(libc::SIGURG, Handler(handler)), Handler::NONE);
        let fd = signalfd_create().unwrap();
        assert!(signalfd_owned());
        assert_eq!(signalfd_create(), Err(CreateError::Exists));
        // SAFETY: SIGURG is blocked, so it waits on the signalfd (and is ignored by default).
        unsafe { libc::raise(libc::SIGURG) };
        assert_eq!(
            signalfd_receive(fd),
            Received::Handler {
                signo: libc::SIGURG.cast_unsigned(),
                handler: Handler(handler)
            }
        );
        assert!(matches!(signalfd_receive(fd), Received::ReadFailed(_)));
        // A native registration made before the descriptor existed also needs clearing.
        let native = libc::SIGWINCH;
        let slot = slot(native).unwrap();
        slot.activated.store(true, Relaxed);
        slot.signo.store(u16::try_from(native).unwrap(), Relaxed);
        let previous = crate::process::signal::install(native, libc::SIG_IGN, false, false);
        assert!(signalfd_clear());
        let cleared = crate::process::signal::install(native, libc::SIG_DFL, false, false);
        assert_eq!(cleared.unwrap().sa_sigaction, libc::SIG_DFL);
        crate::process::signal::sigaction(native, previous.as_ref());

        signalfd_close();
        assert_eq!(signalfd_fd(), -1);
    }
}
