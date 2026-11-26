//! Differential tests against the pinned upstream C++ (`swoole-rs-oracle`), ported
//! from tests/modules/utilities/gate.cc with its inputs: the hashes for both `char`
//! signednesses, the logger's level clamp, labels and rotation formats,
//! and its line and microsecond formatting with truncation and errno.
use super::*;
use std::ffi::{CStr, c_char};
use swoole_rs_oracle::utilities as upstream;

/// splitmix64: the gate's random inputs without a generator dependency.
struct Random(u64);
impl Random {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    fn int(&mut self) -> i32 {
        i32::from_ne_bytes(self.next().to_ne_bytes()[..4].try_into().unwrap())
    }
    fn byte(&mut self) -> u8 {
        self.next().to_ne_bytes()[0]
    }
}

fn errno() -> *mut libc::c_int {
    #[cfg(target_os = "linux")]
    return unsafe { libc::__errno_location() };
    #[cfg(target_vendor = "apple")]
    return unsafe { libc::__error() };
}

#[test]
fn levels_labels_and_rotations() {
    let mut random = Random(982_341);
    let mut levels = vec![i32::MIN, i32::MAX, -1, 0, 1, 2, 3, 4, 5, 6, 7];
    levels.extend((0..20000).map(|_| random.int()));
    for level in levels {
        assert_eq!(log_level(level), unsafe { upstream::log_level(level) });
        let label = unsafe { CStr::from_ptr(upstream::log_label(level)) };
        assert_eq!(log_label(level), label, "label {level}");
        let rotation = unsafe { CStr::from_ptr(upstream::log_rotation(level)) };
        assert_eq!(log_rotation(level), rotation, "rotation {level}");
    }
}

/// Every length below 1024 for each `char` signedness; Austin only on ASCII, where
/// the pinned body's left shifts of a promoted negative `char` are defined.
#[test]
fn hashes() {
    let mut random = Random(982_341);
    for size in 0..1024 {
        let mut bytes: Vec<u8> = (0..size).map(|_| random.byte()).collect();
        for signed in [true, false] {
            let oracle = if signed {
                upstream::hash
            } else {
                upstream::hash_unsigned
            };
            for algorithm in [0, 1] {
                let want = unsafe { oracle(algorithm, bytes.as_ptr(), size) };
                assert_eq!(
                    swoole_hash(&bytes, algorithm, signed),
                    want,
                    "{algorithm} {bytes:?}"
                );
            }
        }
        for byte in &mut bytes {
            *byte &= 127;
        }
        let oracles: [(_, unsafe extern "C" fn(_, _, _) -> _); 2] =
            [(true, upstream::hash), (false, upstream::hash_unsigned)];
        for (signed, oracle) in oracles {
            let want = unsafe { oracle(2, bytes.as_ptr(), size) };
            assert_eq!(swoole_hash(&bytes, 2, signed), want, "austin {bytes:?}");
        }
    }
}

#[test]
fn log_lines() {
    let mut random = Random(982_341);
    for i in 0..30000usize {
        let mut date: Vec<u8> = (0..i % 128).map(|_| random.byte()).collect();
        let mut content: Vec<u8> = (0..i % 1025).map(|_| random.byte()).collect();
        let date_precision = if i % 3 == 0 {
            -1
        } else {
            i32::try_from(date.len()).unwrap()
        };
        let content_precision = if i % 4 == 0 {
            -1
        } else {
            i32::try_from(content.len()).unwrap()
        };
        let pid = if i % 7 == 0 { i32::MIN } else { random.int() };
        let worker = if i % 11 == 0 { i32::MAX } else { random.int() };
        let level = i32::try_from(i % 10).unwrap() - 1;
        let symbol = random.byte();
        let capacity = i % 257;
        let (mut expected, mut actual) = (vec![42u8; capacity + 16], vec![42u8; capacity + 16]);
        let output = if capacity == 0 {
            std::ptr::null_mut()
        } else {
            expected.as_mut_ptr()
        };
        date.push(0);
        content.push(0);
        let mut error = 0;
        let want = unsafe {
            upstream::log_line(
                output,
                capacity,
                date.as_ptr().cast(),
                date_precision,
                c_char::from_ne_bytes([symbol]),
                pid,
                worker,
                level,
                content.as_ptr().cast(),
                content_precision,
                &raw mut error,
            )
        };
        unsafe { *errno() = libc::EINTR };
        let got = log_line(
            &mut actual[..capacity],
            &date,
            symbol,
            pid,
            worker,
            level,
            &content,
        );
        assert_eq!(unsafe { *errno() }, error);
        assert_eq!(got, want);
        assert_eq!(actual, expected, "{i}");
    }
}

#[test]
fn microsecond_suffixes() {
    let mut random = Random(982_341);
    for i in 0..30000usize {
        let micros = match i {
            0 => i64::MIN,
            1 => i64::MAX,
            _ => random.next().cast_signed(),
        };
        let capacity = i % 32;
        let (mut expected, mut actual) = (vec![42u8; capacity + 16], vec![42u8; capacity + 16]);
        let output = if capacity == 0 {
            std::ptr::null_mut()
        } else {
            expected.as_mut_ptr()
        };
        let mut error = 0;
        let want = unsafe { upstream::log_micros(output, capacity, micros, &raw mut error) };
        unsafe { *errno() = libc::EINTR };
        let got = log_micros(&mut actual[..capacity], micros);
        assert_eq!(unsafe { *errno() }, error);
        assert_eq!(got, want);
        assert_eq!(actual, expected, "{micros}");
    }
}

/// Threads at one and four CPUs, then a forked process, share one lock in a shared
/// mapping; every increment under it lands.
#[test]
fn spinlock_across_threads_and_a_fork() {
    if crate::utilities::isolated_process::run_in_child(
        "utilities::upstream::spinlock_across_threads_and_a_fork",
    ) {
        return;
    }
    let page = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            4096,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED | libc::MAP_ANON,
            -1,
            0,
        )
    };
    assert_ne!(page, libc::MAP_FAILED);
    let [lock, count] = unsafe { &*page.cast::<[AtomicU32; 2]>() };
    let work = |cpus| {
        for _ in 0..5000 {
            swr_v1_utils_spinlock(Some(lock), cpus, 1024);
            count.store(count.load(Ordering::Relaxed) + 1, Ordering::Relaxed);
            lock.store(0, Ordering::SeqCst);
        }
    };
    std::thread::scope(|scope| {
        for i in 0..4 {
            scope.spawn(move || work(if i % 2 == 1 { 1 } else { 4 }));
        }
    });
    assert_eq!(count.load(Ordering::Relaxed), 20000);
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0);
    if pid == 0 {
        work(4);
        unsafe { libc::_exit(0) };
    }
    work(4);
    let mut status = 0;
    assert_eq!(unsafe { libc::waitpid(pid, &raw mut status, 0) }, pid);
    assert!(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0);
    assert_eq!(count.load(Ordering::Relaxed), 30000);
    assert_eq!(unsafe { libc::munmap(page, 4096) }, 0);
}

// Compare the Rust operations with the pinned functions' integer results.
#[cfg(target_os = "linux")]
fn wait_result(word: &AtomicU32, timeout: f64) -> i32 {
    let deadline = crate::timer::Deadline::from_futex_timeout(timeout);
    if super::misc::futex_wait(word, deadline) {
        0
    } else {
        -1
    }
}

#[cfg(target_os = "linux")]
fn wake_result(word: &AtomicU32, n: i32) -> i32 {
    super::misc::futex_wakeup(word, n).map_or(-1, |count| i32::try_from(count).unwrap_or(i32::MAX))
}

/// The former `wait.cc`'s differential: the pinned misc.cc futex wait and wake against
/// Rust's `futex_wait` and `futex_wakeup`, each from errno `EDOM` on its own copy of
/// the word, with the same result, errno and word after.
#[cfg(target_os = "linux")]
#[test]
fn futex_wait_and_wake_match_the_pin() {
    use swoole_rs_oracle::locks as pinned;
    let call = |word: u32, f: &dyn Fn(&AtomicU32) -> i32| {
        let word = AtomicU32::new(word);
        unsafe { *errno() = libc::EDOM };
        let result = f(&word);
        (result, unsafe { *errno() }, word.into_inner())
    };
    for word in [0, 1, 2, 0x7fff_ffff, 0x8000_0000, 0xffff_ffff] {
        for n in [i32::MIN, -2, -1, 0, 1, 2, i32::MAX] {
            assert_eq!(
                call(word, &|a| unsafe { pinned::wake(a.as_ptr(), n) }),
                call(word, &|a| wake_result(a, n)),
                "wake {word:#x} {n}"
            );
        }
        // Nonpositive waits on zero are indefinite; exercise only immediate paths.
        let nonpositive: &[f64] = if word == 0 { &[] } else { &[-123.0, -0.0, 0.0] };
        for &timeout in [0.000_001, 0.001, 0.0015].iter().chain(nonpositive) {
            assert_eq!(
                call(word, &|a| unsafe { pinned::wait(a.as_ptr(), timeout) }),
                call(word, &|a| wait_result(a, timeout)),
                "wait {word:#x} {timeout}"
            );
        }
    }
}

/// The former `wait.cc`'s shared processes: a forked child blocks in the futex on a shared
/// word until the parent's one wake, with the pinned wait and wake, then with Rust's.
#[cfg(target_os = "linux")]
#[test]
fn futex_wakes_a_shared_waiter() {
    use swoole_rs_oracle::locks as pinned;
    if crate::utilities::isolated_process::run_in_child(
        "utilities::upstream::futex_wakes_a_shared_waiter",
    ) {
        return;
    }
    shared_waiter(
        |a, timeout| unsafe { pinned::wait(a.as_ptr(), timeout) },
        |a, n| unsafe { pinned::wake(a.as_ptr(), n) },
    );
    shared_waiter(wait_result, wake_result);
}

#[cfg(target_os = "linux")]
fn shared_waiter(wait: impl Fn(&AtomicU32, f64) -> i32, wake: impl Fn(&AtomicU32, i32) -> i32) {
    use std::time::{Duration, Instant};
    let size = size_of::<AtomicU32>();
    let page = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED | libc::MAP_ANON,
            -1,
            0,
        )
    };
    assert_ne!(page, libc::MAP_FAILED, "shared mmap");
    let word = unsafe { &*page.cast::<AtomicU32>() };
    let mut channel = [0; 2];
    assert_eq!(unsafe { libc::pipe(channel.as_mut_ptr()) }, 0, "pipe");
    let child = unsafe { libc::fork() };
    assert!(child >= 0, "fork");
    if child == 0 {
        unsafe {
            libc::close(channel[0]);
            libc::alarm(5);
        }
        let result = wait(word, 2.0);
        let wrote = unsafe { libc::write(channel[1], (&raw const result).cast(), 4) } == 4;
        unsafe { libc::_exit(i32::from(!(wrote && result == 0))) };
    }
    unsafe { libc::close(channel[1]) };
    let deadline = Instant::now() + Duration::from_secs(3);
    let blocked = loop {
        if std::fs::read_to_string(format!("/proc/{child}/wchan"))
            .is_ok_and(|state| state.contains("futex"))
        {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    if !blocked {
        unsafe {
            libc::kill(child, libc::SIGKILL);
            libc::waitpid(child, std::ptr::null_mut(), 0);
        }
    }
    assert!(blocked, "child actually waiting before wake");
    assert_eq!(wake(word, 1), 1, "wake actual shared waiter");
    let mut result = -2;
    assert_eq!(
        unsafe { libc::read(channel[0], (&raw mut result).cast(), 4) },
        4,
        "read child result"
    );
    let mut status = 0;
    assert_eq!(unsafe { libc::waitpid(child, &raw mut status, 0) }, child);
    assert!(
        libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0 && result == 0,
        "shared child completed"
    );
    assert_eq!(word.load(Ordering::Relaxed), 0, "shared final word");
    unsafe {
        libc::close(channel[0]);
        libc::munmap(page, size);
    }
}
