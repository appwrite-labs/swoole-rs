//! Target-specific config.h probes shared by the native build and its Rust callers.

/// The configure probes for the target OS. Build scripts pass Cargo's target,
/// which can differ from the OS running the build script.
pub(crate) fn defines(os: &str) -> impl Iterator<Item = &'static str> {
    let common = [
        "HAVE_AI_ALL",
        "HAVE_AI_V4MAPPED",
        "HAVE_EXECINFO",
        "HAVE_MSGQUEUE",
        "HAVE_REUSEPORT",
        "HAVE_RWLOCK",
    ];
    let platform: &[&str] = if os == "macos" {
        &["HAVE_KQUEUE"]
    } else {
        &[
            "HAVE_CPU_AFFINITY",
            "HAVE_EPOLL",
            "HAVE_EVENTFD",
            "HAVE_FUTEX",
            "HAVE_MALLOC_TRIM",
            "HAVE_MUTEX_TIMEDLOCK",
            "HAVE_PTHREAD_BARRIER",
            "HAVE_RWLOCK_TIMEDRDLOCK",
            "HAVE_SIGNALFD",
            "HAVE_SPINLOCK",
        ]
    };
    common.into_iter().chain(platform.iter().copied())
}
