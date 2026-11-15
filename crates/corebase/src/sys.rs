//! Safe wrappers for the libc calls the crates make on plain descriptors and
//! process state. Buffers are Rust slices, so the calls have no memory
//! preconditions and callers need no `unsafe`. Results are libc's, with errno
//! left as the call set it.
#![expect(
    clippy::must_use_candidate,
    reason = "callers ignore a close or write result where the C they replace did"
)]
use libc::{c_int, off_t, pid_t};
use std::ffi::CStr;
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::ptr::NonNull;

/// A NUL-terminated string that stays live and unchanged for `'a`, as thin as the
/// `const char *` it is, so a C function's string parameter can be declared as one on
/// either side of the boundary.
///
/// Invariant: it points at a NUL-terminated string live for `'a`. Rust makes one only
/// from a `&'a CStr`. The other source is a parameter (or declared static) of this type
/// on the C boundary, where the C caller's `const char *` contract stands for the
/// constructor, as it does for a `&T` parameter there.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Text<'a>(NonNull<libc::c_char>, PhantomData<&'a CStr>);

impl<'a> From<&'a CStr> for Text<'a> {
    fn from(text: &'a CStr) -> Self {
        Self(NonNull::from(text).cast(), PhantomData)
    }
}

impl<'a> Text<'a> {
    /// The string.
    #[must_use]
    pub fn get(self) -> &'a CStr {
        // SAFETY: the type's invariant: NUL-terminated and live for `'a`.
        unsafe { CStr::from_ptr(self.0.as_ptr()) }
    }
}

/// libc's calls that take only plain values, a live reference of the exact type they
/// read or write, or a NUL-terminated string, and keep none of them: they have no
/// memory preconditions, so they are declared `safe`.
mod c {
    use libc::{c_char, c_int, c_long, c_uint, c_ulong, c_void, mode_t, pid_t, useconds_t};

    /// `getrlimit`'s resource: glibc declares an unsigned enum, the others an `int`.
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    type Resource = libc::__rlimit_resource_t;
    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    type Resource = c_int;

    /// `getpriority`'s `which`: glibc declares an unsigned enum, the others an `int`.
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    type Which = libc::__priority_which_t;
    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    type Which = c_int;

    pub(crate) use super::Text;

    // POSIX's prototypes, the same on every Unix this builds for. Each reads
    // its integers, reads a `Text` up to its NUL, or reads or writes the one object
    // its reference names. `malloc`, `calloc` and `strndup` return null or a fresh
    // allocation; `strtoul` stores a pointer into its text, or leaves `end` alone. A
    // `sigset_t` is plain bits the set calls read or write whole, and a signal number
    // outside the set is an error they return. `fork` takes nothing and touches no
    // memory of the caller's; it stays inside the crate, whose callers each say
    // what their child goes on to do.
    #[expect(
        suspicious_runtime_symbol_definitions,
        reason = "`close` and `malloc` keep their C signatures; they are declared safe because \
                  each takes plain values and touches no caller memory"
    )]
    unsafe extern "C" {
        pub(crate) safe fn socket(domain: c_int, kind: c_int, protocol: c_int) -> c_int;
        pub(crate) safe fn listen(fd: c_int, backlog: c_int) -> c_int;
        pub(crate) safe fn shutdown(fd: c_int, how: c_int) -> c_int;
        pub(crate) safe fn socketpair(
            domain: c_int,
            kind: c_int,
            protocol: c_int,
            fds: &mut [c_int; 2],
        ) -> c_int;
        pub(crate) safe fn pipe(fds: &mut [c_int; 2]) -> c_int;
        pub(crate) safe fn fork() -> pid_t;
        pub(crate) safe fn dup(fd: c_int) -> c_int;
        pub(crate) safe fn dup2(from: c_int, to: c_int) -> c_int;
        pub(crate) safe fn close(fd: c_int) -> c_int;
        pub(crate) safe fn flock(fd: c_int, operation: c_int) -> c_int;
        pub(crate) safe fn sleep(seconds: c_uint) -> c_uint;
        pub(crate) safe fn usleep(microseconds: useconds_t) -> c_int;
        pub(crate) safe fn unlink(path: Text<'_>) -> c_int;
        pub(crate) safe fn access(path: Text<'_>, mode: c_int) -> c_int;
        pub(crate) safe fn chdir(path: Text<'_>) -> c_int;
        pub(crate) safe fn chroot(path: Text<'_>) -> c_int;
        pub(crate) safe fn mkdir(path: Text<'_>, mode: mode_t) -> c_int;
        pub(crate) safe fn malloc(size: usize) -> *mut c_void;
        pub(crate) safe fn calloc(count: usize, size: usize) -> *mut c_void;
        /// At most `length` bytes of `text`, in a `malloc` allocation.
        pub(crate) safe fn strndup(text: Text<'_>, length: usize) -> *mut c_char;
        pub(crate) safe fn strncasecmp(left: Text<'_>, right: Text<'_>, count: usize) -> c_int;
        pub(crate) safe fn strtoul(text: Text<'_>, end: &mut *mut c_char, base: c_int) -> c_ulong;
        pub(crate) safe fn if_nametoindex(name: Text<'_>) -> c_uint;
        pub(crate) safe fn sysconf(name: c_int) -> c_long;
        pub(crate) safe fn kill(pid: pid_t, signo: c_int) -> c_int;
        pub(crate) safe fn getpriority(which: Which, who: libc::id_t) -> c_int;
        pub(crate) safe fn setpriority(which: Which, who: libc::id_t, priority: c_int) -> c_int;
        pub(crate) safe fn waitpid(pid: pid_t, status: Option<&mut c_int>, options: c_int)
        -> pid_t;
        pub(crate) safe fn uname(name: &mut libc::utsname) -> c_int;
        pub(crate) safe fn getrlimit(resource: Resource, limit: &mut libc::rlimit) -> c_int;
        /// Stores the head of a new list, none for an empty one, when it returns 0.
        pub(super) safe fn getifaddrs(list: &mut Option<super::InterfaceList>) -> c_int;
        /// Frees a list `getifaddrs` stored, which the handle's move hands over.
        pub(super) safe fn freeifaddrs(list: super::InterfaceList);
        pub(crate) safe fn srandom(seed: c_uint);
        #[cfg_attr(
            all(target_os = "macos", target_arch = "x86_64"),
            link_name = "fstat$INODE64"
        )]
        #[cfg_attr(target_os = "netbsd", link_name = "__fstat50")]
        pub(crate) safe fn fstat(fd: c_int, status: &mut libc::stat) -> c_int;
        #[cfg_attr(
            all(target_os = "macos", target_arch = "x86_64"),
            link_name = "stat$INODE64"
        )]
        #[cfg_attr(target_os = "netbsd", link_name = "__stat50")]
        pub(crate) safe fn stat(path: Text<'_>, status: &mut libc::stat) -> c_int;
        #[cfg_attr(target_os = "netbsd", link_name = "__nanosleep50")]
        pub(crate) safe fn nanosleep(
            request: &libc::timespec,
            remaining: Option<&mut libc::timespec>,
        ) -> c_int;
        #[cfg_attr(target_os = "netbsd", link_name = "__setitimer50")]
        pub(crate) safe fn setitimer(
            which: c_int,
            value: &libc::itimerval,
            old: Option<&mut libc::itimerval>,
        ) -> c_int;
        /// Returns `calendar`, or null for a time with no calendar.
        #[cfg_attr(target_os = "netbsd", link_name = "__localtime_r50")]
        pub(crate) safe fn localtime_r(
            time: &libc::time_t,
            calendar: &mut libc::tm,
        ) -> *mut libc::tm;
        #[cfg_attr(target_os = "netbsd", link_name = "__clock_gettime50")]
        pub(crate) safe fn clock_gettime(clock: libc::clockid_t, now: &mut libc::timespec)
        -> c_int;
        /// The id of the calling thread, for a null `thread`.
        #[cfg(target_vendor = "apple")]
        pub(crate) safe fn pthread_threadid_np(thread: libc::pthread_t, id: &mut u64) -> c_int;
        /// `sigemptyset(3)`: `set` without a signal.
        #[cfg_attr(target_os = "netbsd", link_name = "__sigemptyset14")]
        pub safe fn sigemptyset(set: &mut libc::sigset_t) -> c_int;
        /// `sigfillset(3)`: `set` with every signal.
        #[cfg_attr(target_os = "netbsd", link_name = "__sigfillset14")]
        pub safe fn sigfillset(set: &mut libc::sigset_t) -> c_int;
        /// `sigaddset(3)`: -1 with errno set for a number that names no signal.
        #[cfg_attr(target_os = "netbsd", link_name = "__sigaddset14")]
        pub safe fn sigaddset(set: &mut libc::sigset_t, signo: c_int) -> c_int;
        /// `sigdelset(3)`: -1 with errno set for a number that names no signal.
        #[cfg_attr(target_os = "netbsd", link_name = "__sigdelset14")]
        pub safe fn sigdelset(set: &mut libc::sigset_t, signo: c_int) -> c_int;
        /// `sigismember(3)`: 1, 0, or -1 with errno set for a number that names no signal.
        #[cfg_attr(target_os = "netbsd", link_name = "__sigismember14")]
        pub safe fn sigismember(set: &libc::sigset_t, signo: c_int) -> c_int;
        /// `sigprocmask(2)`: changes the mask by `set`, when given, and stores the mask
        /// from before in `old`, when given.
        #[cfg_attr(target_os = "netbsd", link_name = "__sigprocmask14")]
        pub safe fn sigprocmask(
            how: c_int,
            set: Option<&libc::sigset_t>,
            old: Option<&mut libc::sigset_t>,
        ) -> c_int;
        /// `pthread_sigmask(3)`, as `sigprocmask` for the calling thread: 0 or an error
        /// number.
        pub safe fn pthread_sigmask(
            how: c_int,
            set: Option<&libc::sigset_t>,
            old: Option<&mut libc::sigset_t>,
        ) -> c_int;
        /// `signalfd(2)`: a new descriptor for -1, else `fd` with the new mask.
        #[cfg(any(target_os = "linux", target_os = "android"))]
        pub safe fn signalfd(fd: c_int, mask: &libc::sigset_t, flags: c_int) -> c_int;
        /// `epoll_create(2)`.
        #[cfg(any(target_os = "linux", target_os = "android"))]
        pub safe fn epoll_create(size: c_int) -> c_int;
        /// `epoll_ctl(2)`: `event` is the interest and token to keep, none for
        /// `EPOLL_CTL_DEL`.
        #[cfg(any(target_os = "linux", target_os = "android"))]
        pub safe fn epoll_ctl(
            epfd: c_int,
            op: c_int,
            fd: c_int,
            event: Option<&mut libc::epoll_event>,
        ) -> c_int;
        /// `kqueue(2)`: a new queue's descriptor.
        #[cfg(any(target_vendor = "apple", target_os = "freebsd"))]
        pub safe fn kqueue() -> c_int;
        /// glibc's `malloc_trim(3)`, which takes the allocator's own locks: 1 when it
        /// gave memory back to the system.
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        pub safe fn malloc_trim(pad: usize) -> c_int;
        /// This thread's errno slot, live for the thread: as a `Cell`, of `c_int`'s
        /// layout, so Rust writes it with plain stores and holds no reference into
        /// it, and the reference (`!Send`, as `Cell` is `!Sync`) never leaves the
        /// thread. errno is per thread on every platform this builds for.
        #[cfg_attr(
            any(target_os = "linux", target_os = "android"),
            link_name = "__errno_location"
        )]
        #[cfg_attr(
            not(any(target_os = "linux", target_os = "android")),
            link_name = "__error"
        )]
        pub(crate) safe fn errno_location() -> &'static std::cell::Cell<c_int>;
        /// This thread's `h_errno` slot, as [`errno_location`]'s: glibc and musl keep
        /// one per thread.
        #[cfg(target_os = "linux")]
        #[link_name = "__h_errno_location"]
        pub(crate) safe fn h_errno_location() -> &'static std::cell::Cell<c_int>;
        /// Apple's `h_errno`, one for the process: an int, which Rust reads and writes
        /// as one, atomically.
        #[cfg(not(target_os = "linux"))]
        pub(crate) safe static h_errno: std::sync::atomic::AtomicI32;
    }
}
#[cfg(any(target_vendor = "apple", target_os = "freebsd"))]
pub use c::kqueue;
#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub use c::malloc_trim;
pub(crate) use c::{calloc, fork, getrlimit, malloc, srandom, uname};
#[cfg(any(target_os = "linux", target_os = "android"))]
pub use c::{epoll_create, epoll_ctl, signalfd};
pub use c::{
    pthread_sigmask, sigaddset, sigdelset, sigemptyset, sigfillset, sigismember, sigprocmask,
};

/// A byte a read may overwrite with any value: `u8`, or one not yet written
/// (`CppString::spare`). Readers only write their buffer, so they take either.
pub trait ReadSlot: sealed::Sealed + Sized {
    /// Copy what fits of `bytes` to the start of `slots`: the count copied.
    fn copy_in(slots: &mut [Self], bytes: &[u8]) -> usize;
}

impl ReadSlot for u8 {
    #[inline]
    fn copy_in(slots: &mut [Self], bytes: &[u8]) -> usize {
        let n = slots.len().min(bytes.len());
        if let (Some(to), Some(from)) = (slots.get_mut(..n), bytes.get(..n)) {
            to.copy_from_slice(from);
        }
        n
    }
}

impl ReadSlot for MaybeUninit<u8> {
    #[inline]
    fn copy_in(slots: &mut [Self], bytes: &[u8]) -> usize {
        let n = slots.len().min(bytes.len());
        if let (Some(to), Some(from)) = (slots.get_mut(..n), bytes.get(..n)) {
            to.write_copy_of_slice(from);
        }
        n
    }
}

mod sealed {
    /// Only one-byte types every bit pattern initializes.
    pub trait Sealed {}
    impl Sealed for u8 {}
    impl Sealed for std::mem::MaybeUninit<u8> {}

    /// Only the types `Zeroed` documents.
    pub trait Zero {}

    /// Only the types `Pod` documents.
    pub trait Plain: Zero {}
}

/// A C struct all-zero bytes initialize: its fields are integers, byte arrays, raw
/// pointers and nullable function pointers. Sealed: only the types listed here.
pub trait Zeroed: sealed::Zero {}
impl<T: sealed::Zero> Zeroed for T {}

/// C plain data: integers, byte arrays, raw pointers, and structs and unions of
/// them, with no function pointer. Any bytes are a value of one, so a caller may
/// hand it to the kernel to fill. Sealed: only the types listed here.
pub trait Pod: sealed::Plain + Copy {}
impl<T: sealed::Plain + Copy> Pod for T {}

macro_rules! plain {
    ($($(#[$cfg:meta])? $type:ty),* $(,)?) => {
        $($(#[$cfg])? impl sealed::Zero for $type {})*
        $($(#[$cfg])? impl sealed::Plain for $type {})*
    };
}
// Linux's sigaction holds `sa_restorer`, a nullable function pointer: zero is a
// value of it, arbitrary bytes are not one to call.
impl sealed::Zero for libc::sigaction {}
// Their strings are raw pointers, null until a lookup fills them.
impl sealed::Zero for libc::group {}
impl sealed::Zero for libc::passwd {}
// Integers and the raw pointers to its vectors and name, null until a caller sets them.
impl sealed::Zero for libc::msghdr {}
// A System V message queue's status and the permissions in it: integers.
impl sealed::Zero for libc::ipc_perm {}
#[cfg(target_os = "linux")]
impl sealed::Zero for libc::msqid_ds {}
plain!(
    u8,
    i32,
    u32,
    i64,
    u64,
    usize,
    *mut libc::c_void,
    libc::addrinfo,
    libc::group_req,
    libc::group_source_req,
    libc::hostent,
    libc::ifreq,
    libc::in_addr,
    libc::in6_addr,
    libc::in6_pktinfo,
    libc::ip_mreq,
    libc::ipv6_mreq,
    libc::itimerval,
    libc::linger,
    libc::sched_param,
    libc::siginfo_t,
    // Apple's sigset_t is a u32.
    #[cfg(not(target_vendor = "apple"))]
    libc::sigset_t,
    libc::sockaddr,
    libc::sockaddr_in,
    libc::sockaddr_in6,
    libc::sockaddr_storage,
    libc::sockaddr_un,
    libc::stat,
    libc::statvfs,
    libc::timeval,
    libc::tm,
    libc::utsname,
    crate::net_types::AddressStorage,
    crate::net_types::SocketAddress,
    #[cfg(any(target_os = "linux", target_os = "android"))]
    libc::cpu_set_t,
    #[cfg(any(target_os = "linux", target_os = "android"))]
    libc::ifconf,
    #[cfg(any(target_os = "linux", target_os = "android"))]
    libc::signalfd_siginfo,
    #[cfg(any(target_os = "linux", target_os = "android"))]
    libc::statx,
    #[cfg(any(target_os = "linux", target_os = "android"))]
    libc::tcp_info,
    #[cfg(target_vendor = "apple")]
    libc::tcp_connection_info,
    #[cfg(any(target_vendor = "apple", target_os = "freebsd"))]
    libc::kevent,
);
impl<T: sealed::Zero, const N: usize> sealed::Zero for [T; N] {}
impl<T: sealed::Plain, const N: usize> sealed::Plain for [T; N] {}

/// An all-zero `T`, as C's `= {}`.
#[must_use]
pub const fn zeroed<T: Zeroed>() -> T {
    // SAFETY: zero bytes are a value of every `Zeroed`.
    unsafe { std::mem::zeroed() }
}

/// `size_of::<T>()` as a `socklen_t`.
#[must_use]
pub const fn socklen<T>() -> libc::socklen_t {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "an option or address struct is far smaller than 4 GiB"
    )]
    {
        size_of::<T>() as libc::socklen_t
    }
}

/// This thread's errno.
#[must_use]
pub fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// Sets this thread's errno.
pub fn set_errno(value: c_int) {
    c::errno_location().set(value);
}

/// The resolver's error, `h_errno`: this thread's on Linux, the process's on Apple.
#[must_use]
pub fn h_errno() -> c_int {
    #[cfg(target_os = "linux")]
    return c::h_errno_location().get();
    #[cfg(not(target_os = "linux"))]
    return c::h_errno.load(std::sync::atomic::Ordering::Relaxed);
}

/// Sets `h_errno`, as [`h_errno`] reads it.
pub fn set_h_errno(value: c_int) {
    #[cfg(target_os = "linux")]
    c::h_errno_location().set(value);
    #[cfg(not(target_os = "linux"))]
    c::h_errno.store(value, std::sync::atomic::Ordering::Relaxed);
}

/// `strerror(3)`: libc's message for `code`, as std's `strerror_r` reads it.
#[must_use]
pub fn strerror(code: c_int) -> String {
    let mut text = std::io::Error::from_raw_os_error(code).to_string();
    // std appends the code; strip that, leaving libc's message.
    let message = text
        .strip_suffix(&format!(" (os error {code})"))
        .map_or(text.len(), str::len);
    text.truncate(message);
    text
}

/// `read(2)` into `buf`.
pub fn read(fd: c_int, buf: &mut [impl ReadSlot]) -> isize {
    // SAFETY: `buf` is writable for its length.
    unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) }
}

/// `write(2)` of `buf`.
pub fn write(fd: c_int, buf: &[u8]) -> isize {
    // SAFETY: `buf` is readable for its length.
    unsafe { libc::write(fd, buf.as_ptr().cast(), buf.len()) }
}

/// `pread(2)` into `buf` at `offset`.
pub fn pread(fd: c_int, buf: &mut [impl ReadSlot], offset: off_t) -> isize {
    // SAFETY: `buf` is writable for its length.
    unsafe { libc::pread(fd, buf.as_mut_ptr().cast(), buf.len(), offset) }
}

/// `pwrite(2)` of `buf` at `offset`.
pub fn pwrite(fd: c_int, buf: &[u8], offset: off_t) -> isize {
    // SAFETY: `buf` is readable for its length.
    unsafe { libc::pwrite(fd, buf.as_ptr().cast(), buf.len(), offset) }
}

/// `close(2)`.
pub fn close(fd: c_int) -> c_int {
    c::close(fd)
}

/// `writev(2)` of `vectors`.
#[must_use]
#[inline]
pub fn writev(fd: c_int, vectors: &[std::io::IoSlice<'_>]) -> isize {
    let count = c_int::try_from(vectors.len()).unwrap_or(c_int::MAX);
    // SAFETY: an `IoSlice` is ABI-compatible with an iovec, of readable bytes.
    unsafe { libc::writev(fd, vectors.as_ptr().cast(), count) }
}

/// `listen(2)`.
pub fn listen(fd: c_int, backlog: c_int) -> c_int {
    c::listen(fd, backlog)
}

/// `socket(2)`.
#[must_use]
pub fn socket(domain: c_int, kind: c_int, protocol: c_int) -> c_int {
    c::socket(domain, kind, protocol)
}

/// `shutdown(2)`.
pub fn shutdown(fd: c_int, how: c_int) -> c_int {
    c::shutdown(fd, how)
}

/// `getsockopt(2)` of an int option into `value`, which keeps what it held when
/// the call fails.
pub fn getsockopt_int(fd: c_int, level: c_int, name: c_int, value: &mut c_int) -> c_int {
    getsockopt(fd, level, name, value).map_or(-1, |_| 0)
}

/// `getsockopt(2)` into `value`: the length the kernel stored, or `None` with
/// errno set.
pub fn getsockopt<T: Pod>(
    fd: c_int,
    level: c_int,
    name: c_int,
    value: &mut T,
) -> Option<libc::socklen_t> {
    let mut length = socklen::<T>();
    // SAFETY: `value` holds the `length` writable bytes given, and any bytes are a `T`.
    let status =
        unsafe { libc::getsockopt(fd, level, name, (&raw mut *value).cast(), &raw mut length) };
    (status == 0).then_some(length)
}

/// `setsockopt(2)` of `value`.
pub fn setsockopt<T: Pod>(fd: c_int, level: c_int, name: c_int, value: &T) -> c_int {
    // SAFETY: `value` holds the readable bytes whose count is given.
    unsafe { libc::setsockopt(fd, level, name, (&raw const *value).cast(), socklen::<T>()) }
}

/// `setsockopt(2)` of an option given as bytes.
pub fn setsockopt_bytes(fd: c_int, level: c_int, name: c_int, value: &[u8]) -> c_int {
    let length = libc::socklen_t::try_from(value.len()).unwrap_or(libc::socklen_t::MAX);
    // SAFETY: `value` is readable for its length, which `length` does not exceed.
    unsafe { libc::setsockopt(fd, level, name, value.as_ptr().cast(), length) }
}

/// `socketpair(2)`: both ends, or `None` with errno set.
#[must_use]
pub fn socketpair(domain: c_int, kind: c_int, protocol: c_int) -> Option<[c_int; 2]> {
    let mut fds = [0; 2];
    (c::socketpair(domain, kind, protocol, &mut fds) == 0).then_some(fds)
}

/// `recv(2)` into `buf`.
pub fn recv(fd: c_int, buf: &mut [impl ReadSlot], flags: c_int) -> isize {
    // SAFETY: `buf` is writable for its length.
    unsafe { libc::recv(fd, buf.as_mut_ptr().cast(), buf.len(), flags) }
}

/// `send(2)` of `buf`.
pub fn send(fd: c_int, buf: &[u8], flags: c_int) -> isize {
    // SAFETY: `buf` is readable for its length.
    unsafe { libc::send(fd, buf.as_ptr().cast(), buf.len(), flags) }
}

/// `dup(2)`.
#[must_use]
pub fn dup(fd: c_int) -> c_int {
    c::dup(fd)
}

/// `dup2(2)`.
pub fn dup2(from: c_int, to: c_int) -> c_int {
    c::dup2(from, to)
}

/// `fcntl(2)` with an integer argument (`F_GETFL`, `F_SETFL`, `F_GETFD`,
/// `F_SETFD`, `F_DUPFD`, ...); commands that take no argument ignore it.
pub fn fcntl(fd: c_int, cmd: c_int, arg: c_int) -> c_int {
    // SAFETY: an integer argument has no memory preconditions.
    unsafe { libc::fcntl(fd, cmd, arg) }
}

/// `flock(2)`.
pub fn flock(fd: c_int, operation: c_int) -> c_int {
    c::flock(fd, operation)
}

/// `poll(2)` of `fds`.
pub fn poll(fds: &mut [libc::pollfd], timeout: c_int) -> c_int {
    let count = libc::nfds_t::try_from(fds.len()).unwrap_or(libc::nfds_t::MAX);
    // SAFETY: `fds` holds `count` writable pollfds.
    unsafe { libc::poll(fds.as_mut_ptr(), count, timeout) }
}

/// `pipe(2)`: the read and write ends, or `None` with errno set.
#[must_use]
pub fn pipe() -> Option<[c_int; 2]> {
    let mut fds = [0; 2];
    (c::pipe(&mut fds) == 0).then_some(fds)
}

/// `fstat(2)`: the descriptor's status, or `None` with errno set.
#[must_use]
pub fn fstat(fd: c_int) -> Option<libc::stat> {
    let mut stat = zeroed::<libc::stat>();
    (c::fstat(fd, &mut stat) == 0).then_some(stat)
}

/// `sleep(3)`: the seconds left when a signal cut it short.
pub fn sleep(seconds: libc::c_uint) -> libc::c_uint {
    c::sleep(seconds)
}

/// `usleep(3)`.
pub fn usleep(microseconds: libc::useconds_t) -> c_int {
    c::usleep(microseconds)
}

/// `nanosleep(2)` for `request`, the time left going to `remaining`.
pub fn nanosleep(request: &libc::timespec, remaining: &mut libc::timespec) -> c_int {
    c::nanosleep(request, Some(remaining))
}

/// `open(2)` of `path`; `mode` applies when `flags` has `O_CREAT`.
pub fn open(path: &CStr, flags: c_int, mode: libc::mode_t) -> c_int {
    // SAFETY: `path` is NUL-terminated; the mode is promoted as C does.
    unsafe { libc::open(path.as_ptr(), flags, libc::c_uint::from(mode)) }
}

/// `unlink(2)`.
pub fn unlink(path: &CStr) -> c_int {
    c::unlink(path.into())
}

/// `bytes` to C's `stdout`, which native code's own `printf` shares, so the two
/// keep their order.
pub fn print(bytes: &[u8]) {
    for chunk in bytes.chunks(c_int::MAX.unsigned_abs() as usize) {
        let length = c_int::try_from(chunk.len()).unwrap_or(c_int::MAX);
        // SAFETY: "%.*s" reads at most `length` bytes of `chunk`.
        unsafe { libc::printf(c"%.*s".as_ptr(), length, chunk.as_ptr()) };
    }
}

/// One change to the kqueue `kq`, no events out: `kevent(2)`'s result.
#[cfg(any(target_vendor = "apple", target_os = "freebsd"))]
#[must_use]
pub fn kevent_change(kq: c_int, change: &libc::kevent) -> c_int {
    kevent::kevent(
        kq,
        change,
        kevent::Count::One,
        None,
        kevent::Count::Zero,
        None,
    )
}

/// `kevent(2)` declared for one change and no events out.
#[cfg(any(target_vendor = "apple", target_os = "freebsd"))]
mod kevent {
    use libc::c_int;

    /// A count of `kevent` records: each value is the length of the one record, or
    /// of none, that its parameter is given.
    #[repr(i32)]
    pub(super) enum Count {
        Zero = 0,
        One = 1,
    }

    /// No value: a null-only event list.
    pub(super) enum Nothing {}

    // libc's. It reads the `changes` count of records at `change`, which is one, and
    // writes at most the `count` given for `events`, which is none; a null timeout
    // returns at once. It keeps nothing, so it is declared `safe`.
    unsafe extern "C" {
        pub(super) safe fn kevent(
            kq: c_int,
            change: &libc::kevent,
            changes: Count,
            events: Option<&'static mut Nothing>,
            count: Count,
            timeout: Option<&libc::timespec>,
        ) -> c_int;
    }
}

/// `if_nametoindex(3)`: the interface's index, 0 with errno set when there is none.
#[must_use]
pub fn if_nametoindex(name: &CStr) -> libc::c_uint {
    c::if_nametoindex(name.into())
}

/// An `ioctl(2)` request on a socket that reads and writes one `ifreq`, and no
/// memory it points to.
#[derive(Clone, Copy)]
pub enum InterfaceRequest {
    /// `SIOCGIFADDR`: the interface's address.
    Address,
    /// `SIOCGIFFLAGS`: its `IFF_*` flags.
    #[cfg(target_os = "linux")]
    Flags,
    /// `SIOCGIFHWADDR`: its hardware address.
    #[cfg(target_os = "linux")]
    HardwareAddress,
    /// `SIOCGIFNAME`: the name of the interface `ifru_ifindex` numbers.
    #[cfg(target_os = "linux")]
    Name,
}

/// `ioctl(fd, request, value)`: -1 with errno set on failure.
pub fn interface_ioctl(fd: c_int, request: InterfaceRequest, value: &mut libc::ifreq) -> c_int {
    // The request numbers as `ioctl` takes them (an int on musl).
    #[cfg(target_os = "linux")]
    let request: libc::Ioctl = match request {
        InterfaceRequest::Address => 0x8915,
        InterfaceRequest::Flags => 0x8913,
        InterfaceRequest::HardwareAddress => 0x8927,
        InterfaceRequest::Name => 0x8910,
    };
    #[cfg(not(target_os = "linux"))]
    let request = match request {
        InterfaceRequest::Address => libc::SIOCGIFADDR,
    };
    // SAFETY: each request reads and writes only the one live ifreq.
    unsafe { libc::ioctl(fd, request, std::ptr::from_mut(value)) }
}

/// The head of a list `getifaddrs` stored, not yet freed. Only `getifaddrs` makes one
/// (nothing here constructs it) and only `freeifaddrs` takes it, by value, so each list
/// is freed at most once.
#[repr(transparent)]
struct InterfaceList(NonNull<libc::ifaddrs>);

impl InterfaceList {
    /// The list's first entry, borrowed while the list is held.
    fn head(&self) -> &libc::ifaddrs {
        // SAFETY: an unfreed list's head (the type's invariant), live while it is held.
        unsafe { self.0.as_ref() }
    }
}

/// The host's interface addresses (`getifaddrs(3)`), freed on drop.
pub struct Interfaces(Option<InterfaceList>);

/// One entry of an `Interfaces` list.
#[derive(Clone, Copy)]
pub struct Interface<'a>(&'a libc::ifaddrs);

impl Interfaces {
    /// The list, or `None` with errno set.
    #[must_use]
    pub fn new() -> Option<Self> {
        let mut list = None;
        (c::getifaddrs(&mut list) == 0).then_some(Self(list))
    }

    /// The entries, in the list's order.
    pub fn iter(&self) -> impl Iterator<Item = Interface<'_>> {
        std::iter::successors(self.0.as_ref().map(InterfaceList::head), |entry| {
            // SAFETY: null or the list's next entry, which lives as long as `self`.
            unsafe { entry.ifa_next.as_ref() }
        })
        .map(Interface)
    }
}

impl Drop for Interfaces {
    fn drop(&mut self) {
        if let Some(list) = self.0.take() {
            c::freeifaddrs(list);
        }
    }
}

impl<'a> Interface<'a> {
    /// `ifa_name`.
    #[must_use]
    pub fn name(self) -> &'a CStr {
        // SAFETY: interface names are NUL-terminated and live as long as the list.
        unsafe { CStr::from_ptr(self.0.ifa_name) }
    }

    /// `ifa_flags` (`IFF_*`).
    #[must_use]
    pub fn flags(self) -> libc::c_uint {
        self.0.ifa_flags
    }

    /// `ifa_addr`'s header, when the interface has an address: its family says
    /// what follows it.
    #[must_use]
    pub fn address(self) -> Option<&'a libc::sockaddr> {
        // SAFETY: null or an address that lives as long as the list.
        unsafe { self.0.ifa_addr.as_ref() }
    }

    /// The address, when it is an `AF_INET` or `AF_INET6` one.
    #[must_use]
    pub fn ip(self) -> Option<std::net::IpAddr> {
        let address = self.address()?;
        match c_int::from(address.sa_family) {
            libc::AF_INET => {
                // A sockaddr_in fits the header: its `sin_addr` follows the family and
                // the two bytes of `sin_port`, so it is `sa_data[2..6]`.
                let octets = address.sa_data.get(2..6)?.first_chunk::<4>()?;
                Some(
                    octets
                        .map(|octet| u8::from_ne_bytes(octet.to_ne_bytes()))
                        .into(),
                )
            }
            libc::AF_INET6 => {
                let whole = self.0.ifa_addr.cast::<libc::sockaddr_in6>();
                // SAFETY: an AF_INET6 address is a sockaddr_in6, longer than the header
                // `address` borrows, so it is read through the list's own pointer.
                let inet6 = unsafe { whole.read_unaligned() };
                Some(inet6.sin6_addr.s6_addr.into())
            }
            _ => None,
        }
    }
}

/// `access(2)`.
#[must_use]
pub fn access(path: &CStr, mode: c_int) -> c_int {
    c::access(path.into(), mode)
}

/// `chdir(2)`.
#[must_use]
pub fn chdir(path: &CStr) -> c_int {
    c::chdir(path.into())
}

/// `chroot(2)`.
#[must_use]
pub fn chroot(path: &CStr) -> c_int {
    c::chroot(path.into())
}

/// The id of the group named `name` (`getgrnam_r(3)`), or none if there is no such
/// group or the lookup fails. The entry is this call's own, so no other lookup, on
/// this thread or another, can change it before its id is read.
#[must_use]
pub fn group_id(name: &CStr) -> Option<libc::gid_t> {
    let mut entry: libc::group = zeroed();
    lookup_id(|strings, found| {
        // SAFETY: a terminated name, an entry and `strings.len()` writable bytes for
        // its strings, and the slot for the result, all live for the call.
        unsafe {
            libc::getgrnam_r(
                name.as_ptr(),
                &raw mut entry,
                strings.as_mut_ptr(),
                strings.len(),
                found,
            )
        }
    })
    .then_some(entry.gr_gid)
}

/// The id of the user named `name` (`getpwnam_r(3)`), as [`group_id`].
#[must_use]
pub fn user_id(name: &CStr) -> Option<libc::uid_t> {
    let mut entry: libc::passwd = zeroed();
    lookup_id(|strings, found| {
        // SAFETY: as `group_id`'s.
        unsafe {
            libc::getpwnam_r(
                name.as_ptr(),
                &raw mut entry,
                strings.as_mut_ptr(),
                strings.len(),
                found,
            )
        }
    })
    .then_some(entry.pw_uid)
}

/// Runs a `get*nam_r` lookup, doubling the room for the entry's strings while it
/// reports `ERANGE`: whether it found an entry.
fn lookup_id<T>(mut lookup: impl FnMut(&mut [libc::c_char], &mut *mut T) -> c_int) -> bool {
    let mut strings = vec![0; 1024];
    loop {
        let mut found = std::ptr::null_mut();
        let status = lookup(&mut strings, &mut found);
        if status != libc::ERANGE || strings.len() >= 1 << 20 {
            return status == 0 && !found.is_null();
        }
        strings.resize(strings.len() * 2, 0);
    }
}

/// `mkdir(2)`.
#[must_use]
pub fn mkdir(path: &CStr, mode: libc::mode_t) -> c_int {
    c::mkdir(path.into(), mode)
}

/// `strndup(3)`: at most `length` bytes of `text` in a `malloc` allocation the
/// caller frees, or null.
#[must_use]
pub(crate) fn strndup(text: &CStr, length: usize) -> *mut libc::c_char {
    c::strndup(text.into(), length)
}

/// `strncasecmp(3)` of at most `count` bytes, in the current locale.
#[must_use]
pub(crate) fn strncasecmp(left: &CStr, right: &CStr, count: usize) -> c_int {
    c::strncasecmp(left.into(), right.into(), count)
}

/// `strtoul(3)` of `text` in `base`: the value (errno as libc leaves it) and the
/// text from where the number ended.
#[must_use]
pub(crate) fn strtoul(text: &CStr, base: c_int) -> (libc::c_ulong, &CStr) {
    let mut end = text.as_ptr().cast_mut();
    let value = c::strtoul(text.into(), &mut end, base);
    // strtoul ends inside the text, at the latest on its NUL.
    let parsed = end.addr().wrapping_sub(text.as_ptr().addr());
    let rest = text.to_bytes_with_nul().get(parsed..).unwrap_or(&[0]);
    (value, CStr::from_bytes_until_nul(rest).unwrap_or_default())
}

/// `stat(2)`: the path's status, or `None` with errno set.
#[must_use]
pub fn stat(path: &CStr) -> Option<libc::stat> {
    let mut stat = zeroed::<libc::stat>();
    (c::stat(path.into(), &mut stat) == 0).then_some(stat)
}

/// `sysconf(3)`.
#[must_use]
pub fn sysconf(name: c_int) -> libc::c_long {
    c::sysconf(name)
}

/// `kill(2)`.
pub fn kill(pid: pid_t, signo: c_int) -> c_int {
    // The signal is the caller's intent.
    c::kill(pid, signo)
}

/// `getpriority(2)`: -1 is a priority too, so the caller clears errno first.
#[must_use]
pub fn getpriority(which: c_int, who: libc::id_t) -> c_int {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    let which = which.cast_unsigned();
    c::getpriority(which, who)
}

/// `setpriority(2)`.
pub fn setpriority(which: c_int, who: libc::id_t, priority: c_int) -> c_int {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    let which = which.cast_unsigned();
    c::setpriority(which, who, priority)
}

/// `setitimer(2)`, discarding the previous timer.
pub fn setitimer(which: c_int, value: &libc::itimerval) -> c_int {
    c::setitimer(which, value, None)
}

/// `localtime_r(3)`: `time`'s calendar in the local zone, None when it has none.
#[must_use]
pub fn localtime_r(time: libc::time_t) -> Option<libc::tm> {
    let mut tm = zeroed::<libc::tm>();
    (!c::localtime_r(&time, &mut tm).is_null()).then_some(tm)
}

/// `strftime(3)` of `tm` into `out`: the bytes written, 0 when they don't fit.
pub fn strftime(out: &mut [u8], format: &CStr, tm: &libc::tm) -> usize {
    // SAFETY: `out` is writable for its length, `format` is NUL-terminated and `tm` is live.
    unsafe { libc::strftime(out.as_mut_ptr().cast(), out.len(), format.as_ptr(), tm) }
}

/// `waitpid(2)`, storing the status in `status` when given.
pub fn waitpid(pid: pid_t, status: Option<&mut c_int>, options: c_int) -> pid_t {
    c::waitpid(pid, status, options)
}

/// `getpid(2)`.
#[must_use]
pub fn getpid() -> pid_t {
    // A pid fits pid_t; std calls getpid(2), which has no preconditions.
    std::process::id().cast_signed()
}

/// The kernel's id for the calling thread.
#[must_use]
#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn gettid() -> libc::c_long {
    // SAFETY: SYS_gettid takes no arguments.
    unsafe { libc::syscall(libc::SYS_gettid) }
}

/// The kernel's id for the calling thread.
#[must_use]
#[cfg(target_vendor = "apple")]
pub fn gettid() -> libc::c_long {
    let mut id = 0_u64;
    // A null thread means the caller.
    c::pthread_threadid_np(0, &mut id);
    id.cast_signed()
}

/// `clock_gettime(2)` without its status, as a caller that reads a clock every
/// platform has (the timer's): the clock's time, or zero where the call fails.
#[inline]
#[must_use]
pub fn clock_time(clock: libc::clockid_t) -> libc::timespec {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    c::clock_gettime(clock, &mut now);
    now
}

/// `clock_gettime(2)`: the clock's time, or `None` with errno set.
#[must_use]
pub fn clock_gettime(clock: libc::clockid_t) -> Option<libc::timespec> {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    (c::clock_gettime(clock, &mut now) == 0).then_some(now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_calls_round_trip_through_a_pipe() {
        let mut fds = [0; 2];
        // SAFETY: `fds` holds two descriptors.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let [reader, writer] = fds;
        assert_eq!(write(writer, b"abc"), 3);
        let copy = dup(reader);
        assert!(copy >= 0);
        assert_eq!(fcntl(copy, libc::F_SETFL, libc::O_NONBLOCK), 0);
        assert_ne!(fcntl(copy, libc::F_GETFL, 0) & libc::O_NONBLOCK, 0);
        let mut buf = [0; 8];
        assert_eq!(read(copy, &mut buf), 3);
        assert_eq!(&buf[..3], b"abc");
        assert_eq!(read(copy, &mut buf), -1);
        assert_eq!(errno(), libc::EAGAIN);
        set_errno(0);
        assert_eq!(errno(), 0);
        for fd in [reader, writer, copy] {
            assert_eq!(close(fd), 0);
        }
        // Not `copy` again: a test on another thread may already hold that number.
        assert_eq!(close(-1), -1);
        assert_eq!(errno(), libc::EBADF);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no strerror")]
    fn strerror_is_libc_message() {
        for code in [libc::ENOENT, libc::EAGAIN, libc::EBADF, 0, 9999] {
            // SAFETY: strerror returns a NUL-terminated message for any code.
            let libc = unsafe { CStr::from_ptr(libc::strerror(code)) };
            assert_eq!(strerror(code), libc.to_str().unwrap(), "code {code}");
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no socket options or MSG_PEEK")]
    fn socket_calls_round_trip_through_a_pair() {
        let [left, right] = socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0).unwrap();
        assert_eq!(
            setsockopt::<c_int>(left, libc::SOL_SOCKET, libc::SO_KEEPALIVE, &1),
            0
        );
        let mut kind: c_int = 0;
        let length = getsockopt(left, libc::SOL_SOCKET, libc::SO_TYPE, &mut kind);
        assert_eq!(
            (length, kind),
            (Some(socklen::<c_int>()), libc::SOCK_STREAM)
        );
        assert_eq!(send(left, b"abc", 0), 3);
        let mut buf = [0_u8; 8];
        assert_eq!(recv(right, &mut buf, libc::MSG_PEEK), 3);
        assert_eq!(recv(right, &mut buf, 0), 3);
        assert_eq!(&buf[..3], b"abc");
        assert_eq!(close(left), 0);
        assert_eq!(close(right), 0);
        assert!(socketpair(-1, 0, 0).is_none());
        assert!(getsockopt(-1, libc::SOL_SOCKET, libc::SO_TYPE, &mut kind).is_none());
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no kill, if_nametoindex or getifaddrs")]
    fn paths_and_plain_data() {
        let zero: libc::sockaddr_storage = zeroed();
        assert_eq!(zero.ss_family, 0);
        assert_eq!(zeroed::<[u8; 4]>(), [0; 4]);
        assert!(stat(c"/").is_some_and(|root| root.st_mode & libc::S_IFMT == libc::S_IFDIR));
        assert!(stat(c"/no/such/path").is_none());
        assert_eq!(access(c"/", libc::R_OK), 0);
        assert!(sysconf(libc::_SC_PAGESIZE) > 0);
        assert_eq!(kill(getpid(), 0), 0);
        assert_eq!(if_nametoindex(c"no-such-interface"), 0);
        let interfaces = Interfaces::new().unwrap();
        assert!(interfaces.iter().all(|entry| !entry.name().is_empty()));
        let loopback = Some(std::net::Ipv4Addr::LOCALHOST.into());
        assert!(interfaces.iter().any(|entry| entry.ip() == loopback));
    }

    #[test]
    fn process_and_clock() {
        assert_eq!(getpid().unsigned_abs(), std::process::id());
        assert!(clock_gettime(libc::CLOCK_MONOTONIC).is_some());
        assert!(clock_gettime(libc::clockid_t::MAX).is_none());
    }
}
