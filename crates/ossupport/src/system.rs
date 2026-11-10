// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/os/base.cc (Swoole contributors).
//! Upstream `src/os/base.cc`, under its C++ symbols with `upstream-symbols`: the
//! root check, daemonizing, user/group/chroot isolation, parent-death signal, CPU
//! affinity, native thread id and thread names. Logs name the function as
//! upstream's `__SW_FUNC__` did.
use libc::{c_char, c_int, c_long};
use std::ffi::CStr;
use swoole_rs_corebase::log;

#[must_use]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z19swoole_is_root_userv")
)]
pub extern "C" fn swr_v1_os_is_root_user() -> bool {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() == 0 }
}

/// `swoole_daemon`'s Darwin steps before the fork: `chdir("/")` unless `nochdir`,
/// then stdin/stdout/stderr onto `/dev/null` unless `noclose`. Logs the failed step.
#[cfg(all(feature = "upstream-symbols", target_vendor = "apple"))]
fn daemon_detach(nochdir: bool, noclose: bool) -> bool {
    use swoole_rs_corebase::sys;
    // SAFETY: a static C string.
    if !nochdir && unsafe { libc::chdir(c"/".as_ptr()) } != 0 {
        log::sys_warning(c"swoole_daemon", format_args!("chdir() failed"));
        return false;
    }
    if noclose {
        return true;
    }
    let fd = sys::open(c"/dev/null", libc::O_RDWR, 0);
    if fd < 0 {
        log::sys_warning(c"swoole_daemon", format_args!("open() failed"));
        return false;
    }
    // dup2 onto the standard descriptors, as upstream.
    let redirected = (0..3).all(|target| sys::dup2(fd, target) >= 0);
    if !redirected {
        log::sys_warning(c"swoole_daemon", format_args!("dup2() failed"));
    }
    sys::close(fd);
    redirected
}

/// `SW_FORK_DAEMON`, `SW_FORK_PRECHECK` and `SW_GLOBAL_HOOK_AFTER_FORK`.
#[cfg(all(feature = "upstream-symbols", target_vendor = "apple"))]
const FORK_DAEMON: c_int = 4;
#[cfg(all(feature = "upstream-symbols", not(target_vendor = "apple")))]
const FORK_PRECHECK: c_int = 8;
#[cfg(all(feature = "upstream-symbols", not(target_vendor = "apple")))]
const HOOK_AFTER_FORK: c_int = 12;

/// `int swoole_daemon(int nochdir, int noclose)`. Darwin's `daemon(3)` is
/// deprecated, so it detaches, forks and calls `setsid` itself.
#[cfg(all(feature = "upstream-symbols", target_vendor = "apple"))]
#[unsafe(export_name = "_Z13swoole_daemonii")]
#[must_use]
pub extern "C" fn swoole_daemon(nochdir: c_int, noclose: c_int) -> c_int {
    if !daemon_detach(nochdir != 0, noclose != 0) {
        return -1;
    }
    // Upstream's fork; it rejects a coroutine or async-thread caller itself.
    let pid = swoole_rs_corebase::swoole_fork(FORK_DAEMON);
    if pid < 0 {
        log::sys_warning(c"swoole_daemon", format_args!("fork() failed"));
        return -1;
    }
    if pid > 0 {
        // SAFETY: the parent leaves without running exit handlers, as upstream.
        unsafe { libc::_exit(0) };
    }
    // SAFETY: setsid has no memory preconditions.
    if unsafe { libc::setsid() } < 0 {
        log::sys_warning(c"swoole_daemon", format_args!("setsid() failed"));
        return -1;
    }
    0
}

/// `int swoole_daemon(int nochdir, int noclose)`: `daemon(3)`, then the
/// after-fork hooks, since the pid changed (PHP 8.5 refreshes its memory
/// manager after a fork, and `daemon(3)` forks behind its back).
#[cfg(all(feature = "upstream-symbols", not(target_vendor = "apple")))]
#[unsafe(export_name = "_Z13swoole_daemonii")]
#[must_use]
pub extern "C" fn swoole_daemon(nochdir: c_int, noclose: c_int) -> c_int {
    // Upstream's fork precheck; it forks nothing (-1 before `swoole_init`).
    if swoole_rs_corebase::swoole_fork(FORK_PRECHECK) < 0 {
        return -1;
    }
    // SAFETY: daemon has no memory preconditions.
    let rv = unsafe { libc::daemon(nochdir, noclose) };
    if rv == 0 {
        // SAFETY: the after-fork hooks take no argument.
        unsafe { swoole_rs_corebase::swoole_call_hook(HOOK_AFTER_FORK, std::ptr::null_mut()) };
    }
    rv
}

/// `swoole_set_isolation`: look up the group and user, switch gid then uid, then
/// chroot and `chdir("/")`; empty names skip their step. `warn` hears each failed
/// step as it happens (errno intact): 1 group lookup, 2 user lookup, 3 setgid,
/// 4 setuid, 5 `chdir("/")`, 6 chroot.
fn isolate(group: &CStr, user: &CStr, root: &CStr, mut warn: impl FnMut(u32)) {
    let mut gid = None;
    if !group.is_empty() {
        // SAFETY: a valid C string; the entry is read before the next lookup.
        let entry = unsafe { libc::getgrnam(group.as_ptr()) };
        // SAFETY: getgrnam returned null or a valid entry.
        gid = unsafe { entry.as_ref() }.map(|entry| entry.gr_gid);
        if gid.is_none() {
            warn(1);
        }
    }
    let mut uid = None;
    if !user.is_empty() {
        // SAFETY: a valid C string; the entry is read before the next lookup.
        let entry = unsafe { libc::getpwnam(user.as_ptr()) };
        // SAFETY: getpwnam returned null or a valid entry.
        uid = unsafe { entry.as_ref() }.map(|entry| entry.pw_uid);
        if uid.is_none() {
            warn(2);
        }
    }
    // SAFETY: setgid has no memory preconditions.
    if gid.is_some_and(|gid| unsafe { libc::setgid(gid) } < 0) {
        warn(3);
    }
    // SAFETY: setuid has no memory preconditions.
    if uid.is_some_and(|uid| unsafe { libc::setuid(uid) } < 0) {
        warn(4);
    }
    if !root.is_empty() {
        // SAFETY: a valid C string.
        if unsafe { libc::chroot(root.as_ptr()) } == 0 {
            // SAFETY: a static C string.
            if unsafe { libc::chdir(c"/".as_ptr()) } < 0 {
                warn(5);
            }
        } else {
            warn(6);
        }
    }
}

/// `swoole_set_isolation(group, user, chroot)`, which the header calls with each
/// `std::string`'s `c_str()`.
/// # Safety
/// `group`, `user` and `root` are valid NUL-terminated strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_set_isolation(
    group: *const c_char,
    user: *const c_char,
    root: *const c_char,
) {
    // SAFETY: valid C strings per the contract.
    let group = unsafe { CStr::from_ptr(group) };
    // SAFETY: as above.
    let user = unsafe { CStr::from_ptr(user) };
    // SAFETY: as above.
    let root = unsafe { CStr::from_ptr(root) };
    let origin = c"swoole_set_isolation";
    let (g, u, r) = (
        group.to_string_lossy(),
        user.to_string_lossy(),
        root.to_string_lossy(),
    );
    isolate(group, user, root, |step| match step {
        1 => log::warning(origin, format_args!("get group [{g}] info failed")),
        2 => log::warning(origin, format_args!("get user [{u}] info failed")),
        3 => log::sys_warning(origin, format_args!("setgid to [{g}] failed")),
        4 => log::sys_warning(origin, format_args!("setuid to [{u}] failed")),
        5 => log::sys_warning(origin, format_args!("chdir('/') failed")),
        _ => log::sys_warning(origin, format_args!("chroot('{r}') failed")),
    });
}

/// `swoole_set_process_death_signal`: Linux `PR_SET_PDEATHSIG`, FreeBSD
/// `PROC_PDEATHSIG_CTL`, a no-op elsewhere.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z31swoole_set_process_death_signali")
)]
pub extern "C" fn swr_v1_os_set_process_death_signal(signal: c_int) {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    // SAFETY: PR_SET_PDEATHSIG takes the signal number by value.
    unsafe {
        libc::prctl(libc::PR_SET_PDEATHSIG, signal.unsigned_abs());
    }
    #[cfg(target_os = "freebsd")]
    {
        let mut signal = signal;
        // SAFETY: PROC_PDEATHSIG_CTL reads one int through the pointer.
        unsafe {
            libc::procctl(
                libc::P_PID,
                0,
                libc::PROC_PDEATHSIG_CTL,
                (&raw mut signal).cast(),
            );
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "android", target_os = "freebsd")))]
    let _ = signal;
}

#[cfg(target_os = "freebsd")]
type CpuSet = libc::cpuset_t;
#[cfg(any(target_os = "linux", target_os = "android"))]
type CpuSet = libc::cpu_set_t;

/// `int swoole_get_cpu_affinity(cpu_set_t *set)`: the whole process.
/// # Safety
/// `set` points to a writable `cpu_set_t`.
#[cfg(any(target_os = "linux", target_os = "android", target_os = "freebsd"))]
#[cfg_attr(
    all(feature = "upstream-symbols", target_os = "freebsd"),
    unsafe(export_name = "_Z23swoole_get_cpu_affinityP7_cpuset")
)]
#[cfg_attr(
    all(feature = "upstream-symbols", not(target_os = "freebsd")),
    unsafe(export_name = "_Z23swoole_get_cpu_affinityP9cpu_set_t")
)]
pub unsafe extern "C" fn swoole_get_cpu_affinity(set: *mut CpuSet) -> c_int {
    #[cfg(target_os = "freebsd")]
    // SAFETY: `set` is a writable `cpu_set_t` per the contract.
    unsafe {
        libc::cpuset_getaffinity(
            libc::CPU_LEVEL_WHICH,
            libc::CPU_WHICH_PID,
            -1,
            size_of::<CpuSet>(),
            set,
        )
    }
    #[cfg(not(target_os = "freebsd"))]
    {
        // Upstream pins the process's main thread (tid == pid), not the calling thread.
        let pid = swoole_rs_corebase::sys::getpid();
        // SAFETY: `set` is a writable `cpu_set_t` per the contract.
        unsafe { libc::sched_getaffinity(pid, size_of::<CpuSet>(), set) }
    }
}

/// `swoole_thread_get_native_id`: the kernel's id for the calling thread.
#[must_use]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z27swoole_thread_get_native_idv")
)]
pub extern "C" fn swr_v1_os_thread_native_id() -> c_long {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        // SAFETY: SYS_gettid takes no arguments.
        unsafe { libc::syscall(libc::SYS_gettid) }
    }
    #[cfg(target_vendor = "apple")]
    {
        let mut id = 0_u64;
        // SAFETY: a null thread means the caller; `id` is a live out-parameter.
        unsafe { libc::pthread_threadid_np(0, &raw mut id) };
        id.cast_signed()
    }
    #[cfg(target_os = "freebsd")]
    {
        // SAFETY: no preconditions.
        c_long::from(unsafe { libc::pthread_getthreadid_np() })
    }
    #[cfg(target_os = "openbsd")]
    {
        // SAFETY: no preconditions.
        c_long::from(unsafe { libc::getthrid() })
    }
    #[cfg(target_os = "netbsd")]
    {
        // SAFETY: no preconditions.
        c_long::from(unsafe { libc::_lwp_self() })
    }
}

/// `swoole_thread_set_name` for the calling thread; returns the pthread error.
/// # Safety
/// `name` is a valid NUL-terminated string.
#[must_use]
pub unsafe extern "C" fn swr_v1_os_thread_set_name(name: *const c_char) -> c_int {
    #[cfg(target_vendor = "apple")]
    {
        // SAFETY: valid C string per the contract.
        unsafe { libc::pthread_setname_np(name) }
    }
    #[cfg(not(target_vendor = "apple"))]
    {
        // SAFETY: pthread_self has no preconditions.
        let thread = unsafe { libc::pthread_self() };
        // SAFETY: valid C string per the contract; the calling thread is live.
        unsafe { libc::pthread_setname_np(thread, name) }
    }
}

/// `swoole_thread_get_name` for the calling thread; returns the pthread error.
/// # Safety
/// `buf` points to `len` writable bytes.
pub unsafe extern "C" fn swr_v1_os_thread_get_name(buf: *mut c_char, len: usize) -> c_int {
    #[cfg(target_os = "android")]
    {
        // Upstream's Android shim: bionic lacks pthread_getname_np before API 26.
        // SAFETY: pthread_self has no preconditions.
        let name = format!("thread-{}", unsafe { libc::pthread_self() });
        let copied = name.len().min(len.saturating_sub(1));
        if len > 0 {
            // SAFETY: `buf` has `len` writable bytes and `copied < len`.
            unsafe { std::ptr::copy_nonoverlapping(name.as_ptr().cast(), buf, copied) };
            // SAFETY: `copied < len`.
            unsafe { buf.add(copied).write(0) };
        }
        0
    }
    #[cfg(not(target_os = "android"))]
    {
        // SAFETY: pthread_self has no preconditions.
        let thread = unsafe { libc::pthread_self() };
        // SAFETY: `buf` has `len` writable bytes per the contract.
        unsafe { libc::pthread_getname_np(thread, buf, len) }
    }
}

/// `check_pthread_return_value`: a pthread error becomes the last error.
fn pthread_ok(rc: c_int) -> bool {
    if rc != 0 {
        swoole_rs_corebase::log::set_last_error(rc);
    }
    rc == 0
}

/// `bool swoole_thread_set_name(const char *name)`.
/// # Safety
/// `name` is a valid NUL-terminated string.
#[must_use]
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z22swoole_thread_set_namePKc")
)]
pub unsafe extern "C" fn swoole_thread_set_name(name: *const c_char) -> bool {
    // SAFETY: as the caller promises.
    pthread_ok(unsafe { swr_v1_os_thread_set_name(name) })
}

/// `bool swoole_thread_get_name(char *buf, size_t len)`.
/// # Safety
/// `buf` points to `len` writable bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z22swoole_thread_get_namePcm")
)]
pub unsafe extern "C" fn swoole_thread_get_name(buf: *mut c_char, len: usize) -> bool {
    // SAFETY: as the caller promises.
    pthread_ok(unsafe { swr_v1_os_thread_get_name(buf, len) })
}

/// `size_t swoole_random_bytes(char *buf, size_t size)`, over corebase's.
/// # Safety
/// `buf` points to `size` writable bytes.
#[cfg_attr(
    feature = "upstream-symbols",
    unsafe(export_name = "_Z19swoole_random_bytesPcm")
)]
pub unsafe extern "C" fn swoole_random_bytes(buf: *mut c_char, size: usize) -> usize {
    // SAFETY: as the caller promises.
    swoole_rs_corebase::random_bytes(unsafe { std::slice::from_raw_parts_mut(buf.cast(), size) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolation_reports_unknown_names_and_skips_empty_steps() {
        let mut steps = Vec::new();
        isolate(c"", c"", c"", |step| steps.push(step));
        assert!(steps.is_empty(), "nothing to do");
        let missing = c"swr-no-such-name";
        isolate(missing, missing, c"", |step| steps.push(step));
        assert_eq!(steps, [1, 2], "both lookups fail and nothing is switched");
    }

    #[test]
    fn identity_matches_the_process() {
        assert_eq!(
            swr_v1_os_is_root_user(),
            unsafe { libc::geteuid() } == 0,
            "euid zero"
        );
        assert!(swr_v1_os_thread_native_id() > 0, "a kernel thread id");
        let other = std::thread::spawn(|| swr_v1_os_thread_native_id())
            .join()
            .unwrap_or_default();
        assert_ne!(other, swr_v1_os_thread_native_id(), "per thread");
    }

    #[test]
    fn thread_names_round_trip() {
        std::thread::spawn(|| {
            assert_eq!(
                unsafe { swr_v1_os_thread_set_name(c"swr-worker".as_ptr()) },
                0,
                "set"
            );
            let mut buf = [0 as c_char; 32];
            assert_eq!(
                unsafe { swr_v1_os_thread_get_name(buf.as_mut_ptr(), buf.len()) },
                0,
                "get"
            );
            let name = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) };
            assert_eq!(name, c"swr-worker", "same name");
            assert!(unsafe { swoole_thread_set_name(c"swr-upstream".as_ptr()) });
            assert!(unsafe { swoole_thread_get_name(buf.as_mut_ptr(), buf.len()) });
            let name = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) };
            assert_eq!(name, c"swr-upstream", "upstream wrappers");
        })
        .join()
        .unwrap_or_else(|_| unreachable!());
    }

    /// Upstream's `os.thread_name`. Linux, as core-tests: its 15-byte name limit is
    /// what makes the second name fail.
    #[cfg(target_os = "linux")]
    #[test]
    fn os_thread_name() {
        std::thread::spawn(|| {
            let mut new_name = [0 as c_char; 512];
            let thread_name = c"sw-core-tests";
            assert!(unsafe { swoole_thread_set_name(thread_name.as_ptr()) });
            assert!(unsafe { swoole_thread_get_name(new_name.as_mut_ptr(), new_name.len()) });
            assert_eq!(thread_name, unsafe { CStr::from_ptr(new_name.as_ptr()) });
            assert!(!unsafe {
                swoole_thread_set_name(c"swoole-core-tests-max-size-is-16".as_ptr())
            });
            assert_eq!(log::last_error(), libc::ERANGE);
        })
        .join()
        .unwrap_or_else(|_| panic!("the thread's assertions failed"));
    }

    /// Upstream's `os.cpu_affinity`. `swoole_set_cpu_affinity` left the build (Rust pins
    /// workers in crates/process `affinity.rs`), so the set is its body,
    /// `sched_setaffinity(getpid())`; the gets are Rust's `swoole_get_cpu_affinity`.
    #[cfg(target_os = "linux")]
    #[test]
    fn os_cpu_affinity() {
        let set = |set: &libc::cpu_set_t| unsafe {
            libc::sched_setaffinity(libc::getpid(), size_of::<libc::cpu_set_t>(), set)
        };
        let mut affinity: libc::cpu_set_t = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { swoole_get_cpu_affinity(&raw mut affinity) }, 0);
        let ori_affinity = affinity;

        unsafe { libc::CPU_ZERO(&mut affinity) };
        unsafe { libc::CPU_SET(1, &mut affinity) };

        assert_eq!(set(&affinity), 0);
        assert_eq!(unsafe { swoole_get_cpu_affinity(&raw mut affinity) }, 0);

        // SW_CPU_NUM, as swoole_init sets it.
        let cpu_n = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) }.max(1);
        for i in 0..usize::try_from(cpu_n).unwrap() {
            assert_eq!(unsafe { libc::CPU_ISSET(i, &affinity) }, i == 1, "cpu {i}");
        }

        assert_eq!(set(&ori_affinity), 0);
    }

    /// Upstream's `os.set_isolation`: unknown names and a missing chroot only warn.
    #[test]
    fn os_set_isolation() {
        unsafe {
            swr_v1_os_set_isolation(
                c"not-exists-group".as_ptr(),
                c"not-exists-user".as_ptr(),
                c"/tmp/not-exists-dir".as_ptr(),
            );
        }
    }
}
