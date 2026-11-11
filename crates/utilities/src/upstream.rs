//! Differential tests against the pinned upstream C++ (`swoole-rs-oracle`), ported
//! from tests/modules/utilities/gate.cc with its inputs: the hashes for both `char`
//! signednesses, pretty names, the logger's level clamp, labels and rotation formats,
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
    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % n as u64).unwrap()
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
        for (signed, oracle) in [
            (true, upstream::hash as unsafe extern "C" fn(_, _, _) -> _),
            (false, upstream::hash_unsigned),
        ] {
            let want = unsafe { oracle(2, bytes.as_ptr(), size) };
            assert_eq!(swoole_hash(&bytes, 2, signed), want, "austin {bytes:?}");
        }
    }
}

#[test]
fn pretty_names() {
    let mut random = Random(982_341);
    let mut names: Vec<Vec<u8>> = [
        "void ns::Class::method(int)",
        "int x()",
        "::f()",
        "aa::bb()",
        "no bracket",
        "()",
        "a::::b()",
    ]
    .map(|name| name.as_bytes().to_vec())
    .into();
    for colons in 0..65 {
        for prefix in 0..8 {
            let (a, c) = ("a".repeat(prefix), ":".repeat(colons));
            names.push(format!("{a}{c}f()").into_bytes());
            names.push(format!("void {a}{c}f(int)").into_bytes());
        }
    }
    for _ in 0..20000 {
        let length = random.below(80);
        names.push((0..length).map(|_| b"ab :()_\0"[random.below(8)]).collect());
    }
    for name in &names {
        for strip in [false, true] {
            let mut want = vec![0u8; name.len()];
            let length =
                unsafe { upstream::pretty(name.as_ptr(), name.len(), strip, want.as_mut_ptr()) };
            let (begin, count) = pretty_range(name, strip);
            assert_eq!(
                &name[begin..begin + count],
                &want[..length],
                "{name:?} {strip}"
            );
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
    if crate::isolated_process::run_in_child("upstream::spinlock_across_threads_and_a_fork") {
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
