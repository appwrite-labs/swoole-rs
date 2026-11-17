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

use crate::lent::{self, Drain, Fill};

/// A NUL-terminated string that stays live and unchanged for `'a`, as thin as the
/// `const char *` it is, so a C function's string parameter can be declared as one on
/// either side of the boundary.
///
/// Invariant: it points at a NUL-terminated string live for `'a`. Rust makes one only
/// from a `&'a CStr`. The other source is a parameter, declared static or declared
/// return value of this type on the C boundary, where the C side's `const char *`
/// contract stands for the constructor, as it does for a `&T` there.
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

/// An open stdio stream (`FILE *`) native code lends to one call: only C mints one,
/// as a parameter of an export typed as this, where its caller's open stream stands
/// for the constructor, as for [`Text`]. Not `Copy`: the call it is lent to takes it,
/// so a close leaves no handle behind.
#[repr(transparent)]
pub struct OpenFile(NonNull<libc::FILE>);

// SAFETY: stdio locks a stream inside each call (POSIX `flockfile`), so the call it is
// lent to may run on another thread.
unsafe impl Send for OpenFile {}

/// An open directory stream (`DIR *`) native code lends to one call, as [`OpenFile`].
#[repr(transparent)]
pub struct OpenDir(NonNull<libc::DIR>);

// SAFETY: a directory stream belongs to no thread; the one call it is lent to may run
// on another while its lender waits.
unsafe impl Send for OpenDir {}

/// An argument list as `execv` reads it: strings up to the null that ends the list.
///
/// Invariant: it points at a slice of `Option<Text>`s, each `None` a null pointer,
/// whose last is `None`, live for `'a`. Only [`execv`] makes one, from such a slice.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct Argv<'a>(
    NonNull<Option<Text<'a>>>,
    PhantomData<&'a [Option<Text<'a>>]>,
);

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

    use super::Argv;
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
        /// Reads `path` and the strings of `argv` up to its null; returns only on failure.
        pub(crate) safe fn execv(path: Text<'_>, argv: Argv<'_>) -> c_int;
        pub safe fn malloc(size: usize) -> *mut c_void;
        pub safe fn calloc(count: usize, size: usize) -> *mut c_void;
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
        /// `gettid(2)`: the calling thread's kernel id (glibc 2.30, musl 1.2.2, bionic).
        #[cfg(any(target_os = "linux", target_os = "android"))]
        pub(crate) safe fn gettid() -> pid_t;
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
        /// `epoll_create1(2)`: the call Miri implements.
        #[cfg(any(target_os = "linux", target_os = "android"))]
        pub safe fn epoll_create1(flags: c_int) -> c_int;
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
pub use c::{calloc, malloc};
#[cfg(any(target_os = "linux", target_os = "android"))]
pub use c::{epoll_create, epoll_create1, epoll_ctl, signalfd};
pub(crate) use c::{fork, getrlimit, srandom, uname};
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
    libc::epoll_event,
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
    let message = text.trim_suffix(&format!(" (os error {code})")).len();
    text.truncate(message);
    text
}

/// `read(2)` into `buf`: the bytes read, or `None` with errno set.
pub fn read(fd: c_int, buf: &mut [impl ReadSlot]) -> Option<usize> {
    Fill::new(buf).read(fd)
}

/// `write(2)` of `buf`: the bytes written, or `None` with errno set.
pub fn write(fd: c_int, buf: &[u8]) -> Option<usize> {
    Drain::new(buf).write(fd)
}

/// `pread(2)` into `buf` at `offset`.
pub fn pread(fd: c_int, buf: &mut [impl ReadSlot], offset: off_t) -> Option<usize> {
    Fill::new(buf).pread(fd, offset)
}

/// `pwrite(2)` of `buf` at `offset`.
pub fn pwrite(fd: c_int, buf: &[u8], offset: off_t) -> Option<usize> {
    Drain::new(buf).pwrite(fd, offset)
}

/// `close(2)`.
///
/// # Errors
/// The syscall's errno.
pub fn close(fd: c_int) -> std::io::Result<()> {
    status_result(c::close(fd))
}

/// `writev(2)` of `vectors`.
#[must_use]
#[inline]
pub fn writev(fd: c_int, vectors: &[std::io::IoSlice<'_>]) -> Option<usize> {
    usize::try_from(lent::lend_vectors(vectors, |iov, count| {
        lent::writev(fd, iov, count)
    }))
    .ok()
}

/// `readv(2)` into `vectors`' buffers, in order.
#[must_use]
#[inline]
pub fn readv(fd: c_int, vectors: &mut [lent::ReadVec<'_>]) -> Option<usize> {
    usize::try_from(lent::lend_out(vectors, |iov, count| {
        lent::readv(fd, iov, count)
    }))
    .ok()
}

/// `listen(2)`.
///
/// # Errors
/// The syscall's errno.
pub fn listen(fd: c_int, backlog: c_int) -> std::io::Result<()> {
    status_result(c::listen(fd, backlog))
}

/// `socket(2)`.
/// # Errors
/// The syscall's errno.
pub fn socket(domain: c_int, kind: c_int, protocol: c_int) -> std::io::Result<c_int> {
    nonnegative_result(c::socket(domain, kind, protocol))
}

/// `shutdown(2)`.
///
/// # Errors
/// The syscall's errno.
pub fn shutdown(fd: c_int, how: c_int) -> std::io::Result<()> {
    status_result(c::shutdown(fd, how))
}

/// `getsockopt(2)` into `value`: the length the kernel stored.
///
/// # Errors
/// The syscall's errno; `value` keeps what it held when the call fails.
pub fn getsockopt<T: Pod>(
    fd: c_int,
    level: c_int,
    name: c_int,
    value: &mut T,
) -> std::io::Result<libc::socklen_t> {
    let (status, length) = lent::lend_value_out(value, |at, length| {
        lent::getsockopt(fd, level, name, at, length)
    });
    status_result(status)?;
    Ok(length)
}

/// `setsockopt(2)` of `value`.
///
/// # Errors
/// The syscall's errno.
pub fn setsockopt<T: Pod>(fd: c_int, level: c_int, name: c_int, value: &T) -> std::io::Result<()> {
    let status = lent::lend_value(value, |at, length| {
        lent::setsockopt(fd, level, name, at, length)
    });
    status_result(status)
}

/// `setsockopt(2)` of an option given as bytes.
///
/// # Errors
/// The syscall's errno.
pub fn setsockopt_bytes(fd: c_int, level: c_int, name: c_int, value: &[u8]) -> std::io::Result<()> {
    let status = lent::lend_in(value, |at, length| {
        lent::setsockopt(fd, level, name, at.bytes(), length)
    });
    status_result(status)
}

/// Capture a syscall's errno, retaining its nonnegative descriptor, flags or count.
fn nonnegative_result(fd: c_int) -> std::io::Result<c_int> {
    if fd < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(fd)
    }
}

/// Capture a status syscall's errno before another operation can overwrite it.
fn status_result(status: c_int) -> std::io::Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// `socketpair(2)`: both ends, or `None` with errno set.
#[must_use]
pub fn socketpair(domain: c_int, kind: c_int, protocol: c_int) -> Option<[c_int; 2]> {
    let mut fds = [0; 2];
    (c::socketpair(domain, kind, protocol, &mut fds) == 0).then_some(fds)
}

/// `recv(2)` into `buf`.
pub fn recv(fd: c_int, buf: &mut [impl ReadSlot], flags: c_int) -> Option<usize> {
    Fill::new(buf).recv(fd, flags)
}

/// `send(2)` of `buf`.
pub fn send(fd: c_int, buf: &[u8], flags: c_int) -> Option<usize> {
    Drain::new(buf).send(fd, flags)
}

/// `dup(2)`.
/// # Errors
/// The syscall's errno.
pub fn dup(fd: c_int) -> std::io::Result<c_int> {
    nonnegative_result(c::dup(fd))
}

/// `dup2(2)`.
/// # Errors
/// The syscall's errno.
pub fn dup2(from: c_int, to: c_int) -> std::io::Result<c_int> {
    nonnegative_result(c::dup2(from, to))
}

/// Commands whose argument is an integer or absent: the only ones [`fcntl`] passes on.
#[cfg(not(target_os = "linux"))]
const INTEGER_COMMANDS: [c_int; 6] = [
    libc::F_GETFL,
    libc::F_SETFL,
    libc::F_GETFD,
    libc::F_SETFD,
    libc::F_DUPFD,
    libc::F_DUPFD_CLOEXEC,
];
#[cfg(target_os = "linux")]
const INTEGER_COMMANDS: [c_int; 8] = [
    libc::F_GETFL,
    libc::F_SETFL,
    libc::F_GETFD,
    libc::F_SETFD,
    libc::F_DUPFD,
    libc::F_DUPFD_CLOEXEC,
    libc::F_GETPIPE_SZ,
    libc::F_SETPIPE_SZ,
];

/// `fcntl(2)` with an integer argument (`F_GETFL`, `F_SETFL`, `F_GETFD`,
/// `F_SETFD`, `F_DUPFD`, ...); commands that take no argument ignore it. Any other
/// command (one whose argument is a pointer, such as `F_GETLK`) fails with `EINVAL`
/// without calling libc.
/// # Errors
/// The syscall's captured errno, or `EINVAL` for a command outside the integer list.
pub fn fcntl(fd: c_int, cmd: c_int, arg: c_int) -> std::io::Result<c_int> {
    /// A command from `INTEGER_COMMANDS`, made only after the check below.
    #[repr(transparent)]
    struct IntegerCommand(c_int);
    // An integer command reads its argument as an int, or not at all, and
    // dereferences nothing.
    unsafe extern "C" {
        #[link_name = "fcntl"]
        safe fn integer_fcntl(fd: c_int, cmd: IntegerCommand, ...) -> c_int;
    }
    if !INTEGER_COMMANDS.contains(&cmd) {
        return Err(std::io::Error::from_raw_os_error(libc::EINVAL));
    }
    nonnegative_result(integer_fcntl(fd, IntegerCommand(cmd), arg))
}

/// `flock(2)`.
///
/// # Errors
/// The syscall's errno.
pub fn flock(fd: c_int, operation: c_int) -> std::io::Result<()> {
    status_result(c::flock(fd, operation))
}

/// `poll(2)` of `fds`: the number ready, or zero on timeout.
/// # Errors
/// The syscall's captured errno.
pub fn poll(fds: &mut [libc::pollfd], timeout: c_int) -> std::io::Result<c_int> {
    nonnegative_result(lent::lend_out(fds, |fds, count| {
        lent::poll(fds, count, timeout)
    }))
}

/// `epoll_wait(2)` into `events`, at most all of them: the number ready, or zero on
/// timeout.
/// # Errors
/// The syscall's captured errno (`EINVAL` for no room).
#[cfg(any(target_os = "linux", target_os = "android"))]
#[inline]
pub fn epoll_wait(
    epfd: c_int,
    events: &mut [libc::epoll_event],
    timeout: c_int,
) -> std::io::Result<c_int> {
    nonnegative_result(lent::lend_out(events, |events, count| {
        lent::epoll_wait(epfd, events, count, timeout)
    }))
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
///
/// # Errors
/// The syscall's errno.
pub fn usleep(microseconds: libc::useconds_t) -> std::io::Result<()> {
    status_result(c::usleep(microseconds))
}

/// `nanosleep(2)` for `request`, the time left going to `remaining`.
///
/// # Errors
/// The syscall's errno.
pub fn nanosleep(request: &libc::timespec, remaining: &mut libc::timespec) -> std::io::Result<()> {
    status_result(c::nanosleep(request, Some(remaining)))
}

/// `mmap(2)` of a fresh anonymous read-write mapping of `len` bytes, `MAP_SHARED`
/// across a fork when `shared`, else `MAP_PRIVATE`; `stack` adds `MAP_STACK` where it
/// is required (OpenBSD). Zeroed. Null, with errno set, when it fails. A fresh mapping
/// at an address the kernel picks aliases nothing; unmapping it is the caller's.
#[must_use]
pub fn map_anonymous(len: usize, shared: bool, stack: bool) -> *mut libc::c_void {
    use anonymous::Flags;
    // Miri maps only private memory, and with no fork a shared mapping is no different.
    let shared = shared && !cfg!(miri);
    #[cfg(target_os = "openbsd")]
    let flags = match (shared, stack) {
        (true, false) => Flags::SHARED,
        (false, false) => Flags::PRIVATE,
        (true, true) => Flags::SHARED_STACK,
        (false, true) => Flags::PRIVATE_STACK,
    };
    #[cfg(not(target_os = "openbsd"))]
    let flags = {
        _ = stack;
        if shared {
            Flags::SHARED
        } else {
            Flags::PRIVATE
        }
    };
    let protection = anonymous::Protection::READ_WRITE;
    let mapped = anonymous::mmap(None, len, protection, flags, anonymous::NoFile::FD, 0);
    if mapped == libc::MAP_FAILED {
        std::ptr::null_mut()
    } else {
        mapped
    }
}

/// `mmap(2)` declared for fresh anonymous mappings only.
mod anonymous {
    /// No value: a null-only address hint.
    pub(super) enum Nothing {}

    // Transparent over `c_int` (an enum's ABI is not guaranteed to match), with the
    // field private so only the constants below exist.

    /// `PROT_READ | PROT_WRITE`.
    #[repr(transparent)]
    pub(super) struct Protection(libc::c_int);

    impl Protection {
        pub(super) const READ_WRITE: Self = Self(libc::PROT_READ | libc::PROT_WRITE);
    }

    /// An anonymous mapping's flags, never `MAP_FIXED`.
    #[repr(transparent)]
    pub(super) struct Flags(libc::c_int);

    impl Flags {
        pub(super) const PRIVATE: Self = Self(libc::MAP_PRIVATE | libc::MAP_ANON);
        pub(super) const SHARED: Self = Self(libc::MAP_SHARED | libc::MAP_ANON);
        #[cfg(target_os = "openbsd")]
        pub(super) const PRIVATE_STACK: Self =
            Self(libc::MAP_PRIVATE | libc::MAP_ANON | libc::MAP_STACK);
        #[cfg(target_os = "openbsd")]
        pub(super) const SHARED_STACK: Self =
            Self(libc::MAP_SHARED | libc::MAP_ANON | libc::MAP_STACK);
    }

    /// The descriptor an anonymous mapping takes: none.
    #[repr(transparent)]
    pub(super) struct NoFile(libc::c_int);

    impl NoFile {
        pub(super) const FD: Self = Self(-1);
    }

    // libc's. With no address and no `MAP_FIXED` the kernel places the mapping where
    // nothing is mapped, so it replaces no memory; it reads none of the caller's and
    // keeps nothing. So it is declared `safe`.
    unsafe extern "C" {
        pub(super) safe fn mmap(
            address: Option<&'static Nothing>,
            len: usize,
            protection: Protection,
            flags: Flags,
            fd: NoFile,
            offset: libc::off_t,
        ) -> *mut libc::c_void;
    }
}

/// `open(2)` of `path`: the new descriptor. `mode` applies when `flags` has `O_CREAT`.
///
/// # Errors
/// The syscall's errno.
pub fn open(path: &CStr, flags: c_int, mode: libc::mode_t) -> std::io::Result<c_int> {
    // `open(2)` reads only the path, and its one optional argument as a mode.
    #[expect(
        suspicious_runtime_symbol_definitions,
        reason = "`Text` is a transparent `const char *` that is never null"
    )]
    unsafe extern "C" {
        #[link_name = "open"]
        safe fn libc_open(path: Text<'_>, flags: c_int, ...) -> c_int;
    }
    // The mode is promoted as C does.
    nonnegative_result(libc_open(path.into(), flags, libc::c_uint::from(mode)))
}

/// `execv(3)`: runs `path` in place of this process, with `argv` as its arguments,
/// which end with `None` (C's null). Returns only on failure, with errno set by
/// `execv`, or as it was when `argv` doesn't end with `None`. Allocates nothing,
/// so a forked child may call it.
pub fn execv(path: &CStr, argv: &[Option<Text<'_>>]) {
    if matches!(argv.last(), Some(None)) {
        c::execv(path.into(), Argv(NonNull::from(argv).cast(), PhantomData));
    }
}

/// `unlink(2)`.
///
/// # Errors
/// The syscall's errno.
pub fn unlink(path: &CStr) -> std::io::Result<()> {
    status_result(c::unlink(path.into()))
}

/// `bytes` to C's `stdout`, which native code's own `printf` shares, so the two
/// keep their order.
pub fn print(bytes: &[u8]) {
    let stdout = crate::Stream::stdout();
    for chunk in bytes.chunks(c_int::MAX.unsigned_abs() as usize) {
        // What `printf("%.*s", length, chunk)` writes: the chunk up to its first NUL.
        stdout.write(chunk.split(|&byte| byte == 0).next().unwrap_or_default());
    }
}

/// One change to the kqueue `kq`, no events out.
///
/// # Errors
/// `kevent(2)`'s errno.
#[cfg(any(target_vendor = "apple", target_os = "freebsd"))]
pub fn kevent_change(kq: c_int, change: &libc::kevent) -> std::io::Result<()> {
    nonnegative_result(lent::lend_out(&mut [], |events, count| {
        kevent::kevent(kq, change, kevent::Changes::One, events, count, None)
    }))
    .map(drop)
}

/// No change to the kqueue `kq`, and its ready events into `events`, at most all of
/// them, waiting up to `timeout` (`None`: until one comes): the number ready.
///
/// # Errors
/// `kevent(2)`'s errno.
#[cfg(any(target_vendor = "apple", target_os = "freebsd"))]
#[inline]
pub fn kevent_wait(
    kq: c_int,
    events: &mut [libc::kevent],
    timeout: Option<&libc::timespec>,
) -> std::io::Result<c_int> {
    nonnegative_result(lent::lend_out(events, |events, count| {
        kevent::kevent(kq, &zeroed(), kevent::Changes::Zero, events, count, timeout)
    }))
}

/// `kevent(2)` declared for at most one change.
#[cfg(any(target_vendor = "apple", target_os = "freebsd"))]
mod kevent {
    use crate::lent::{Count, Out};
    use libc::c_int;

    /// How many `kevent` records the change parameter is given: none, or the one.
    #[repr(i32)]
    pub(super) enum Changes {
        Zero = 0,
        One = 1,
    }

    // libc's. It reads the `changes` count of records at `change`, and writes at most
    // the lending's `count` events into `events`; with no room it returns once the
    // change is made, else it waits up to `timeout` (null: until an event comes). It
    // keeps nothing, so it is declared `safe`.
    unsafe extern "C" {
        pub(super) safe fn kevent<'id>(
            kq: c_int,
            change: &libc::kevent,
            changes: Changes,
            events: Out<'id, libc::kevent>,
            count: Count<'id, c_int>,
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

/// A `struct ifreq` as its bytes, which the kernel copies in and out whole:
/// the interface's name, then the `ifr_ifru` union a request reads or fills,
/// whose fields are read from the bytes rather than through the union.
#[derive(Clone, Copy)]
pub struct Ifreq([u8; size_of::<libc::ifreq>()]);

impl Default for Ifreq {
    fn default() -> Self {
        Self([0; size_of::<libc::ifreq>()])
    }
}

impl Ifreq {
    /// A request on the interface `name`: its first `IFNAMSIZ - 1` bytes,
    /// NUL-terminated.
    #[must_use]
    pub fn named(name: &[u8]) -> Self {
        let mut request = Self::default();
        for (slot, &byte) in request.0.iter_mut().zip(name).take(libc::IFNAMSIZ - 1) {
            *slot = byte;
        }
        request
    }

    /// `ifr_name`, up to its NUL within `IFNAMSIZ`.
    #[must_use]
    pub fn name(&self) -> &CStr {
        let name = self.0.get(..libc::IFNAMSIZ).unwrap_or_default();
        CStr::from_bytes_until_nul(name).unwrap_or_default()
    }

    /// `ifr_ifru`'s bytes.
    fn value(&self) -> &[u8] {
        self.0.get(libc::IFNAMSIZ..).unwrap_or_default()
    }

    /// Sets `ifru_ifindex`, the interface `SIOCGIFNAME` names.
    pub fn set_index(&mut self, index: c_int) {
        let value = self.0.get_mut(libc::IFNAMSIZ..).unwrap_or_default();
        if let Some(slot) = value.first_chunk_mut() {
            *slot = index.to_ne_bytes();
        }
    }

    /// `((struct sockaddr_in *) &ifru_addr)->sin_addr`, as `SIOCGIFADDR`
    /// fills it: the four bytes after the family and port on every platform.
    #[must_use]
    pub fn ipv4(&self) -> libc::in_addr {
        let s_addr = match *self.value() {
            [_, _, _, _, a, b, c, d, ..] => u32::from_ne_bytes([a, b, c, d]),
            _ => 0,
        };
        libc::in_addr { s_addr }
    }

    /// `ifru_hwaddr.sa_data`, as `SIOCGIFHWADDR` fills it: the fourteen bytes
    /// after the two of the family.
    #[must_use]
    pub fn sa_data(&self) -> [u8; 14] {
        let data = self.value().get(2..).unwrap_or_default();
        data.first_chunk().copied().unwrap_or_default()
    }
}

/// `ioctl(fd, request, value)`.
///
/// # Errors
/// The syscall's errno.
pub fn interface_ioctl(
    fd: c_int,
    request: InterfaceRequest,
    value: &mut Ifreq,
) -> std::io::Result<()> {
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
    // SAFETY: each request copies in and out only the one ifreq's bytes,
    // which `value` holds, at any alignment.
    nonnegative_result(unsafe { libc::ioctl(fd, request, value.0.as_mut_ptr()) }).map(drop)
}

/// The head of a list `getifaddrs` stored, not yet freed. Only `getifaddrs` makes one
/// (nothing here constructs it) and only `freeifaddrs` takes it, by value, so each list
/// is freed at most once.
#[repr(transparent)]
struct InterfaceList(NonNull<Entry<'static>>);

impl InterfaceList {
    /// The list's first entry, borrowed while the list is held.
    fn head(&self) -> &Entry<'_> {
        // SAFETY: an unfreed list's head (the type's invariant), live while it is held.
        unsafe { self.0.as_ref() }
    }
}

/// `struct ifaddrs`'s leading fields, as glibc, musl and the BSDs (Apple's) lay them
/// out: an entry of a list `getifaddrs` stored, whose fields are live while the list is.
#[repr(C)]
struct Entry<'a> {
    /// `ifa_next`: the list's next entry, none at its end.
    next: Option<&'a Entry<'a>>,
    /// `ifa_name`.
    name: Text<'a>,
    /// `ifa_flags`.
    flags: libc::c_uint,
    /// `ifa_addr`: null, or an address as long as its family says.
    addr: *mut libc::sockaddr,
}

const _: () = {
    use std::mem::offset_of;
    assert!(
        offset_of!(Entry<'_>, next) == offset_of!(libc::ifaddrs, ifa_next)
            && offset_of!(Entry<'_>, name) == offset_of!(libc::ifaddrs, ifa_name)
            && offset_of!(Entry<'_>, flags) == offset_of!(libc::ifaddrs, ifa_flags)
            && offset_of!(Entry<'_>, addr) == offset_of!(libc::ifaddrs, ifa_addr)
            && size_of::<Entry<'_>>() <= size_of::<libc::ifaddrs>(),
        "`Entry` is `struct ifaddrs`'s prefix"
    );
};

/// The host's interface addresses (`getifaddrs(3)`), freed on drop.
pub struct Interfaces(Option<InterfaceList>);

/// One entry of an `Interfaces` list.
#[derive(Clone, Copy)]
pub struct Interface<'a>(&'a Entry<'a>);

impl Interfaces {
    /// The list, or `None` with errno set.
    #[must_use]
    pub fn new() -> Option<Self> {
        // Miri has no getifaddrs: an empty list, as a host with no interface reads.
        if cfg!(miri) {
            return Some(Self(None));
        }
        let mut list = None;
        (c::getifaddrs(&mut list) == 0).then_some(Self(list))
    }

    /// The entries, in the list's order.
    pub fn iter(&self) -> impl Iterator<Item = Interface<'_>> {
        std::iter::successors(self.0.as_ref().map(InterfaceList::head), |entry| entry.next)
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
        self.0.name.get()
    }

    /// `ifa_flags` (`IFF_*`).
    #[must_use]
    pub fn flags(self) -> libc::c_uint {
        self.0.flags
    }

    /// `ifa_addr`'s header, when the interface has an address: its family says
    /// what follows it.
    #[must_use]
    pub fn address(self) -> Option<&'a libc::sockaddr> {
        // SAFETY: null or an address that lives as long as the list.
        unsafe { self.0.addr.as_ref() }
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
                let whole = self.0.addr.cast::<libc::sockaddr_in6>();
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
///
/// # Errors
/// The syscall's errno.
pub fn access(path: &CStr, mode: c_int) -> std::io::Result<()> {
    status_result(c::access(path.into(), mode))
}

/// `chdir(2)`.
///
/// # Errors
/// The syscall's errno.
pub fn chdir(path: &CStr) -> std::io::Result<()> {
    status_result(c::chdir(path.into()))
}

/// `chroot(2)`.
///
/// # Errors
/// The syscall's errno.
pub fn chroot(path: &CStr) -> std::io::Result<()> {
    status_result(c::chroot(path.into()))
}

/// The id of the group named `name` (`getgrnam_r(3)`), or none if there is no such
/// group or the lookup fails. The entry is this call's own, so no other lookup, on
/// this thread or another, can change it before its id is read.
#[must_use]
pub fn group_id(name: &CStr) -> Option<libc::gid_t> {
    let mut entry: libc::group = zeroed();
    lookup_id(|strings, found| {
        lent::lend_out(strings, |at, size| {
            lent::getgrnam_r(name.into(), &mut entry, at, size, found)
        })
    })
    .then_some(entry.gr_gid)
}

/// The id of the user named `name` (`getpwnam_r(3)`), as [`group_id`].
#[must_use]
pub fn user_id(name: &CStr) -> Option<libc::uid_t> {
    let mut entry: libc::passwd = zeroed();
    lookup_id(|strings, found| {
        lent::lend_out(strings, |at, size| {
            lent::getpwnam_r(name.into(), &mut entry, at, size, found)
        })
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
///
/// # Errors
/// The syscall's errno.
pub fn mkdir(path: &CStr, mode: libc::mode_t) -> std::io::Result<()> {
    status_result(c::mkdir(path.into(), mode))
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
pub fn strtoul(text: &CStr, base: c_int) -> (libc::c_ulong, &CStr) {
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
///
/// # Errors
/// The syscall's errno.
pub fn kill(pid: pid_t, signo: c_int) -> std::io::Result<()> {
    // The signal is the caller's intent.
    status_result(c::kill(pid, signo))
}

/// `getpriority(2)`: -1 is a priority too, so the caller clears errno first.
#[must_use]
pub fn getpriority(which: c_int, who: libc::id_t) -> c_int {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    let which = which.cast_unsigned();
    c::getpriority(which, who)
}

/// `setpriority(2)`.
///
/// # Errors
/// The syscall's errno.
pub fn setpriority(which: c_int, who: libc::id_t, priority: c_int) -> std::io::Result<()> {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    let which = which.cast_unsigned();
    status_result(c::setpriority(which, who, priority))
}

/// `setitimer(2)`, discarding the previous timer.
///
/// # Errors
/// The syscall's errno.
pub fn setitimer(which: c_int, value: &libc::itimerval) -> std::io::Result<()> {
    status_result(c::setitimer(which, value, None))
}

/// `localtime_r(3)`: `time`'s calendar in the local zone, None when it has none.
#[must_use]
pub fn localtime_r(time: libc::time_t) -> Option<libc::tm> {
    let mut tm = zeroed::<libc::tm>();
    (!c::localtime_r(&time, &mut tm).is_null()).then_some(tm)
}

/// `strftime(3)` of `tm` into `out`: the bytes written, 0 when they don't fit.
pub fn strftime(out: &mut [u8], format: &CStr, tm: &libc::tm) -> usize {
    lent::lend_out(out, |at, max| {
        lent::strftime(at.chars(), max, format.into(), tm)
    })
}

/// `waitpid(2)`, storing the status in `status` when given. `None` means
/// `WNOHANG` found no child ready to reap.
///
/// # Errors
/// The syscall's captured errno.
pub fn waitpid(
    pid: pid_t,
    status: Option<&mut c_int>,
    options: c_int,
) -> std::io::Result<Option<pid_t>> {
    nonnegative_result(c::waitpid(pid, status, options)).map(|child| (child != 0).then_some(child))
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
    libc::c_long::from(c::gettid())
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
    fn execv_refuses_a_list_without_its_end() {
        // Each returns instead of running the program.
        execv(c"/bin/true", &[Some(c"true".into())]);
        execv(c"/bin/true", &[]);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri can't call execv")]
    fn execv_returns_its_failure() {
        // A path that doesn't exist fails in execv itself.
        execv(c"/nonexistent/swoole-rs", &[None]);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ENOENT)
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri can't spawn a process")]
    fn waitpid_distinguishes_pending_child_and_exit() {
        use std::io::Write;
        use std::process::{Command, Stdio};

        let mut child = Command::new("/bin/sh")
            .args(["-c", "read line; exit 7"])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        let pid = libc::pid_t::try_from(child.id()).unwrap();
        let mut status = 123;
        assert_eq!(
            waitpid(pid, Some(&mut status), libc::WNOHANG).unwrap(),
            None
        );
        assert_eq!(status, 123);
        child.stdin.take().unwrap().write_all(b"done\n").unwrap();
        assert_eq!(waitpid(pid, Some(&mut status), 0).unwrap(), Some(pid));
        assert!(libc::WIFEXITED(status));
        assert_eq!(libc::WEXITSTATUS(status), 7);
        assert_eq!(
            waitpid(pid, None, 0).unwrap_err().raw_os_error(),
            Some(libc::ECHILD)
        );
        // The syscall above already reaped it; the owning handle cannot wait twice.
        assert_eq!(child.wait().unwrap_err().raw_os_error(), Some(libc::ECHILD));
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri can't wait for a process")]
    fn waitpid_captures_errors_without_changing_status() {
        let mut status = 123;
        let error = waitpid(getpid(), Some(&mut status), libc::WNOHANG).unwrap_err();
        set_errno(libc::EINTR);
        assert_eq!(error.raw_os_error(), Some(libc::ECHILD));
        assert_eq!(status, 123);
    }

    #[test]
    fn descriptor_errors_survive_errno_changes() {
        let copy_error = dup(-1).unwrap_err();
        let [reader, writer] = pipe().unwrap();
        let replace_error = dup2(-1, writer).unwrap_err();
        set_errno(libc::EINTR);
        assert_eq!(copy_error.raw_os_error(), Some(libc::EBADF));
        assert_eq!(replace_error.raw_os_error(), Some(libc::EBADF));
        // A failed dup2 leaves the target open, including when source is invalid.
        assert_eq!(write(writer, b"x"), Some(1));
        let mut byte = [0];
        assert_eq!(read(reader, &mut byte), Some(1));
        assert_eq!(byte, *b"x");
        assert_eq!(dup2(writer, writer).unwrap(), writer);
        close(reader).unwrap();
        close(writer).unwrap();
    }

    #[test]
    fn fcntl_errors_keep_errno_and_success_keeps_flags() {
        let error = fcntl(-1, libc::F_GETFL, 0).unwrap_err();
        set_errno(libc::EINTR);
        assert_eq!(error.raw_os_error(), Some(libc::EBADF));
        let [reader, writer] = pipe().unwrap();
        let flags = fcntl(reader, libc::F_GETFL, 0).unwrap();
        assert_eq!(
            fcntl(reader, libc::F_SETFL, flags | libc::O_NONBLOCK).unwrap(),
            0
        );
        assert_eq!(
            fcntl(reader, libc::F_GETFL, 0).unwrap(),
            flags | libc::O_NONBLOCK
        );
        let copy = fcntl(reader, libc::F_DUPFD, 0).unwrap();
        assert_ne!(copy, reader);
        assert_eq!(
            fcntl(copy, libc::F_GETFL, 0).unwrap(),
            flags | libc::O_NONBLOCK
        );
        let failed = fcntl(reader, -1, 0).unwrap_err();
        set_errno(libc::ENOENT);
        assert_eq!(failed.raw_os_error(), Some(libc::EINVAL));
        assert_eq!(
            fcntl(reader, libc::F_GETFL, 0).unwrap(),
            flags | libc::O_NONBLOCK
        );
        for fd in [reader, writer, copy] {
            close(fd).unwrap();
        }
    }

    #[test]
    fn poll_distinguishes_timeout_readiness_and_invalid_descriptors() {
        let [reader, writer] = pipe().unwrap();
        let mut event = libc::pollfd {
            fd: reader,
            events: libc::POLLIN,
            revents: 0,
        };
        assert_eq!(poll(std::slice::from_mut(&mut event), 0).unwrap(), 0);
        assert_eq!(event.revents, 0);
        assert_eq!(write(writer, b"x"), Some(1));
        assert_eq!(poll(std::slice::from_mut(&mut event), 0).unwrap(), 1);
        assert_ne!(event.revents & libc::POLLIN, 0);
        assert_eq!((event.fd, event.events), (reader, libc::POLLIN));
        let mut invalid = [
            libc::pollfd {
                fd: c_int::MAX,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: -1,
                events: libc::POLLIN,
                revents: libc::POLLIN,
            },
        ];
        // An invalid descriptor is reported in revents, not as a syscall error;
        // negative descriptors are ignored and their revents cleared.
        assert_eq!(poll(&mut invalid, 0).unwrap(), 1);
        assert_eq!(invalid[0].revents, libc::POLLNVAL);
        assert_eq!(invalid[1].revents, 0);
        assert_eq!(poll(&mut [], 0).unwrap(), 0);
        close(reader).unwrap();
        close(writer).unwrap();
    }

    #[test]
    fn descriptor_calls_round_trip_through_a_pipe() {
        let mut fds = [0; 2];
        // SAFETY: `fds` holds two descriptors.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let [reader, writer] = fds;
        assert_eq!(write(writer, b"abc"), Some(3));
        let copy = dup(reader).unwrap_or(-1);
        assert!(copy >= 0);
        assert_eq!(
            fcntl(copy, libc::F_SETFL, libc::O_NONBLOCK).unwrap_or(-1),
            0
        );
        assert_ne!(
            fcntl(copy, libc::F_GETFL, 0).unwrap_or(-1) & libc::O_NONBLOCK,
            0
        );
        assert_eq!(
            fcntl(copy, libc::F_GETLK, 0).map_err(|e| e.raw_os_error()),
            Err(Some(libc::EINVAL))
        );
        let mut buf = [0; 8];
        assert_eq!(read(copy, &mut buf), Some(3));
        assert_eq!(&buf[..3], b"abc");
        assert_eq!(read(copy, &mut buf), None);
        assert_eq!(errno(), libc::EAGAIN);
        set_errno(0);
        assert_eq!(errno(), 0);
        for fd in [reader, writer, copy] {
            assert!(close(fd).is_ok());
        }
        // Not `copy` again: a test on another thread may already hold that number.
        assert!(close(-1).is_err());
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
    #[cfg_attr(miri, ignore = "Miri has no socket options")]
    fn socket_option_errors_keep_errno_and_failed_output() {
        let socket_error = socket(-1, libc::SOCK_STREAM, 0).unwrap_err();
        let socket_errno = errno();
        let descriptor = socket(libc::AF_UNIX, libc::SOCK_STREAM, 0).unwrap();
        close(descriptor).unwrap();
        set_errno(libc::EINTR);
        assert_eq!(socket_error.raw_os_error(), Some(socket_errno));
        assert_ne!(socket_errno, 0);
        let mut value: c_int = 73;
        let read = getsockopt(-1, libc::SOL_SOCKET, libc::SO_TYPE, &mut value).unwrap_err();
        let write = setsockopt(-1, libc::SOL_SOCKET, libc::SO_KEEPALIVE, &1_i32).unwrap_err();
        let bytes = setsockopt_bytes(
            -1,
            libc::SOL_SOCKET,
            libc::SO_KEEPALIVE,
            &1_i32.to_ne_bytes(),
        )
        .unwrap_err();
        set_errno(libc::ENOENT);
        assert_eq!(value, 73);
        for error in [read, write, bytes] {
            assert_eq!(error.raw_os_error(), Some(libc::EBADF));
        }
        let [left, right] = socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0).unwrap();
        setsockopt_bytes(
            left,
            libc::SOL_SOCKET,
            libc::SO_KEEPALIVE,
            &1_i32.to_ne_bytes(),
        )
        .unwrap();
        let length = getsockopt(left, libc::SOL_SOCKET, libc::SO_KEEPALIVE, &mut value).unwrap();
        // Linux reports 1, macOS the option's flag bit; both mean on.
        assert_eq!((length, value != 0), (socklen::<c_int>(), true));
        assert!(close(left).is_ok());
        assert!(close(right).is_ok());
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no socket options or MSG_PEEK")]
    fn socket_calls_round_trip_through_a_pair() {
        let [left, right] = socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0).unwrap();
        assert_eq!(
            setsockopt::<c_int>(left, libc::SOL_SOCKET, libc::SO_KEEPALIVE, &1).map_or(-1, |()| 0),
            0
        );
        let mut kind: c_int = 0;
        let length = getsockopt(left, libc::SOL_SOCKET, libc::SO_TYPE, &mut kind).ok();
        assert_eq!(
            (length, kind),
            (Some(socklen::<c_int>()), libc::SOCK_STREAM)
        );
        assert_eq!(send(left, b"abc", 0), Some(3));
        let mut buf = [0_u8; 8];
        assert_eq!(recv(right, &mut buf, libc::MSG_PEEK), Some(3));
        assert_eq!(recv(right, &mut buf, 0), Some(3));
        assert_eq!(&buf[..3], b"abc");
        assert!(close(left).is_ok());
        assert!(close(right).is_ok());
        assert!(socketpair(-1, 0, 0).is_none());
        assert!(
            getsockopt(-1, libc::SOL_SOCKET, libc::SO_TYPE, &mut kind)
                .ok()
                .is_none()
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no kill, if_nametoindex or getifaddrs")]
    fn paths_and_plain_data() {
        let zero: libc::sockaddr_storage = zeroed();
        assert_eq!(zero.ss_family, 0);
        assert_eq!(zeroed::<[u8; 4]>(), [0; 4]);
        assert!(stat(c"/").is_some_and(|root| root.st_mode & libc::S_IFMT == libc::S_IFDIR));
        assert!(stat(c"/no/such/path").is_none());
        assert_eq!(access(c"/", libc::R_OK).map_or(-1, |()| 0), 0);
        assert!(sysconf(libc::_SC_PAGESIZE) > 0);
        assert!(kill(getpid(), 0).is_ok());
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
