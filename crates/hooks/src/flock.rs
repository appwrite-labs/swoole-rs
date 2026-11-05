//! `swoole_coroutine_flock`: `flock(2)` as hooked inside a coroutine. A blocking
//! exclusive or shared lock becomes non-blocking attempts with `System::sleep` between
//! them, backing off from `SW_FILE_LOCK_DEFAULT_SECOND` to `SW_FILE_LOCK_MAX_SECOND`;
//! unlocking, `LOCK_NB` and calls outside a coroutine go straight to flock.
use crate::sleep::CoroutineSleep;
use crate::swr_v1_hook_backoff;
use swoole_rs_corebase::constants::{SW_FILE_LOCK_DEFAULT_SECOND, SW_FILE_LOCK_MAX_SECOND};

fn flock(fd: i32, operation: i32) -> i32 {
    // SAFETY: flock has no memory preconditions.
    unsafe { libc::flock(fd, operation) }
}

/// Locks `fd` like `flock(fd, operation)`, yielding through `coroutine_sleep` (none
/// outside a coroutine) while another holder has it. `Err` for an operation that is
/// none of `LOCK_EX`, `LOCK_SH`, `LOCK_UN`.
fn coroutine_flock(
    fd: i32,
    operation: i32,
    coroutine_sleep: Option<CoroutineSleep>,
) -> Result<i32, ()> {
    let Some(sleep) = coroutine_sleep.filter(|_| operation & libc::LOCK_NB == 0) else {
        return Ok(flock(fd, operation));
    };
    if operation == libc::LOCK_UN {
        return Ok(flock(fd, libc::LOCK_UN));
    }
    if operation != libc::LOCK_EX && operation != libc::LOCK_SH {
        return Err(());
    }
    let mut delay = SW_FILE_LOCK_DEFAULT_SECOND;
    loop {
        match flock(fd, operation | libc::LOCK_NB) {
            0 => return Ok(0),
            -1 if std::io::Error::last_os_error().raw_os_error() == Some(libc::EWOULDBLOCK) => {}
            _ => return Ok(-1),
        }
        // SAFETY: native passes System::sleep, valid inside the current coroutine.
        if unsafe { sleep(delay) } != 0 {
            return Ok(-1);
        }
        delay = swr_v1_hook_backoff(delay, SW_FILE_LOCK_DEFAULT_SECOND, SW_FILE_LOCK_MAX_SECOND);
    }
}

fn set_errno(value: i32) {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    // SAFETY: returns this thread's errno slot, live for the thread.
    let slot = unsafe { libc::__errno_location() };
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    // SAFETY: returns this thread's errno slot, live for the thread.
    let slot = unsafe { libc::__error() };
    // SAFETY: `slot` is this thread's errno.
    unsafe { slot.write(value) };
}

/// Hooked `flock(fd, operation)`. An invalid operation in a coroutine fails with
/// EINVAL as errno and the last error.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_coroutine_flock(fd: i32, operation: i32) -> i32 {
    let host = crate::stdio::host();
    // SAFETY: native reads its reactor and coroutine.
    let sleep = host
        .filter(|host| unsafe { (host.in_coroutine)() })
        .map(|host| host.sleep);
    coroutine_flock(fd, operation, sleep).unwrap_or_else(|()| {
        set_errno(libc::EINVAL);
        if let Some(host) = host {
            // SAFETY: native's last-error setter.
            unsafe { (host.set_last_error)(libc::EINVAL) };
        }
        -1
    })
}

#[cfg(test)]
mod tests {
    use super::coroutine_flock;
    use std::cell::RefCell;
    use std::os::fd::AsRawFd;

    thread_local! {
        /// The delays slept, and the descriptor to unlock on the second sleep.
        static SLEPT: RefCell<(Vec<f64>, i32)> = const { RefCell::new((Vec::new(), -1)) };
    }
    unsafe extern "C" fn release_on_second(seconds: f64) -> i32 {
        SLEPT.with_borrow_mut(|(slept, holder)| {
            slept.push(seconds);
            if slept.len() == 2 {
                unsafe { libc::flock(*holder, libc::LOCK_UN) };
            }
        });
        0
    }
    unsafe extern "C" fn cancelled(_: f64) -> i32 {
        -1
    }

    fn lock(fd: i32, operation: i32, sleep: Option<super::CoroutineSleep>) -> Result<i32, ()> {
        coroutine_flock(fd, operation, sleep)
    }

    #[test]
    fn waits_for_the_holder_backing_off() {
        let path = std::env::temp_dir().join(format!("swr-flock-{}", std::process::id()));
        let holder = std::fs::File::create(&path).unwrap();
        let waiter = std::fs::File::open(&path).unwrap();
        let (holder, waiter) = (holder.as_raw_fd(), waiter.as_raw_fd());
        let ok = Ok(0);

        assert_eq!(lock(holder, libc::LOCK_EX, Some(release_on_second)), ok);
        SLEPT.with_borrow_mut(|state| state.1 = holder);
        assert_eq!(lock(waiter, libc::LOCK_SH, Some(release_on_second)), ok);
        assert_eq!(SLEPT.with_borrow(|state| state.0.clone()), [0.001, 0.002]);

        assert_eq!(
            lock(holder, libc::LOCK_EX | libc::LOCK_NB, Some(cancelled)),
            Ok(-1)
        );
        assert_eq!(lock(holder, libc::LOCK_EX, Some(cancelled)), Ok(-1));
        assert_eq!(lock(waiter, libc::LOCK_UN, Some(cancelled)), ok);
        assert_eq!(lock(holder, libc::LOCK_EX, Some(cancelled)), ok);
        assert_eq!(lock(holder, libc::LOCK_UN, None), ok);

        assert_eq!(
            lock(holder, libc::LOCK_EX | libc::LOCK_SH, Some(cancelled)),
            Err(())
        );
        assert_eq!(lock(-1, libc::LOCK_EX, Some(cancelled)), Ok(-1));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn the_export_raises_einval_in_a_coroutine() {
        use crate::stdio::tests::{LAST_ERROR, Mode, with};
        let invalid = libc::LOCK_EX | libc::LOCK_SH;
        let raised = with(Mode::Pool, || super::swoole_coroutine_flock(-1, invalid));
        let errno = std::io::Error::last_os_error().raw_os_error();
        assert_eq!(
            (raised, errno, LAST_ERROR.get()),
            (-1, Some(libc::EINVAL), libc::EINVAL)
        );
        // Outside one it is flock's own answer.
        assert_eq!(
            with(Mode::Here, || super::swoole_coroutine_flock(
                -1,
                libc::LOCK_EX
            )),
            -1
        );
        let errno = std::io::Error::last_os_error().raw_os_error();
        assert_eq!(errno, Some(libc::EBADF));
    }
}
