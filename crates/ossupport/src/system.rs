//! Process and thread identity helpers from upstream `src/os/base.cc`: root
//! check, user/group/chroot isolation, parent-death signal, CPU affinity, native
//! thread id and thread names. Native code logs; Rust runs the syscalls.
use libc::{c_char, c_int, c_long, c_void};

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
/// then stdin/stdout/stderr onto `/dev/null` unless `noclose`. 0 done, or the
/// failed step for native's warning: 1 chdir, 2 open, 3 dup2.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_os_daemon_detach(nochdir: bool, noclose: bool) -> c_int {
    // SAFETY: a static C string.
    if !nochdir && unsafe { libc::chdir(c"/".as_ptr()) } != 0 {
        return 1;
    }
    if noclose {
        return 0;
    }
    // SAFETY: a static C string.
    let fd = unsafe { libc::open(c"/dev/null".as_ptr(), libc::O_RDWR) };
    if fd < 0 {
        return 2;
    }
    // SAFETY: dup2 onto the standard descriptors, as upstream.
    let redirected = (0..3).all(|target| unsafe { libc::dup2(fd, target) } >= 0);
    // SAFETY: the descriptor opened above.
    unsafe { libc::close(fd) };
    if redirected { 0 } else { 3 }
}

/// A failed `swoole_set_isolation` step, reported as it happens (errno intact):
/// 1 group lookup, 2 user lookup, 3 setgid, 4 setuid, 5 `chdir("/")`, 6 chroot.
pub type IsolationWarn = extern "C" fn(step: u32, context: *mut c_void);

/// # Safety
/// `name` is a valid NUL-terminated string.
unsafe fn present(name: *const c_char) -> bool {
    // SAFETY: a valid C string has at least its terminator.
    unsafe { *name != 0 }
}

/// `swoole_set_isolation`: look up the group and user, switch gid then uid, then
/// chroot and `chdir("/")`; empty names skip their step.
/// # Safety
/// `group`, `user` and `root` are valid NUL-terminated strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_set_isolation(
    group: *const c_char,
    user: *const c_char,
    root: *const c_char,
    warn: IsolationWarn,
    context: *mut c_void,
) {
    let mut gid = None;
    // SAFETY: valid C string per the contract.
    if unsafe { present(group) } {
        // SAFETY: valid C string; the entry is read before the next lookup.
        let entry = unsafe { libc::getgrnam(group) };
        // SAFETY: getgrnam returned null or a valid entry.
        gid = unsafe { entry.as_ref() }.map(|entry| entry.gr_gid);
        if gid.is_none() {
            warn(1, context);
        }
    }
    let mut uid = None;
    // SAFETY: valid C string per the contract.
    if unsafe { present(user) } {
        // SAFETY: valid C string; the entry is read before the next lookup.
        let entry = unsafe { libc::getpwnam(user) };
        // SAFETY: getpwnam returned null or a valid entry.
        uid = unsafe { entry.as_ref() }.map(|entry| entry.pw_uid);
        if uid.is_none() {
            warn(2, context);
        }
    }
    // SAFETY: setgid has no memory preconditions.
    if gid.is_some_and(|gid| unsafe { libc::setgid(gid) } < 0) {
        warn(3, context);
    }
    // SAFETY: setuid has no memory preconditions.
    if uid.is_some_and(|uid| unsafe { libc::setuid(uid) } < 0) {
        warn(4, context);
    }
    // SAFETY: valid C string per the contract.
    if unsafe { present(root) } {
        // SAFETY: valid C string per the contract.
        if unsafe { libc::chroot(root) } == 0 {
            // SAFETY: a static C string.
            if unsafe { libc::chdir(c"/".as_ptr()) } < 0 {
                warn(5, context);
            }
        } else {
            warn(6, context);
        }
    }
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

/// `swoole_set_cpu_affinity`: the whole process.
/// # Safety
/// `set` points to a valid `cpu_set_t`.
#[cfg(any(target_os = "linux", target_os = "android", target_os = "freebsd"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_set_cpu_affinity(set: *const CpuSet) -> c_int {
    #[cfg(target_os = "freebsd")]
    // SAFETY: `set` is a valid `cpu_set_t` per the contract.
    unsafe {
        libc::cpuset_setaffinity(
            libc::CPU_LEVEL_WHICH,
            libc::CPU_WHICH_PID,
            -1,
            size_of::<CpuSet>(),
            set,
        )
    }
    #[cfg(not(target_os = "freebsd"))]
    {
        // SAFETY: getpid has no preconditions. Upstream pins the process's main
        // thread (tid == pid), not the calling thread.
        let pid = unsafe { libc::getpid() };
        // SAFETY: `set` is a valid `cpu_set_t` per the contract.
        unsafe { libc::sched_setaffinity(pid, size_of::<CpuSet>(), set) }
    }
}

/// `swoole_get_cpu_affinity`: the whole process.
/// # Safety
/// `set` points to a writable `cpu_set_t`.
#[cfg(any(target_os = "linux", target_os = "android", target_os = "freebsd"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_get_cpu_affinity(set: *mut CpuSet) -> c_int {
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
        // SAFETY: getpid has no preconditions. Upstream pins the process's main
        // thread (tid == pid), not the calling thread.
        let pid = unsafe { libc::getpid() };
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
    unsafe { swoole_rs_corebase::swr_v1_os_random_bytes(buf.cast(), size) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static STEPS: Mutex<Vec<u32>> = Mutex::new(Vec::new());
    extern "C" fn record(step: u32, _context: *mut c_void) {
        STEPS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(step);
    }

    #[test]
    fn isolation_reports_unknown_names_and_skips_empty_steps() {
        unsafe {
            swr_v1_os_set_isolation(
                c"".as_ptr(),
                c"".as_ptr(),
                c"".as_ptr(),
                record,
                std::ptr::null_mut(),
            );
        }
        assert!(
            STEPS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty(),
            "nothing to do"
        );
        let missing = c"swr-no-such-name";
        unsafe {
            swr_v1_os_set_isolation(
                missing.as_ptr(),
                missing.as_ptr(),
                c"".as_ptr(),
                record,
                std::ptr::null_mut(),
            );
        }
        assert_eq!(
            *STEPS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            [1, 2],
            "both lookups fail and nothing is switched"
        );
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
}
