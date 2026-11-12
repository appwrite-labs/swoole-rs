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
use std::mem::MaybeUninit;

/// A byte a read may overwrite with any value: `u8`, or one not yet written
/// (`CppString::spare`). Readers only write their buffer, so they take either.
pub trait ReadSlot: sealed::Sealed {}
impl ReadSlot for u8 {}
impl ReadSlot for MaybeUninit<u8> {}

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
    #[cfg(any(target_os = "linux", target_os = "android"))]
    // SAFETY: returns this thread's errno slot, live for the thread.
    let slot = unsafe { libc::__errno_location() };
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    // SAFETY: returns this thread's errno slot, live for the thread.
    let slot = unsafe { libc::__error() };
    // SAFETY: `slot` is this thread's errno.
    unsafe { slot.write(value) };
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
    // SAFETY: close(2) has no memory preconditions.
    unsafe { libc::close(fd) }
}

/// `socket(2)`.
#[must_use]
pub fn socket(domain: c_int, kind: c_int, protocol: c_int) -> c_int {
    // SAFETY: socket(2) has no memory preconditions.
    unsafe { libc::socket(domain, kind, protocol) }
}

/// `shutdown(2)`.
pub fn shutdown(fd: c_int, how: c_int) -> c_int {
    // SAFETY: shutdown(2) has no memory preconditions.
    unsafe { libc::shutdown(fd, how) }
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
    // SAFETY: `fds` has room for two descriptors.
    (unsafe { libc::socketpair(domain, kind, protocol, fds.as_mut_ptr()) } == 0).then_some(fds)
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
    // SAFETY: dup(2) has no memory preconditions.
    unsafe { libc::dup(fd) }
}

/// `dup2(2)`.
pub fn dup2(from: c_int, to: c_int) -> c_int {
    // SAFETY: dup2(2) has no memory preconditions.
    unsafe { libc::dup2(from, to) }
}

/// `fcntl(2)` with an integer argument (`F_GETFL`, `F_SETFL`, `F_GETFD`,
/// `F_SETFD`, `F_DUPFD`, ...); commands that take no argument ignore it.
pub fn fcntl(fd: c_int, cmd: c_int, arg: c_int) -> c_int {
    // SAFETY: an integer argument has no memory preconditions.
    unsafe { libc::fcntl(fd, cmd, arg) }
}

/// `flock(2)`.
pub fn flock(fd: c_int, operation: c_int) -> c_int {
    // SAFETY: flock(2) has no memory preconditions.
    unsafe { libc::flock(fd, operation) }
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
    // SAFETY: `fds` has room for two descriptors.
    (unsafe { libc::pipe(fds.as_mut_ptr()) } == 0).then_some(fds)
}

/// `fstat(2)`: the descriptor's status, or `None` with errno set.
#[must_use]
pub fn fstat(fd: c_int) -> Option<libc::stat> {
    let mut stat = zeroed::<libc::stat>();
    // SAFETY: `stat` is writable storage for one stat.
    (unsafe { libc::fstat(fd, &raw mut stat) } == 0).then_some(stat)
}

/// `sleep(3)`: the seconds left when a signal cut it short.
pub fn sleep(seconds: libc::c_uint) -> libc::c_uint {
    // SAFETY: sleep(3) has no memory preconditions.
    unsafe { libc::sleep(seconds) }
}

/// `usleep(3)`.
pub fn usleep(microseconds: libc::useconds_t) -> c_int {
    // SAFETY: usleep(3) has no memory preconditions.
    unsafe { libc::usleep(microseconds) }
}

/// `nanosleep(2)` for `request`, the time left going to `remaining`.
pub fn nanosleep(request: &libc::timespec, remaining: &mut libc::timespec) -> c_int {
    // SAFETY: both are live timespecs; nanosleep reads one and writes the other.
    unsafe { libc::nanosleep(request, remaining) }
}

/// `open(2)` of `path`; `mode` applies when `flags` has `O_CREAT`.
pub fn open(path: &CStr, flags: c_int, mode: libc::mode_t) -> c_int {
    // SAFETY: `path` is NUL-terminated; the mode is promoted as C does.
    unsafe { libc::open(path.as_ptr(), flags, libc::c_uint::from(mode)) }
}

/// `unlink(2)`.
pub fn unlink(path: &CStr) -> c_int {
    // SAFETY: `path` is NUL-terminated.
    unsafe { libc::unlink(path.as_ptr()) }
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
    let (none, no_wait) = (std::ptr::null_mut(), std::ptr::null());
    // SAFETY: one live change record, and no output records.
    unsafe { libc::kevent(kq, change, 1, none, 0, no_wait) }
}

/// `if_nametoindex(3)`: the interface's index, 0 with errno set when there is none.
#[must_use]
pub fn if_nametoindex(name: &CStr) -> libc::c_uint {
    // SAFETY: `name` is NUL-terminated.
    unsafe { libc::if_nametoindex(name.as_ptr()) }
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

/// The host's interface addresses (`getifaddrs(3)`), freed on drop.
pub struct Interfaces(*mut libc::ifaddrs);

/// One entry of an `Interfaces` list.
#[derive(Clone, Copy)]
pub struct Interface<'a>(&'a libc::ifaddrs);

impl Interfaces {
    /// The list, or `None` with errno set.
    #[must_use]
    pub fn new() -> Option<Self> {
        let mut list = std::ptr::null_mut();
        // SAFETY: a writable out-pointer.
        (unsafe { libc::getifaddrs(&raw mut list) } == 0).then_some(Self(list))
    }

    /// The entries, in the list's order.
    pub fn iter(&self) -> impl Iterator<Item = Interface<'_>> {
        // SAFETY: the list lives as long as `self`.
        std::iter::successors(unsafe { self.0.as_ref() }, |entry| {
            // SAFETY: as above.
            unsafe { entry.ifa_next.as_ref() }
        })
        .map(Interface)
    }
}

impl Drop for Interfaces {
    fn drop(&mut self) {
        // SAFETY: the list `getifaddrs` returned, freed once.
        unsafe { libc::freeifaddrs(self.0) };
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
                // SAFETY: an AF_INET address is a sockaddr_in.
                let inet = unsafe {
                    std::ptr::from_ref(address)
                        .cast::<libc::sockaddr_in>()
                        .read_unaligned()
                };
                Some(inet.sin_addr.s_addr.to_ne_bytes().into())
            }
            libc::AF_INET6 => {
                // SAFETY: an AF_INET6 address is a sockaddr_in6.
                let inet6 = unsafe {
                    std::ptr::from_ref(address)
                        .cast::<libc::sockaddr_in6>()
                        .read_unaligned()
                };
                Some(inet6.sin6_addr.s6_addr.into())
            }
            _ => None,
        }
    }
}

/// `access(2)`.
#[must_use]
pub fn access(path: &CStr, mode: c_int) -> c_int {
    // SAFETY: `path` is NUL-terminated.
    unsafe { libc::access(path.as_ptr(), mode) }
}

/// `chdir(2)`.
#[must_use]
pub fn chdir(path: &CStr) -> c_int {
    // SAFETY: `path` is NUL-terminated.
    unsafe { libc::chdir(path.as_ptr()) }
}

/// `stat(2)`: the path's status, or `None` with errno set.
#[must_use]
pub fn stat(path: &CStr) -> Option<libc::stat> {
    let mut stat = zeroed::<libc::stat>();
    // SAFETY: `path` is NUL-terminated and `stat` is writable storage for one stat.
    (unsafe { libc::stat(path.as_ptr(), &raw mut stat) } == 0).then_some(stat)
}

/// `sysconf(3)`.
#[must_use]
pub fn sysconf(name: c_int) -> libc::c_long {
    // SAFETY: sysconf(3) has no memory preconditions.
    unsafe { libc::sysconf(name) }
}

/// `kill(2)`.
pub fn kill(pid: pid_t, signo: c_int) -> c_int {
    // SAFETY: kill(2) has no memory preconditions; the signal is the caller's intent.
    unsafe { libc::kill(pid, signo) }
}

/// `setitimer(2)`, discarding the previous timer.
pub fn setitimer(which: c_int, value: &libc::itimerval) -> c_int {
    // SAFETY: `value` is a live itimerval, and a null old value is allowed.
    unsafe { libc::setitimer(which as _, value, std::ptr::null_mut()) }
}

/// `localtime_r(3)`: `time`'s calendar in the local zone, None when it has none.
#[must_use]
pub fn localtime_r(time: libc::time_t) -> Option<libc::tm> {
    let mut tm = zeroed::<libc::tm>();
    // SAFETY: `time` and `tm` are live locals; localtime_r writes only `tm`.
    (!unsafe { libc::localtime_r(&raw const time, &raw mut tm) }.is_null()).then_some(tm)
}

/// `strftime(3)` of `tm` into `out`: the bytes written, 0 when they don't fit.
pub fn strftime(out: &mut [u8], format: &CStr, tm: &libc::tm) -> usize {
    // SAFETY: `out` is writable for its length, `format` is NUL-terminated and `tm` is live.
    unsafe { libc::strftime(out.as_mut_ptr().cast(), out.len(), format.as_ptr(), tm) }
}

/// `waitpid(2)`, storing the status in `status` when given.
pub fn waitpid(pid: pid_t, status: Option<&mut c_int>, options: c_int) -> pid_t {
    let status = status.map_or(std::ptr::null_mut(), std::ptr::from_mut);
    // SAFETY: `status` is null or one writable int.
    unsafe { libc::waitpid(pid, status, options) }
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
    // SAFETY: a null thread means the caller; `id` is a live out-parameter.
    unsafe { libc::pthread_threadid_np(0, &raw mut id) };
    id.cast_signed()
}

/// `clock_gettime(2)`: the clock's time, or `None` with errno set.
#[must_use]
pub fn clock_gettime(clock: libc::clockid_t) -> Option<libc::timespec> {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `now` is writable storage for one timespec.
    (unsafe { libc::clock_gettime(clock, &raw mut now) } == 0).then_some(now)
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
        assert_eq!(close(copy), -1);
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
    }

    #[test]
    fn process_and_clock() {
        assert_eq!(getpid().unsigned_abs(), std::process::id());
        assert!(clock_gettime(libc::CLOCK_MONOTONIC).is_some());
        assert!(clock_gettime(libc::clockid_t::MAX).is_none());
    }
}
