//! `swoole_coroutine_flock`: `flock(2)` as hooked inside a coroutine. A blocking
//! exclusive or shared lock becomes non-blocking attempts with `System::sleep` between
//! them, backing off from `SW_FILE_LOCK_DEFAULT_SECOND` to `SW_FILE_LOCK_MAX_SECOND`;
//! unlocking, `LOCK_NB` and calls outside a coroutine go straight to flock.
use crate::stdio::NATIVE;
use swoole_rs_corebase::constants::{SW_FILE_LOCK_DEFAULT_SECOND, SW_FILE_LOCK_MAX_SECOND};
use swoole_rs_corebase::set_errno;
use swoole_rs_reactor::backoff;

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
    coroutine_sleep: Option<fn(f64) -> i32>,
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
        if sleep(delay) != 0 {
            return Ok(-1);
        }
        delay = backoff(delay, SW_FILE_LOCK_DEFAULT_SECOND, SW_FILE_LOCK_MAX_SECOND);
    }
}

/// Hooked `flock(fd, operation)`. An invalid operation in a coroutine fails with
/// EINVAL as errno and the last error.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_coroutine_flock(fd: i32, operation: i32) -> i32 {
    let sleep = (NATIVE.in_coroutine)().then_some(NATIVE.sleep);
    coroutine_flock(fd, operation, sleep).unwrap_or_else(|()| {
        set_errno(libc::EINVAL);
        (NATIVE.set_last_error)(libc::EINVAL);
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
    fn release_on_second(seconds: f64) -> i32 {
        SLEPT.with_borrow_mut(|(slept, holder)| {
            slept.push(seconds);
            if slept.len() == 2 {
                unsafe { libc::flock(*holder, libc::LOCK_UN) };
            }
        });
        0
    }
    fn cancelled(_: f64) -> i32 {
        -1
    }

    fn lock(fd: i32, operation: i32, sleep: Option<fn(f64) -> i32>) -> Result<i32, ()> {
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

    thread_local! {
        /// The delays slept, the holder to unlock, and the sleep that unlocks it (`Ok`)
        /// or is cancelled (`Err`).
        static MATRIX: RefCell<(Vec<f64>, i32, Result<usize, usize>)> =
            const { RefCell::new((Vec::new(), -1, Ok(0))) };
    }
    fn matrix_sleep(seconds: f64) -> i32 {
        MATRIX.with_borrow_mut(|(slept, holder, end)| {
            slept.push(seconds);
            match *end {
                Ok(n) if slept.len() == n => {
                    unsafe { libc::flock(*holder, libc::LOCK_UN) };
                    0
                }
                Err(n) if slept.len() == n => -1,
                _ => 0,
            }
        })
    }

    /// The former hooks-ordered gate's 22 `wait_for` scenarios over upstream's retry
    /// loop, which flock ran (`src/coroutine/file_lock.cc`): 14 to 23 failed attempts
    /// before the lock frees, or a cancelled sleep after 1 to 10, with the delays
    /// doubling from 1 ms to 100 ms and starting over. Scenarios 1059 and 1060 threw
    /// from `wait_for`'s `std::function` callback; nothing takes one now.
    #[test]
    fn ported_wait_for_matrix() {
        let cycle = [0.001, 0.002, 0.004, 0.008, 0.016, 0.032, 0.064, 0.1];
        let path = std::env::temp_dir().join(format!("swr-flock-matrix-{}", std::process::id()));
        let holder = std::fs::File::create(&path).unwrap();
        let waiter = std::fs::File::open(&path).unwrap();
        let (holder, waiter) = (holder.as_raw_fd(), waiter.as_raw_fd());
        for k in 0..20 {
            let (end, result) = if k >= 10 {
                (Err(k - 10 + 1), Ok(-1))
            } else {
                (Ok(k % 10 + 14), Ok(0))
            };
            assert_eq!(lock(holder, libc::LOCK_EX, None), Ok(0));
            MATRIX.set((Vec::new(), holder, end));
            assert_eq!(lock(waiter, libc::LOCK_EX, Some(matrix_sleep)), result);
            let sleeps = match end {
                Ok(n) | Err(n) => n,
            };
            let expected: Vec<f64> = cycle.iter().copied().cycle().take(sleeps).collect();
            assert_eq!(
                MATRIX.with_borrow(|state| state.0.clone()),
                expected,
                "scenario {k}"
            );
            assert_eq!(lock(waiter, libc::LOCK_UN, None), Ok(0));
            assert_eq!(lock(holder, libc::LOCK_UN, None), Ok(0));
        }
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
