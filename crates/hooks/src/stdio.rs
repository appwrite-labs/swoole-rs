//! The hooked stdio, directory, file-system and resolver functions
//! (`swoole_coroutine_fopen`, `swoole_coroutine_stat`, `swoole_coroutine_getaddrinfo`,
//! …): inside a coroutine each libc call runs on the async thread pool while the
//! coroutine waits; outside one it runs here, as [`NATIVE`] decides. Native code
//! lends the `io_uring` engine and the hooked sockets' constructors in [`Host`].
//! The exports keep upstream's names.
use std::ffi::CStr;
use std::mem::MaybeUninit;
use std::sync::OnceLock;
use swoole_rs_corebase::lent::{Count, In, ItemSize, OneByte, Out, lend_in, lend_out};
use swoole_rs_corebase::sys::{OpenDir, OpenFile, Text};

use libc::{DIR, FILE, addrinfo, c_char, c_int, dirent, mode_t, off_t, size_t, ssize_t};

/// A hooked I/O operation failed or the pool refused to start it.
#[derive(Debug)]
pub enum IoError {
    /// The operation's captured OS error, on its executing thread.
    Io(std::io::Error),
    /// No operation ran; errno keeps the runtime's existing value.
    PoolRefused,
}

impl IoError {
    pub(crate) fn last_os_error() -> Self {
        Self::Io(std::io::Error::last_os_error())
    }
}

/// Runs `job` on the async pool and parks the coroutine until it is done; false if
/// the pool refused it.
pub type Offload = fn(&mut (dyn FnMut() + Send)) -> bool;

/// `call()` here without `offload`, else on the pool while this frame waits;
/// `refused` if the pool refuses.
pub fn on_pool<T: Send>(
    offload: Option<Offload>,
    refused: T,
    mut call: impl FnMut() -> T + Send,
) -> T {
    let Some(offload) = offload else {
        return call();
    };
    let mut result = None;
    offload(&mut || result = Some(call()));
    result.unwrap_or(refused)
}

/// [`on_pool`] for a `call` or result that isn't `Send`.
///
/// # Safety
/// `call` may run, and make its result, on a pool thread: it only touches what the
/// caller lent.
pub unsafe fn offloaded<T, F: FnMut() -> T>(offload: Option<Offload>, refused: T, call: F) -> T {
    /// A value the caller lets cross to the pool thread.
    struct Lent<T>(T);
    #[expect(
        clippy::non_send_fields_in_send_ty,
        reason = "Lent exists to carry what offloaded's caller promises may cross threads"
    )]
    // SAFETY: as the caller of `offloaded` promises.
    unsafe impl<T> Send for Lent<T> {}
    impl<F: FnMut() -> T, T> Lent<F> {
        fn run(&mut self) -> Lent<T> {
            Lent((self.0)())
        }
    }
    let mut call = Lent(call);
    on_pool(offload, Lent(refused), move || call.run()).0
}

/// The runtime the hooked functions use.
pub(crate) struct Ops {
    /// A reactor and a coroutine are running (hook.cc's `!is_no_coro()`).
    pub in_coroutine: fn() -> bool,
    /// `coroutine::async_job`.
    pub offload: Offload,
    /// Whether `System::sleep` completed without cancellation.
    pub sleep: fn(f64) -> bool,
    /// `swoole_set_last_error`.
    pub set_last_error: fn(c_int),
}

/// The coroutine runtime's.
pub(crate) static NATIVE: Ops = Ops {
    in_coroutine: native::in_coroutine,
    offload: swoole_rs_corebase::offload::async_job,
    sleep: crate::sleep::system_sleep,
    set_last_error: swoole_rs_corebase::log::set_last_error,
};

mod native {
    pub(super) fn in_coroutine() -> bool {
        !swoole_rs_corebase::thread_reactor().is_null()
            && !swoole_rs_corebase::coroutine::current().is_null()
    }
}

/// What the build decides of the hooked functions: the `io_uring` engine and
/// configure's `io_uring` switches. Native's `swoole_init` installs it
/// ([`swr_v1_hook_install`]) before any hook can run.
pub struct Host {
    /// The `io_uring` engine's host, when the build has one: then the file calls go
    /// to the ring.
    #[cfg(feature = "iouring")]
    pub uring: Option<&'static crate::uring::Host>,
    /// `HAVE_IOURING_STATX`: `fstat`, `stat` and `lstat` go to the ring too.
    pub uring_statx: bool,
    /// `HAVE_IOURING_FTRUNCATE`: so does `ftruncate`.
    pub uring_ftruncate: bool,
    /// `HAVE_IOURING_FUTEX`: the engine probes the futex opcodes.
    pub uring_futex: bool,
    /// `SW_USE_URING_SOCKET`: the sockets Rust makes are `UringSocket`s.
    pub uring_socket: bool,
}

static HOST: OnceLock<&'static Host> = OnceLock::new();

/// [`NATIVE`] once native installs the [`Host`]: [`offload`] reads it here, so a crate
/// that links hooks without the runtime (the PDO drivers' tests) never links it.
static RUNTIME: OnceLock<&'static Ops> = OnceLock::new();

/// Install the build's [`Host`] and the runtime: native's `swoole_init` calls it.
pub fn swr_v1_hook_install() {
    use swoole_rs_corebase::{Switch, built_with};
    static BUILT: OnceLock<Host> = OnceLock::new();
    #[cfg(all(feature = "iouring", feature = "upstream-symbols"))]
    let uring = Some(&crate::iouring::HOST);
    #[cfg(all(feature = "iouring", not(feature = "upstream-symbols")))]
    let uring = None;
    let host = BUILT.get_or_init(|| Host {
        #[cfg(feature = "iouring")]
        uring,
        uring_statx: built_with(Switch::IouringStatx),
        uring_ftruncate: built_with(Switch::IouringFtruncate),
        uring_futex: built_with(Switch::IouringFutex),
        uring_socket: built_with(Switch::UringSocket),
    });
    set_host(host, &NATIVE);
    #[cfg(all(feature = "iouring", feature = "upstream-symbols"))]
    swoole_rs_corebase::reactor::register_uring(&crate::iouring::REACTOR);
    crate::co_io::install_native();
}

/// Installs `host` and the runtime `ops` the hooked functions use from now on.
fn set_host(host: &'static Host, ops: &'static Ops) {
    HOST.get_or_init(|| host);
    RUNTIME.get_or_init(|| ops);
}

/// The installed runtime, or the coroutine runtime's before native installs one.
pub(crate) fn runtime() -> &'static Ops {
    RUNTIME.get().copied().unwrap_or(&NATIVE)
}

pub(crate) fn host() -> Option<&'static Host> {
    HOST.get().copied()
}

/// The host inside a coroutine (hook.cc's `!is_no_coro()`), else none.
pub(crate) fn coroutine() -> Option<&'static Host> {
    host().filter(|_| (runtime().in_coroutine)())
}

/// `async_job` inside a coroutine, else none: the call runs here.
#[must_use]
pub fn offload() -> Option<Offload> {
    RUNTIME
        .get()
        .filter(|ops| (ops.in_coroutine)())
        .map(|ops| ops.offload)
}

/// The ring's host inside a coroutine when `io_uring` takes this call (`gate`).
#[cfg(feature = "iouring")]
fn ring(gate: fn(&Host) -> bool) -> Option<&'static crate::uring::Host> {
    let uring = host().filter(|host| gate(host))?.uring?;
    (runtime().in_coroutine)().then_some(uring)
}

/// A hooked call's argument as it crosses to the pool thread: a string as the `&CStr`
/// it is, the rest as themselves.
trait Lend {
    type Sent: Send;
    fn lend(self) -> Self::Sent;
}

impl<'a> Lend for Text<'a> {
    type Sent = &'a CStr;
    fn lend(self) -> &'a CStr {
        self.get()
    }
}

impl<'a> Lend for Option<Text<'a>> {
    type Sent = Option<&'a CStr>;
    fn lend(self) -> Option<&'a CStr> {
        self.map(Text::get)
    }
}

impl<T: Send> Lend for &mut T {
    type Sent = Self;
    fn lend(self) -> Self {
        self
    }
}

macro_rules! as_itself {
    ($($type:ty),*) => {$(
        impl Lend for $type {
            type Sent = Self;
            fn lend(self) -> Self {
                self
            }
        }
    )*};
}
as_itself!(OpenFile, OpenDir, i32, u16, u32, i64);

impl<A: Lend> Lend for (A,) {
    type Sent = (A::Sent,);
    fn lend(self) -> Self::Sent {
        (self.0.lend(),)
    }
}

impl<A: Lend, B: Lend> Lend for (A, B) {
    type Sent = (A::Sent, B::Sent);
    fn lend(self) -> Self::Sent {
        (self.0.lend(), self.1.lend())
    }
}

impl<A: Lend, B: Lend, C: Lend> Lend for (A, B, C) {
    type Sent = (A::Sent, B::Sent, C::Sent);
    fn lend(self) -> Self::Sent {
        (self.0.lend(), self.1.lend(), self.2.lend())
    }
}

/// A hooked call's result as it crosses back: an integer as itself, a pointer as its
/// address, which only the caller's thread turns back into one.
trait Crossing {
    type Sent: Send;
    fn sent(self) -> Self::Sent;
    fn back(sent: Self::Sent) -> Self;
}

impl<T> Crossing for *mut T {
    type Sent = usize;
    fn sent(self) -> usize {
        self.expose_provenance()
    }
    fn back(address: usize) -> Self {
        std::ptr::with_exposed_provenance_mut(address)
    }
}

macro_rules! crosses_as_itself {
    ($($type:ty),*) => {$(
        impl Crossing for $type {
            type Sent = Self;
            fn sent(self) -> Self {
                self
            }
            fn back(sent: Self) -> Self {
                sent
            }
        }
    )*};
}
crosses_as_itself!(i32, i64, isize, usize);

/// `call` over `args` here outside a coroutine, else on the pool while this frame
/// waits; `refused` if the pool refuses.
fn lent<A: Lend, R: Crossing>(args: A, refused: R, call: impl FnOnce(A::Sent) -> R + Send) -> R {
    let mut lent = Some((args.lend(), call));
    let run = move || lent.take().map(|(args, call)| call(args).sent());
    on_pool(offload(), None, run).map_or(refused, R::back)
}

/// Each hooked function over the arguments C lends: `ring` names its `io_uring`
/// operation.
macro_rules! hooked {
    ($($name:ident($($arg:ident: $ty:ty),*) -> $ret:ty, refused $refused:expr
        $(, ring $gate:expr => $ring:ident($($ring_arg:expr),*) map $ring_map:expr)? => $call:expr;)*) => {$(
        #[doc = concat!("Hooked `", stringify!($call), "`: inside a coroutine it runs on the pool",
            $(", or as `", stringify!($ring), "` on `io_uring`",)? ".")]
        #[unsafe(no_mangle)]
        pub extern "C" fn $name($($arg: $ty),*) -> $ret {
            $(
                #[cfg(feature = "iouring")]
                if let Some(ring) = ring($gate) {
                    return crate::uring::$ring(ring, $($ring_arg),*).map_or(-1, $ring_map);
                }
            )?
            lent(($($arg,)*), $refused, |($($arg,)*)| $call)
        }
    )*};
}

hooked! {
    swoole_coroutine_fopen(pathname: Text<'_>, mode: Text<'_>) -> *mut FILE, refused std::ptr::null_mut() => fopen(pathname.into(), mode.into());
    swoole_coroutine_fdopen(fd: c_int, mode: Text<'_>) -> *mut FILE, refused std::ptr::null_mut() => fdopen(fd, mode.into());
    swoole_coroutine_freopen(pathname: Option<Text<'_>>, mode: Text<'_>, stream: OpenFile) -> *mut FILE, refused std::ptr::null_mut() => freopen(pathname.map(Text::from), mode.into(), stream);
    swoole_coroutine_fputs(s: Text<'_>, stream: OpenFile) -> c_int, refused -1 => fputs(s.into(), stream);
    swoole_coroutine_feof(stream: OpenFile) -> c_int, refused -1 => feof(stream);
    swoole_coroutine_fflush(stream: OpenFile) -> c_int, refused -1 => fflush(stream);
    swoole_coroutine_fclose(stream: OpenFile) -> c_int, refused -1 => fclose(stream);
    swoole_coroutine_opendir(name: Text<'_>) -> *mut DIR, refused std::ptr::null_mut() => opendir(name.into());
    swoole_coroutine_readdir(dirp: OpenDir) -> *mut dirent, refused std::ptr::null_mut() => readdir(dirp);
    swoole_coroutine_closedir(dirp: OpenDir) -> c_int, refused -1 => closedir(dirp);
    swoole_coroutine_open(pathname: Text<'_>, flags: c_int, mode: mode_t) -> c_int, refused -1,
        ring |_| true => open_file(pathname.get(), flags, mode) map std::convert::identity => swoole_rs_corebase::sys::open(pathname, flags, mode).unwrap_or(-1);
    swoole_coroutine_fstat(fd: c_int, statbuf: &mut libc::stat) -> c_int, refused -1,
        ring |host| host.uring_statx => fstat(fd, &mut *statbuf) map |()| 0 => fstat(fd, statbuf);
    swoole_coroutine_stat(path: Text<'_>, statbuf: &mut libc::stat) -> c_int, refused -1,
        ring |host| host.uring_statx => stat(path.get(), &mut *statbuf) map |()| 0 => stat(path.into(), statbuf);
    // Upstream's io_uring lstat is its stat.
    swoole_coroutine_lstat(path: Text<'_>, statbuf: &mut libc::stat) -> c_int, refused -1,
        ring |host| host.uring_statx => stat(path.get(), &mut *statbuf) map |()| 0 => lstat(path.into(), statbuf);
    swoole_coroutine_unlink(pathname: Text<'_>) -> c_int, refused -1,
        ring |_| true => unlink(pathname.get()) map |()| 0 => unlink(pathname.into());
    swoole_coroutine_mkdir(pathname: Text<'_>, mode: mode_t) -> c_int, refused -1,
        ring |_| true => mkdir(pathname.get(), mode) map |()| 0 => mkdir(pathname.into(), mode);
    swoole_coroutine_rmdir(pathname: Text<'_>) -> c_int, refused -1,
        ring |_| true => rmdir(pathname.get()) map |()| 0 => rmdir(pathname.into());
    swoole_coroutine_rename(oldpath: Text<'_>, newpath: Text<'_>) -> c_int, refused -1,
        ring |_| true => rename(oldpath.get(), newpath.get()) map |()| 0 => rename(oldpath.into(), newpath.into());
    swoole_coroutine_fsync(fd: c_int) -> c_int, refused -1,
        ring |_| true => fsync(fd) map |()| 0 => fsync(fd);
    swoole_coroutine_ftruncate(fd: c_int, length: off_t) -> c_int, refused -1,
        ring |host| host.uring_ftruncate => ftruncate(fd, length) map |()| 0 => ftruncate(fd, length);
    swoole_coroutine_lseek(fd: c_int, offset: off_t, whence: c_int) -> off_t, refused -1 => lseek(fd, offset, whence);
    swoole_coroutine_statvfs(path: Text<'_>, buf: &mut libc::statvfs) -> c_int, refused -1 => statvfs(path.into(), buf);
    swoole_coroutine_access(pathname: Text<'_>, mode: c_int) -> c_int, refused -1 => access(pathname.into(), mode);
    swoole_coroutine_fdatasync(fd: c_int) -> c_int, refused -1,
        ring |_| true => fdatasync(fd) map |()| 0 => SYNC_DATA(fd);
}

/// Hooked `fread`: inside a coroutine it runs on the pool.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_coroutine_fread<'id>(
    ptr: Out<'id, MaybeUninit<u8>>,
    size: ItemSize<'id>,
    nmemb: Count<'id>,
    stream: OpenFile,
) -> size_t {
    let mut fill = ptr.fill_items(&size, nmemb);
    let slots = fill.slots();
    let read = lent((stream,), 0, |(stream,)| {
        lend_out(slots, |at, count| {
            fread(at.bytes(), OneByte::SIZE, count, stream)
        })
    });
    read.checked_div(size.get()).unwrap_or(0)
}

/// Hooked `fwrite`: inside a coroutine it runs on the pool.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_coroutine_fwrite<'id>(
    ptr: In<'id, u8>,
    size: ItemSize<'id>,
    nmemb: Count<'id>,
    stream: OpenFile,
) -> size_t {
    let bytes = ptr.drain_items(&size, nmemb).bytes();
    let written = lent((stream,), 0, |(stream,)| {
        lend_in(bytes, |at, count| {
            fwrite(at.bytes(), OneByte::SIZE, count, stream)
        })
    });
    written.checked_div(size.get()).unwrap_or(0)
}

/// Hooked `fgets`: inside a coroutine it runs on the pool. `s`, or null.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_coroutine_fgets<'id>(
    s: Out<'id, MaybeUninit<u8>>,
    size: Count<'id, c_int>,
    stream: OpenFile,
) -> *mut c_char {
    let mut fill = s.fill(size);
    let slots = fill.slots();
    let at = slots.as_mut_ptr().cast::<c_char>();
    let read = lent((stream,), 0, |(stream,)| {
        let line = lend_out(slots, |at, count| fgets(at.chars(), count, stream));
        i32::from(!line.is_null())
    });
    if read == 0 { std::ptr::null_mut() } else { at }
}

/// Hooked `readlink`: inside a coroutine it runs on the pool.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_coroutine_readlink<'id>(
    pathname: Text<'_>,
    buf: Out<'id, MaybeUninit<u8>>,
    len: Count<'id>,
) -> ssize_t {
    let mut fill = buf.fill(len);
    let slots = fill.slots();
    lent((pathname,), -1, |(pathname,)| {
        lend_out(slots, |at, count| {
            readlink(pathname.into(), at.chars(), count)
        })
    })
}

/// `F_FULLFSYNC`, which takes no argument.
#[cfg(target_vendor = "apple")]
#[repr(transparent)]
struct FullFsync(c_int);

/// `fdatasync`, or upstream's macOS stand-in for it: `fcntl(F_FULLFSYNC)`.
#[cfg(target_vendor = "apple")]
const SYNC_DATA: fn(c_int) -> c_int = |fd| full_fsync(fd, FullFsync(libc::F_FULLFSYNC), 0);
#[cfg(not(target_vendor = "apple"))]
const SYNC_DATA: fn(c_int) -> c_int = |fd| fdatasync(fd);

// libc's. Each reads its integers and its `Text`s up to their NULs, writes at most the
// one object its reference names or its lent buffer's count, and uses the stream or
// directory C lent it, which it takes; it keeps none of them. So they are `safe`.
unsafe extern "C" {
    safe fn fopen(path: Text<'_>, mode: Text<'_>) -> *mut FILE;
    safe fn fdopen(fd: c_int, mode: Text<'_>) -> *mut FILE;
    safe fn freopen(path: Option<Text<'_>>, mode: Text<'_>, stream: OpenFile) -> *mut FILE;
    safe fn fread<'id>(buf: Out<'id>, size: OneByte, count: Count<'id>, stream: OpenFile)
    -> size_t;
    safe fn fwrite<'id>(buf: In<'id>, size: OneByte, count: Count<'id>, stream: OpenFile)
    -> size_t;
    safe fn fgets<'id>(
        s: Out<'id, c_char>,
        size: Count<'id, c_int>,
        stream: OpenFile,
    ) -> *mut c_char;
    safe fn fputs(s: Text<'_>, stream: OpenFile) -> c_int;
    safe fn feof(stream: OpenFile) -> c_int;
    safe fn fflush(stream: OpenFile) -> c_int;
    safe fn fclose(stream: OpenFile) -> c_int;
    #[cfg_attr(
        all(target_os = "macos", target_arch = "x86_64"),
        link_name = "opendir$INODE64"
    )]
    #[cfg_attr(target_os = "netbsd", link_name = "__opendir30")]
    safe fn opendir(name: Text<'_>) -> *mut DIR;
    #[cfg_attr(
        all(target_os = "macos", target_arch = "x86_64"),
        link_name = "readdir$INODE64"
    )]
    #[cfg_attr(target_os = "netbsd", link_name = "__readdir30")]
    safe fn readdir(dir: OpenDir) -> *mut dirent;
    safe fn closedir(dir: OpenDir) -> c_int;
    #[cfg_attr(
        all(target_os = "macos", target_arch = "x86_64"),
        link_name = "fstat$INODE64"
    )]
    #[cfg_attr(target_os = "netbsd", link_name = "__fstat50")]
    safe fn fstat(fd: c_int, status: &mut libc::stat) -> c_int;
    #[cfg_attr(
        all(target_os = "macos", target_arch = "x86_64"),
        link_name = "stat$INODE64"
    )]
    #[cfg_attr(target_os = "netbsd", link_name = "__stat50")]
    safe fn stat(path: Text<'_>, status: &mut libc::stat) -> c_int;
    #[cfg_attr(
        all(target_os = "macos", target_arch = "x86_64"),
        link_name = "lstat$INODE64"
    )]
    #[cfg_attr(target_os = "netbsd", link_name = "__lstat50")]
    safe fn lstat(path: Text<'_>, status: &mut libc::stat) -> c_int;
    safe fn unlink(path: Text<'_>) -> c_int;
    safe fn mkdir(path: Text<'_>, mode: mode_t) -> c_int;
    safe fn rmdir(path: Text<'_>) -> c_int;
    safe fn rename(old: Text<'_>, new: Text<'_>) -> c_int;
    safe fn fsync(fd: c_int) -> c_int;
    safe fn ftruncate(fd: c_int, length: off_t) -> c_int;
    safe fn lseek(fd: c_int, offset: off_t, whence: c_int) -> off_t;
    safe fn readlink<'id>(path: Text<'_>, buf: Out<'id, c_char>, size: Count<'id>) -> ssize_t;
    safe fn statvfs(path: Text<'_>, status: &mut libc::statvfs) -> c_int;
    safe fn access(path: Text<'_>, mode: c_int) -> c_int;
    #[cfg(not(target_vendor = "apple"))]
    safe fn fdatasync(fd: c_int) -> c_int;
    /// `fcntl(F_FULLFSYNC)`: flushes the descriptor and reads no argument.
    #[cfg(target_vendor = "apple")]
    #[link_name = "fcntl"]
    safe fn full_fsync(fd: c_int, cmd: FullFsync, ...) -> c_int;
}

/// Hooked `close` of an fd that is no coroutine socket, on the pool (`offload`).
///
/// # Errors
/// Captures a close failure on its executing thread, or reports pool refusal.
pub fn close_fd(fd: c_int, offload: Option<Offload>) -> Result<(), IoError> {
    on_pool(offload, Err(IoError::PoolRefused), || {
        swoole_rs_corebase::sys::close(fd).map_err(IoError::Io)
    })
}

/// Hooked `sleep`: `System::sleep` (fatal outside a coroutine, as upstream).
#[unsafe(no_mangle)]
pub extern "C" fn swoole_coroutine_sleep(sec: c_int) {
    (runtime().sleep)(f64::from(sec));
}

/// Hooked `usleep`, with upstream's 1024-based conversion.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_coroutine_usleep(usec: c_int) {
    (runtime().sleep)(f64::from(usec) / 1024.0 / 1024.0);
}

/// A resolver failure, kept distinct from refusal to run the lookup.
#[derive(Debug)]
pub enum AddrInfoError {
    /// The resolver's `EAI_*` code, other than `EAI_SYSTEM`.
    Resolver(c_int),
    /// `EAI_SYSTEM`, with errno captured on the resolving thread.
    System(std::io::Error),
    /// The pool refused the lookup; no resolver ran.
    PoolRefused,
}

/// Hooked `getaddrinfo`, always on the pool: upstream does not test for a coroutine.
/// A found list is stored in `pai` for the caller to `freeaddrinfo`.
///
/// # Errors
/// Returns the resolver error, capturing errno for `EAI_SYSTEM`, or pool refusal.
pub fn resolve_addrinfo(
    name: Option<&CStr>,
    service: Option<&CStr>,
    req: Option<&addrinfo>,
    pai: &mut *mut addrinfo,
) -> Result<(), AddrInfoError> {
    // getaddrinfo reads only the hints' integers; the rest must be zero.
    let hints = req.map(|req| {
        (
            req.ai_flags,
            req.ai_family,
            req.ai_socktype,
            req.ai_protocol,
        )
    });
    let call = || {
        let hints = hints.map(|(flags, family, socktype, protocol)| {
            Hints(addrinfo {
                ai_flags: flags,
                ai_family: family,
                ai_socktype: socktype,
                ai_protocol: protocol,
                ai_addrlen: 0,
                ai_addr: std::ptr::null_mut(),
                ai_canonname: std::ptr::null_mut(),
                ai_next: std::ptr::null_mut(),
            })
        });
        let (name, service) = (name.map(Text::from), service.map(Text::from));
        let mut list = std::ptr::null_mut();
        let found = match getaddrinfo(name, service, hints.as_ref(), &mut list) {
            0 => Ok(()),
            libc::EAI_SYSTEM => Err(AddrInfoError::System(std::io::Error::last_os_error())),
            code => Err(AddrInfoError::Resolver(code)),
        };
        // The list is the caller's to free on this thread: only its address crosses.
        (found, list.expose_provenance())
    };
    let (found, list) = on_pool(
        Some(runtime().offload),
        (Err(AddrInfoError::PoolRefused), 0),
        call,
    );
    // getaddrinfo stores a list only when it finds one.
    if list != 0 {
        *pai = std::ptr::with_exposed_provenance_mut(list);
    }
    found
}

/// `getaddrinfo`'s hints: integers, with every pointer null as getaddrinfo requires.
/// Only [`resolve_addrinfo`] makes one, so getaddrinfo reads no pointer
/// of it.
#[repr(transparent)]
struct Hints(addrinfo);

// libc's resolvers. Each reads its `Text`s and the hints' integers, writes at most
// the one slot a reference names and keeps nothing it is given, so they are `safe`.
unsafe extern "C" {
    /// Resolves `name` and `service` (none for none) under `hints`, storing a new
    /// list for the caller to `freeaddrinfo` in `list` when it finds one.
    safe fn getaddrinfo(
        name: Option<Text<'_>>,
        service: Option<Text<'_>>,
        hints: Option<&Hints>,
        list: &mut *mut addrinfo,
    ) -> c_int;
}

/// Resolve the first IPv4 address on the pool, copying it before the worker's
/// resolver storage expires. The lookup's `h_errno` (0 if refused) becomes this
/// thread's. No caller needs the old `hostent` aliases or borrowed address list.
#[must_use]
pub fn swoole_coroutine_gethostbyname(name: &CStr) -> Option<[c_char; 46]> {
    let call = || match swoole_rs_corebase::gethostbyname_result(libc::AF_INET, name) {
        Ok(address) => (Some(address), 0),
        Err(error) => (None, error),
    };
    let (address, error) = on_pool(Some(runtime().offload), (None, 0), call);
    swoole_rs_corebase::sys::set_h_errno(error);
    address
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use c::{
        swoole_coroutine_access, swoole_coroutine_closedir, swoole_coroutine_fclose,
        swoole_coroutine_feof, swoole_coroutine_fgets, swoole_coroutine_fopen,
        swoole_coroutine_fstat, swoole_coroutine_fwrite, swoole_coroutine_lstat,
        swoole_coroutine_mkdir, swoole_coroutine_open, swoole_coroutine_opendir,
        swoole_coroutine_readdir, swoole_coroutine_rename, swoole_coroutine_rmdir,
        swoole_coroutine_stat, swoole_coroutine_unlink,
    };
    use std::cell::Cell;
    use std::ffi::CString;

    /// The exports as C declares them, so the tests lend what C lends.
    mod c {
        use libc::{DIR, FILE, c_char, c_int, c_void, dirent, mode_t, size_t, stat};
        unsafe extern "C" {
            pub(super) fn swoole_coroutine_fopen(
                pathname: *const c_char,
                mode: *const c_char,
            ) -> *mut FILE;
            pub(super) fn swoole_coroutine_fwrite(
                ptr: *const c_void,
                size: size_t,
                nmemb: size_t,
                stream: *mut FILE,
            ) -> size_t;
            pub(super) fn swoole_coroutine_fgets(
                s: *mut c_char,
                size: c_int,
                stream: *mut FILE,
            ) -> *mut c_char;
            pub(super) fn swoole_coroutine_feof(stream: *mut FILE) -> c_int;
            pub(super) fn swoole_coroutine_fclose(stream: *mut FILE) -> c_int;
            pub(super) fn swoole_coroutine_opendir(name: *const c_char) -> *mut DIR;
            pub(super) fn swoole_coroutine_readdir(dirp: *mut DIR) -> *mut dirent;
            pub(super) fn swoole_coroutine_closedir(dirp: *mut DIR) -> c_int;
            pub(super) fn swoole_coroutine_open(
                pathname: *const c_char,
                flags: c_int,
                mode: mode_t,
            ) -> c_int;
            pub(super) fn swoole_coroutine_fstat(fd: c_int, statbuf: *mut stat) -> c_int;
            pub(super) fn swoole_coroutine_stat(path: *const c_char, statbuf: *mut stat) -> c_int;
            pub(super) fn swoole_coroutine_lstat(path: *const c_char, statbuf: *mut stat) -> c_int;
            pub(super) fn swoole_coroutine_unlink(pathname: *const c_char) -> c_int;
            pub(super) fn swoole_coroutine_mkdir(pathname: *const c_char, mode: mode_t) -> c_int;
            pub(super) fn swoole_coroutine_rmdir(pathname: *const c_char) -> c_int;
            pub(super) fn swoole_coroutine_rename(
                oldpath: *const c_char,
                newpath: *const c_char,
            ) -> c_int;
            pub(super) fn swoole_coroutine_access(pathname: *const c_char, mode: c_int) -> c_int;
        }
    }

    /// How the test host answers on this thread.
    #[derive(Clone, Copy, PartialEq)]
    pub(crate) enum Mode {
        /// No coroutine: calls run here.
        Here,
        /// A coroutine, and the pool runs the job on another thread.
        Pool,
        /// A coroutine, and the pool refuses the job.
        Refuse,
    }

    thread_local! {
        static MODE: Cell<Mode> = const { Cell::new(Mode::Here) };
        static OFFLOADED: Cell<u32> = const { Cell::new(0) };
        static SLEPT: Cell<f64> = const { Cell::new(0.0) };
        pub(crate) static LAST_ERROR: Cell<c_int> = const { Cell::new(0) };
    }

    fn in_coroutine() -> bool {
        MODE.get() != Mode::Here
    }
    fn pool(job: &mut (dyn FnMut() + Send)) -> bool {
        OFFLOADED.set(OFFLOADED.get() + 1);
        match MODE.get() {
            Mode::Refuse => return false,
            // Only the resolver offloads outside a coroutine: run it here.
            Mode::Here => {
                job();
                return true;
            }
            Mode::Pool => {}
        }
        std::thread::scope(|scope| scope.spawn(job).join().unwrap());
        true
    }
    fn sleep(seconds: f64) -> bool {
        SLEPT.set(seconds);
        true
    }
    pub(crate) const OPS: Ops = Ops {
        in_coroutine,
        offload: pool,
        sleep,
        set_last_error,
    };
    static TEST_HOST: Host = Host {
        #[cfg(feature = "iouring")]
        uring: None,
        uring_statx: false,
        uring_ftruncate: false,
        uring_futex: false,
        uring_socket: false,
    };
    fn set_last_error(error: c_int) {
        LAST_ERROR.set(error);
    }

    /// `call` under the test host in `mode`.
    pub(crate) fn with<T>(mode: Mode, call: impl FnOnce() -> T) -> T {
        set_host(&TEST_HOST, &OPS);
        MODE.set(mode);
        let result = call();
        MODE.set(Mode::Here);
        result
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no fopen")]
    fn runs_here_or_on_the_pool() {
        use Mode::{Here, Pool, Refuse};
        let path = std::env::temp_dir().join(format!("swr-stdio-{}", std::process::id()));
        let name = CString::new(path.to_str().unwrap()).unwrap();
        unsafe {
            let file = with(Pool, || {
                swoole_coroutine_fopen(name.as_ptr(), c"w".as_ptr())
            });
            assert!(!file.is_null());
            assert_eq!(
                with(Pool, || swoole_coroutine_fwrite(
                    b"line\n".as_ptr().cast(),
                    1,
                    5,
                    file
                )),
                5
            );
            assert_eq!(with(Here, || swoole_coroutine_fclose(file)), 0);

            let file = with(Here, || {
                swoole_coroutine_fopen(name.as_ptr(), c"r".as_ptr())
            });
            let mut buffer: [c_char; 16] = [0; 16];
            assert!(
                !with(Pool, || swoole_coroutine_fgets(
                    buffer.as_mut_ptr(),
                    16,
                    file
                ))
                .is_null()
            );
            assert_eq!(std::ffi::CStr::from_ptr(buffer.as_ptr()), c"line\n");
            assert_eq!(with(Pool, || swoole_coroutine_feof(file)), 0);
            assert_eq!(
                with(Pool, || swoole_coroutine_fgets(
                    buffer.as_mut_ptr(),
                    16,
                    file
                )),
                std::ptr::null_mut()
            );
            assert_ne!(with(Pool, || swoole_coroutine_feof(file)), 0);
            assert_eq!(with(Refuse, || swoole_coroutine_feof(file)), -1);
            assert_eq!(with(Pool, || swoole_coroutine_fclose(file)), 0);

            let dir = CString::new(std::env::temp_dir().to_str().unwrap()).unwrap();
            let dirp = with(Pool, || swoole_coroutine_opendir(dir.as_ptr()));
            assert!(!with(Pool, || swoole_coroutine_readdir(dirp)).is_null());
            assert_eq!(with(Pool, || swoole_coroutine_closedir(dirp)), 0);
            assert_eq!(
                with(Refuse, || swoole_coroutine_opendir(
                    c"/nonexistent-swr".as_ptr()
                )),
                std::ptr::null_mut()
            );
        }
        assert_eq!(OFFLOADED.get(), 12);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no getaddrinfo without a socket type hint")]
    fn resolver_and_close_on_the_pool() {
        use Mode::{Here, Pool, Refuse};
        unsafe {
            let mut list: *mut addrinfo = std::ptr::null_mut();
            let lookup = |service: Option<&CStr>, list: &mut *mut addrinfo| {
                resolve_addrinfo(Some(c"127.0.0.1"), service, None, list).map_or_else(
                    |error| match error {
                        AddrInfoError::Resolver(code) => code,
                        AddrInfoError::System(_) => libc::EAI_SYSTEM,
                        AddrInfoError::PoolRefused => -1,
                    },
                    |()| 0,
                )
            };
            // Upstream offloads the resolver even outside a coroutine.
            assert_eq!(with(Here, || lookup(Some(c"80"), &mut list)), 0);
            assert_eq!(OFFLOADED.get(), 1);
            assert!(!list.is_null());
            libc::freeaddrinfo(list);
            assert_eq!(with(Refuse, || lookup(None, &mut list)), -1);
            let before = list;
            assert!(matches!(
                with(Refuse, || resolve_addrinfo(
                    Some(c"127.0.0.1"),
                    None,
                    None,
                    &mut list
                )),
                Err(AddrInfoError::PoolRefused)
            ));
            assert_eq!(list, before);
            // AI_NUMERICHOST rejects this name locally, on the real pool thread.
            let mut hints: addrinfo = std::mem::zeroed();
            hints.ai_flags = libc::AI_NUMERICHOST;
            hints.ai_socktype = libc::SOCK_STREAM;
            assert!(matches!(
                with(Pool, || resolve_addrinfo(
                    Some(c"not-a-number"),
                    None,
                    Some(&hints),
                    &mut list
                )),
                Err(AddrInfoError::Resolver(libc::EAI_NONAME))
            ));
            assert_eq!(list, before);

            assert!(with(Pool, || swoole_coroutine_gethostbyname(c"127.0.0.1")).is_some());
            let entry = with(Here, || swoole_coroutine_gethostbyname(c"127.0.0.1")).unwrap();
            assert_eq!(&entry[..4], &[127, 0, 0, 1]);
            swoole_rs_corebase::sys::set_h_errno(3);
            assert!(with(Refuse, || swoole_coroutine_gethostbyname(c"localhost")).is_none());
            assert_eq!(swoole_rs_corebase::sys::h_errno(), 0);

            let mut fds = [0; 2];
            assert_eq!(libc::pipe(fds.as_mut_ptr()), 0);
            assert_eq!(close_fd(fds[0], Some(pool)).map_or(-1, |()| 0), 0);
            assert_eq!(close_fd(fds[1], None).map_or(-1, |()| 0), 0);
        }
    }

    #[test]
    fn close_errors_cross_the_pool_and_refusal_keeps_errno() {
        // This test pool runs on another thread without copying its errno back.
        swoole_rs_corebase::set_errno(libc::EINTR);
        let error = with(Mode::Pool, || close_fd(-1, Some(pool))).unwrap_err();
        assert!(matches!(error, IoError::Io(error) if error.raw_os_error() == Some(libc::EBADF)));
        assert_eq!(swoole_rs_corebase::sys::errno(), libc::EINTR);

        let error = with(Mode::Refuse, || close_fd(-1, Some(pool))).unwrap_err();
        assert!(matches!(error, IoError::PoolRefused));
        assert_eq!(swoole_rs_corebase::sys::errno(), libc::EINTR);
    }

    #[test]
    fn sleep_is_natives() {
        with(Mode::Pool, || swoole_coroutine_sleep(2));
        assert_eq!(SLEPT.get(), 2.0);
        with(Mode::Pool, || swoole_coroutine_usleep(1 << 19));
        assert_eq!(SLEPT.get(), 0.5);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no access")]
    fn file_system_calls_on_the_pool() {
        use Mode::{Here, Pool, Refuse};
        let base = std::env::temp_dir().join(format!("swr-fs-{}", std::process::id()));
        let (dir, moved) = (
            CString::new(base.to_str().unwrap()).unwrap(),
            CString::new(format!("{}-moved", base.to_str().unwrap())).unwrap(),
        );
        let file = CString::new(format!("{}-moved/file", base.to_str().unwrap())).unwrap();
        unsafe {
            assert_eq!(
                with(Pool, || swoole_coroutine_mkdir(dir.as_ptr(), 0o700)),
                0
            );
            assert_eq!(
                with(Pool, || swoole_coroutine_rename(
                    dir.as_ptr(),
                    moved.as_ptr()
                )),
                0
            );
            assert_eq!(
                with(Pool, || swoole_coroutine_access(moved.as_ptr(), libc::W_OK)),
                0
            );
            let fd = with(Pool, || {
                swoole_coroutine_open(file.as_ptr(), libc::O_CREAT | libc::O_WRONLY, 0o600)
            });
            assert!(fd >= 0);
            assert_eq!(with(Pool, || swoole_coroutine_ftruncate(fd, 9)), 0);
            assert_eq!(
                with(Pool, || swoole_coroutine_lseek(fd, 0, libc::SEEK_END)),
                9
            );
            assert_eq!(with(Pool, || swoole_coroutine_fdatasync(fd)), 0);
            assert_eq!(with(Pool, || swoole_coroutine_fsync(fd)), 0);
            let mut info: libc::stat = std::mem::zeroed();
            assert_eq!(with(Pool, || swoole_coroutine_fstat(fd, &raw mut info)), 0);
            assert_eq!(info.st_size, 9);
            assert_eq!(libc::close(fd), 0);
            assert_eq!(
                with(Here, || swoole_coroutine_stat(file.as_ptr(), &raw mut info)),
                0
            );
            assert_eq!(with(Pool, || swoole_coroutine_unlink(file.as_ptr())), 0);
            assert_eq!(
                with(Pool, || swoole_coroutine_lstat(
                    file.as_ptr(),
                    &raw mut info
                )),
                -1
            );
            assert_eq!(with(Pool, || swoole_coroutine_rmdir(moved.as_ptr())), 0);
            assert_eq!(with(Refuse, || swoole_coroutine_rmdir(moved.as_ptr())), -1);
        }
        assert_eq!(OFFLOADED.get(), 13);
    }
}
