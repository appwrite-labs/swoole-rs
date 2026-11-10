//! `src/os/signal.cc`'s signalfd path (Linux, `enable_signalfd` with a reactor): the
//! blocked mask, the descriptor and the process that created it, registration and
//! reading one `signalfd_siginfo`. Native code keeps the reactor `Socket` wrapping the
//! descriptor, its event handler and callbacks, and the warnings.
use crate::signal::{SIGNALS, Slot, signal_reset};
use std::ffi::c_int;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering::Relaxed};

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
    // SAFETY: sigset_t is plain data.
    let mut set: libc::sigset_t = unsafe { std::mem::zeroed() };
    // SAFETY: `set` is a valid sigset_t.
    unsafe { libc::sigemptyset(&raw mut set) };
    for signo in 0..128 {
        if bit(signo).is_some_and(|(word, bit)| word.load(Relaxed) & bit != 0) {
            // SAFETY: as above.
            unsafe { libc::sigaddset(&raw mut set, signo) };
        }
    }
    set
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
#[unsafe(export_name = "_Z20swoole_signalfd_initv")]
pub extern "C" fn swoole_signalfd_init() {
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
        // SAFETY: `mask` is initialized; the old mask is not wanted.
        unsafe { libc::sigprocmask(libc::SIG_SETMASK, &raw const mask, std::ptr::null_mut()) };
        // SAFETY: `fd` is this module's signalfd and `mask` is initialized.
        unsafe { libc::signalfd(fd, &raw const mask, FLAGS) };
    }
    previous
}

/// What creating the descriptor did.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Created {
    /// 0 created `fd`; 1 one already exists; 2 signalfd failed; 3 blocking the mask
    /// failed (the descriptor is closed again).
    pub status: u32,
    pub fd: c_int,
    /// The errno of status 2 and 3.
    pub error: i32,
}

/// `swoole_signalfd_create`: a non-blocking signalfd for the mask, which is then
/// blocked for the process.
pub fn signalfd_create() -> Created {
    let existing = FD.load(Relaxed);
    if existing != -1 {
        return Created {
            status: 1,
            fd: existing,
            error: 0,
        };
    }
    let mask = sigset();
    // SAFETY: `mask` is initialized.
    let fd = unsafe { libc::signalfd(-1, &raw const mask, FLAGS) };
    if fd < 0 {
        return Created {
            status: 2,
            fd,
            error: errno(),
        };
    }
    // SAFETY: as above.
    if unsafe { libc::sigprocmask(libc::SIG_BLOCK, &raw const mask, std::ptr::null_mut()) } == -1 {
        let error = errno();
        // SAFETY: `fd` was just opened here.
        unsafe { libc::close(fd) };
        return Created {
            status: 3,
            fd,
            error,
        };
    }
    FD.store(fd, Relaxed);
    PID.store(crate::os::getpid(), Relaxed);
    Created {
        status: 0,
        fd,
        error: 0,
    }
}

/// `swoole_signalfd_close` once native has released its `Socket`: closes the descriptor.
pub fn signalfd_close() {
    let fd = FD.swap(-1, Relaxed);
    if fd != -1 {
        // SAFETY: `fd` was this module's signalfd.
        unsafe { libc::close(fd) };
    }
}

/// `swoole_signalfd_clear` before the close: unblocks the mask and empties it and the
/// table. Whether the unblock succeeded.
#[must_use]
pub fn signalfd_clear() -> bool {
    let mask = sigset();
    // SAFETY: `mask` is initialized.
    let result =
        unsafe { libc::sigprocmask(libc::SIG_UNBLOCK, &raw const mask, std::ptr::null_mut()) };
    signal_reset();
    clear_mask();
    result >= 0
}

/// What one readable event on the descriptor asks for.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Received {
    /// 0 call `handler`; 1 the read failed; 2 the signal number is out of range;
    /// 3 nothing to do (not registered, or `SIG_IGN`); 4 registered without a handler.
    pub status: u32,
    pub signo: u32,
    pub handler: usize,
    /// The errno of status 1.
    pub error: i32,
}

/// `swoole_signalfd_event_callback`: reads one `signalfd_siginfo` from `fd`.
pub fn signalfd_receive(fd: c_int) -> Received {
    // SAFETY: signalfd_siginfo is plain data.
    let mut info: libc::signalfd_siginfo = unsafe { std::mem::zeroed() };
    let size = size_of::<libc::signalfd_siginfo>();
    // SAFETY: `info` is writable for `size` bytes.
    if unsafe { libc::read(fd, (&raw mut info).cast(), size) } < 0 {
        return Received {
            status: 1,
            error: errno(),
            ..Received::default()
        };
    }
    let signo = info.ssi_signo;
    let Some(slot) = c_int::try_from(signo).ok().and_then(slot) else {
        return Received {
            status: 2,
            signo,
            ..Received::default()
        };
    };
    let handler = slot.handler.load(Relaxed);
    let status = match (slot.activated.load(Relaxed), handler) {
        (false, _) | (true, SIG_IGN) => 3,
        (true, 0) => 4,
        _ => 0,
    };
    Received {
        status,
        signo,
        handler,
        error: 0,
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
        let created = signalfd_create();
        assert_eq!(created.status, 0);
        assert!(signalfd_owned());
        assert_eq!(signalfd_create().status, 1);
        // SAFETY: SIGURG is blocked, so it waits on the signalfd (and is ignored by default).
        unsafe { libc::raise(libc::SIGURG) };
        let received = signalfd_receive(created.fd);
        assert_eq!((received.status, received.handler), (0, handler));
        assert_eq!(received.signo, libc::SIGURG.cast_unsigned());
        assert_eq!(signalfd_receive(created.fd).status, 1);
        assert!(signalfd_clear());
        signalfd_close();
        assert_eq!(signalfd_fd(), -1);
    }
}
