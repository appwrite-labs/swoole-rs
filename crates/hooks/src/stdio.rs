//! The hooked stdio, directory, file-system and resolver functions
//! (`swoole_coroutine_fopen`, `swoole_coroutine_stat`, `swoole_coroutine_getaddrinfo`,
//! …): inside a coroutine each libc call runs on the async thread pool while the
//! coroutine waits; outside one it runs here, as [`NATIVE`] decides. Native code
//! lends the `io_uring` engine and the hooked sockets' constructors in [`Host`].
//! The exports keep upstream's names.
use std::ffi::CStr;
use std::sync::OnceLock;
use swoole_rs_corebase::sys::Text;

use libc::{
    DIR, FILE, addrinfo, c_char, c_int, c_uint, c_void, dirent, hostent, mode_t, off_t, size_t,
    ssize_t, stat, statvfs,
};

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
    offload: crate::offload::async_job,
    sleep: native::sleep,
    set_last_error: swoole_rs_corebase::log::set_last_error,
};

mod native {
    pub(super) fn in_coroutine() -> bool {
        !swoole_rs_corebase::thread_reactor().is_null() && !swoole_rs_coroutine::current().is_null()
    }

    pub(super) fn sleep(seconds: f64) -> bool {
        crate::sleep::system_sleep(seconds) == 0
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
    swoole_rs_reactor::register_uring(&crate::iouring::REACTOR);
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

/// Each hooked function: `ring` names its `io_uring` operation over arguments that
/// dereference the caller's pointers, `fd ring` one over plain values.
macro_rules! hooked {
    ($($name:ident($($arg:ident: $ty:ty),*) -> $ret:ty, refused $refused:expr
        $(, ring $gate:expr => $ring:ident($($ring_arg:expr),*) map $ring_map:expr)?
        $(, fd ring $fd_gate:expr => $fd_ring:ident($($fd_ring_arg:expr),*) map $fd_map:expr)? => $call:expr;)*) => {$(
        #[doc = concat!("Hooked `", stringify!($call), "`: inside a coroutine it runs on the pool",
            $(", or as `", stringify!($ring), "` on `io_uring`",)?
            $(", or as `", stringify!($fd_ring), "` on `io_uring`",)? ".")]
        ///
        /// # Safety
        /// The arguments are valid for the libc call.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name($($arg: $ty),*) -> $ret {
            $(
                #[cfg(feature = "iouring")]
                if let Some(ring) = ring($gate) {
                    // SAFETY: the caller's arguments are valid for the call.
                    return unsafe { crate::uring::$ring(ring, $($ring_arg),*) }.map_or(-1, $ring_map);
                }
            )?
            $(
                #[cfg(feature = "iouring")]
                if let Some(ring) = ring($fd_gate) {
                    return crate::uring::$fd_ring(ring, $($fd_ring_arg),*).map_or(-1, $fd_map);
                }
            )?
            // SAFETY: the caller's arguments are valid for the libc call.
            let call = || unsafe { $call };
            // SAFETY: `offload` is native `async` inside a coroutine, or none.
            unsafe { offloaded(offload(), $refused, call) }
        }
    )*};
}

hooked! {
    swoole_coroutine_fopen(pathname: *const c_char, mode: *const c_char) -> *mut FILE, refused std::ptr::null_mut() => libc::fopen(pathname, mode);
    swoole_coroutine_fdopen(fd: c_int, mode: *const c_char) -> *mut FILE, refused std::ptr::null_mut() => libc::fdopen(fd, mode);
    swoole_coroutine_freopen(pathname: *const c_char, mode: *const c_char, stream: *mut FILE) -> *mut FILE, refused std::ptr::null_mut() => libc::freopen(pathname, mode, stream);
    swoole_coroutine_fread(ptr: *mut c_void, size: size_t, nmemb: size_t, stream: *mut FILE) -> size_t, refused 0 => libc::fread(ptr, size, nmemb, stream);
    swoole_coroutine_fwrite(ptr: *const c_void, size: size_t, nmemb: size_t, stream: *mut FILE) -> size_t, refused 0 => libc::fwrite(ptr, size, nmemb, stream);
    swoole_coroutine_fgets(s: *mut c_char, size: c_int, stream: *mut FILE) -> *mut c_char, refused std::ptr::null_mut() => libc::fgets(s, size, stream);
    swoole_coroutine_fputs(s: *const c_char, stream: *mut FILE) -> c_int, refused -1 => libc::fputs(s, stream);
    swoole_coroutine_feof(stream: *mut FILE) -> c_int, refused -1 => libc::feof(stream);
    swoole_coroutine_fflush(stream: *mut FILE) -> c_int, refused -1 => libc::fflush(stream);
    swoole_coroutine_fclose(stream: *mut FILE) -> c_int, refused -1 => libc::fclose(stream);
    swoole_coroutine_opendir(name: *const c_char) -> *mut DIR, refused std::ptr::null_mut() => libc::opendir(name);
    swoole_coroutine_readdir(dirp: *mut DIR) -> *mut dirent, refused std::ptr::null_mut() => libc::readdir(dirp);
    swoole_coroutine_closedir(dirp: *mut DIR) -> c_int, refused -1 => libc::closedir(dirp);
    swoole_coroutine_open(pathname: *const c_char, flags: c_int, mode: mode_t) -> c_int, refused -1,
        ring |_| true => open_file(std::ffi::CStr::from_ptr(pathname), flags, mode) map std::convert::identity => libc::open(pathname, flags, c_uint::from(mode));
    swoole_coroutine_fstat(fd: c_int, statbuf: *mut stat) -> c_int, refused -1,
        ring |host| host.uring_statx => fstat(fd, &mut *statbuf) map |()| 0 => libc::fstat(fd, statbuf);
    swoole_coroutine_stat(path: *const c_char, statbuf: *mut stat) -> c_int, refused -1,
        ring |host| host.uring_statx => stat(std::ffi::CStr::from_ptr(path), &mut *statbuf) map |()| 0 => libc::stat(path, statbuf);
    // Upstream's io_uring lstat is its stat.
    swoole_coroutine_lstat(path: *const c_char, statbuf: *mut stat) -> c_int, refused -1,
        ring |host| host.uring_statx => stat(std::ffi::CStr::from_ptr(path), &mut *statbuf) map |()| 0 => libc::lstat(path, statbuf);
    swoole_coroutine_unlink(pathname: *const c_char) -> c_int, refused -1,
        ring |_| true => unlink(std::ffi::CStr::from_ptr(pathname)) map |()| 0 => libc::unlink(pathname);
    swoole_coroutine_mkdir(pathname: *const c_char, mode: mode_t) -> c_int, refused -1,
        ring |_| true => mkdir(std::ffi::CStr::from_ptr(pathname), mode) map |()| 0 => libc::mkdir(pathname, mode);
    swoole_coroutine_rmdir(pathname: *const c_char) -> c_int, refused -1,
        ring |_| true => rmdir(std::ffi::CStr::from_ptr(pathname)) map |()| 0 => libc::rmdir(pathname);
    swoole_coroutine_rename(oldpath: *const c_char, newpath: *const c_char) -> c_int, refused -1,
        ring |_| true => rename(std::ffi::CStr::from_ptr(oldpath), std::ffi::CStr::from_ptr(newpath)) map |()| 0 => libc::rename(oldpath, newpath);
    swoole_coroutine_fsync(fd: c_int) -> c_int, refused -1,
        fd ring |_| true => fsync(fd) map |()| 0 => libc::fsync(fd);
    swoole_coroutine_ftruncate(fd: c_int, length: off_t) -> c_int, refused -1,
        fd ring |host| host.uring_ftruncate => ftruncate(fd, length) map |()| 0 => libc::ftruncate(fd, length);
    swoole_coroutine_lseek(fd: c_int, offset: off_t, whence: c_int) -> off_t, refused -1 => libc::lseek(fd, offset, whence);
    swoole_coroutine_readlink(pathname: *const c_char, buf: *mut c_char, len: size_t) -> ssize_t, refused -1 => libc::readlink(pathname, buf, len);
    swoole_coroutine_statvfs(path: *const c_char, buf: *mut statvfs) -> c_int, refused -1 => libc::statvfs(path, buf);
    swoole_coroutine_access(pathname: *const c_char, mode: c_int) -> c_int, refused -1 => libc::access(pathname, mode);
}

// Upstream's macOS stand-in for `fdatasync` is `fcntl(F_FULLFSYNC)`.
#[cfg(target_vendor = "apple")]
hooked! {
    swoole_coroutine_fdatasync(fd: c_int) -> c_int, refused -1,
        fd ring |_| true => fdatasync(fd) map |()| 0 => libc::fcntl(fd, libc::F_FULLFSYNC, 0);
}
#[cfg(not(target_vendor = "apple"))]
hooked! {
    swoole_coroutine_fdatasync(fd: c_int) -> c_int, refused -1,
        fd ring |_| true => fdatasync(fd) map |()| 0 => libc::fdatasync(fd);
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
    /// The calling thread's static entry for `name`, or null.
    safe fn gethostbyname(name: Text<'_>) -> *mut hostent;
}

/// Hooked `gethostbyname`, always on the pool: the lookup's `h_errno` (0 if the
/// pool refused it) becomes this thread's.
#[must_use]
pub fn swoole_coroutine_gethostbyname(name: &CStr) -> *mut hostent {
    let call = || {
        let entry = gethostbyname(name.into());
        // The entry is the looking-up thread's, handed to the caller as upstream does:
        // only its address crosses.
        (
            entry.expose_provenance(),
            swoole_rs_corebase::sys::h_errno(),
        )
    };
    let (entry, error) = on_pool(Some(runtime().offload), (0, 0), call);
    swoole_rs_corebase::sys::set_h_errno(error);
    std::ptr::with_exposed_provenance_mut(entry)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::cell::Cell;
    use std::ffi::CString;

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

            // The entry lives in the looking-up thread's storage, which may end with
            // the test's pool thread: read it only from a lookup run here.
            assert!(!with(Pool, || swoole_coroutine_gethostbyname(c"127.0.0.1")).is_null());
            let entry = with(Here, || swoole_coroutine_gethostbyname(c"127.0.0.1"));
            assert_eq!((*entry).h_addrtype, libc::AF_INET);
            swoole_rs_corebase::sys::set_h_errno(3);
            assert!(with(Refuse, || swoole_coroutine_gethostbyname(c"localhost")).is_null());
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
            let mut info: stat = std::mem::zeroed();
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
