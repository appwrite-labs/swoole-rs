//! The hooked stdio, directory, file-system and resolver functions
//! (`swoole_coroutine_fopen`, `swoole_coroutine_stat`, `swoole_coroutine_getaddrinfo`,
//! …): inside a coroutine each libc call runs on the async thread pool while the
//! coroutine waits; outside one it runs here, as [`NATIVE`] decides. Native code
//! lends the `io_uring` engine and the hooked sockets' constructors in [`Host`].
//! The exports keep upstream's names.
use std::sync::OnceLock;

use libc::{
    DIR, FILE, addrinfo, c_char, c_int, c_uint, c_void, dirent, hostent, mode_t, off_t, size_t,
    ssize_t, stat, statvfs,
};

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
    /// `System::sleep`.
    pub sleep: fn(f64) -> c_int,
    /// `swoole_set_last_error`.
    pub set_last_error: fn(c_int),
}

/// The coroutine runtime's.
#[cfg(not(test))]
pub(crate) static NATIVE: Ops = Ops {
    in_coroutine: native::in_coroutine,
    offload: crate::offload::async_job,
    sleep: native::sleep,
    set_last_error: swoole_rs_corebase::log::set_last_error,
};

/// The unit tests link no runtime: theirs is the fake.
#[cfg(test)]
pub(crate) static NATIVE: Ops = tests::OPS;

#[cfg(not(test))]
mod native {
    use libc::c_int;

    pub(super) fn in_coroutine() -> bool {
        !swoole_rs_corebase::thread_reactor().is_null() && !swoole_rs_coroutine::current().is_null()
    }

    pub(super) fn sleep(seconds: f64) -> c_int {
        crate::sleep::system_sleep(seconds)
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
#[cfg(not(test))]
pub fn swr_v1_hook_install() {
    use swoole_rs_corebase::{Switch, built_with};
    static BUILT: OnceLock<Host> = OnceLock::new();
    #[cfg(all(feature = "iouring", feature = "upstream-symbols"))]
    let uring = Some(&crate::iouring::HOST);
    #[cfg(all(feature = "iouring", not(feature = "upstream-symbols")))]
    let uring = None;
    set_host(BUILT.get_or_init(|| Host {
        #[cfg(feature = "iouring")]
        uring,
        uring_statx: built_with(Switch::IouringStatx),
        uring_ftruncate: built_with(Switch::IouringFtruncate),
        uring_futex: built_with(Switch::IouringFutex),
        uring_socket: built_with(Switch::UringSocket),
    }));
    #[cfg(all(feature = "iouring", feature = "upstream-symbols"))]
    swoole_rs_reactor::register_uring(&crate::iouring::REACTOR);
    crate::co_io::install_native();
}

fn set_host(host: &'static Host) {
    HOST.get_or_init(|| host);
    RUNTIME.get_or_init(|| &NATIVE);
}

pub(crate) fn host() -> Option<&'static Host> {
    HOST.get().copied()
}

/// The host inside a coroutine (hook.cc's `!is_no_coro()`), else none.
pub(crate) fn coroutine() -> Option<&'static Host> {
    host().filter(|_| (NATIVE.in_coroutine)())
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
    (NATIVE.in_coroutine)().then_some(uring)
}

/// Each hooked function: `ring` names its `io_uring` operation over arguments that
/// dereference the caller's pointers, `fd ring` one over plain values.
macro_rules! hooked {
    ($($name:ident($($arg:ident: $ty:ty),*) -> $ret:ty, refused $refused:expr
        $(, ring $gate:expr => $ring:ident($($ring_arg:expr),*))?
        $(, fd ring $fd_gate:expr => $fd_ring:ident($($fd_ring_arg:expr),*))? => $call:expr;)*) => {$(
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
                    return unsafe { crate::uring::$ring(ring, $($ring_arg),*) };
                }
            )?
            $(
                #[cfg(feature = "iouring")]
                if let Some(ring) = ring($fd_gate) {
                    return crate::uring::$fd_ring(ring, $($fd_ring_arg),*);
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
        ring |_| true => open_file(std::ffi::CStr::from_ptr(pathname), flags, mode) => libc::open(pathname, flags, c_uint::from(mode));
    swoole_coroutine_fstat(fd: c_int, statbuf: *mut stat) -> c_int, refused -1,
        ring |host| host.uring_statx => fstat(fd, &mut *statbuf) => libc::fstat(fd, statbuf);
    swoole_coroutine_stat(path: *const c_char, statbuf: *mut stat) -> c_int, refused -1,
        ring |host| host.uring_statx => stat(std::ffi::CStr::from_ptr(path), &mut *statbuf) => libc::stat(path, statbuf);
    // Upstream's io_uring lstat is its stat.
    swoole_coroutine_lstat(path: *const c_char, statbuf: *mut stat) -> c_int, refused -1,
        ring |host| host.uring_statx => stat(std::ffi::CStr::from_ptr(path), &mut *statbuf) => libc::lstat(path, statbuf);
    swoole_coroutine_unlink(pathname: *const c_char) -> c_int, refused -1,
        ring |_| true => unlink(std::ffi::CStr::from_ptr(pathname)) => libc::unlink(pathname);
    swoole_coroutine_mkdir(pathname: *const c_char, mode: mode_t) -> c_int, refused -1,
        ring |_| true => mkdir(std::ffi::CStr::from_ptr(pathname), mode) => libc::mkdir(pathname, mode);
    swoole_coroutine_rmdir(pathname: *const c_char) -> c_int, refused -1,
        ring |_| true => rmdir(std::ffi::CStr::from_ptr(pathname)) => libc::rmdir(pathname);
    swoole_coroutine_rename(oldpath: *const c_char, newpath: *const c_char) -> c_int, refused -1,
        ring |_| true => rename(std::ffi::CStr::from_ptr(oldpath), std::ffi::CStr::from_ptr(newpath)) => libc::rename(oldpath, newpath);
    swoole_coroutine_fsync(fd: c_int) -> c_int, refused -1,
        fd ring |_| true => fsync(fd) => libc::fsync(fd);
    swoole_coroutine_ftruncate(fd: c_int, length: off_t) -> c_int, refused -1,
        fd ring |host| host.uring_ftruncate => ftruncate(fd, length) => libc::ftruncate(fd, length);
    swoole_coroutine_lseek(fd: c_int, offset: off_t, whence: c_int) -> off_t, refused -1 => libc::lseek(fd, offset, whence);
    swoole_coroutine_readlink(pathname: *const c_char, buf: *mut c_char, len: size_t) -> ssize_t, refused -1 => libc::readlink(pathname, buf, len);
    swoole_coroutine_statvfs(path: *const c_char, buf: *mut statvfs) -> c_int, refused -1 => libc::statvfs(path, buf);
    swoole_coroutine_access(pathname: *const c_char, mode: c_int) -> c_int, refused -1 => libc::access(pathname, mode);
}

// Upstream's macOS stand-in for `fdatasync` is `fcntl(F_FULLFSYNC)`.
#[cfg(target_vendor = "apple")]
hooked! {
    swoole_coroutine_fdatasync(fd: c_int) -> c_int, refused -1,
        fd ring |_| true => fdatasync(fd) => libc::fcntl(fd, libc::F_FULLFSYNC, 0);
}
#[cfg(not(target_vendor = "apple"))]
hooked! {
    swoole_coroutine_fdatasync(fd: c_int) -> c_int, refused -1,
        fd ring |_| true => fdatasync(fd) => libc::fdatasync(fd);
}

/// Hooked `close` of an fd that is no coroutine socket, on the pool (`offload`).
#[must_use]
pub fn swr_v1_hook_close_fd(fd: c_int, offload: Option<Offload>) -> c_int {
    on_pool(offload, -1, || swoole_rs_corebase::sys::close(fd))
}

/// Hooked `sleep`: `System::sleep` (fatal outside a coroutine, as upstream).
#[unsafe(no_mangle)]
pub extern "C" fn swoole_coroutine_sleep(sec: c_int) {
    (NATIVE.sleep)(f64::from(sec));
}

/// Hooked `usleep`, with upstream's 1024-based conversion.
#[unsafe(no_mangle)]
pub extern "C" fn swoole_coroutine_usleep(usec: c_int) {
    (NATIVE.sleep)(f64::from(usec) / 1024.0 / 1024.0);
}

/// Hooked `getaddrinfo`, always on the pool: upstream does not test for a coroutine.
///
/// # Safety
/// As for `getaddrinfo`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_coroutine_getaddrinfo(
    name: *const c_char,
    service: *const c_char,
    req: *const addrinfo,
    pai: *mut *mut addrinfo,
) -> c_int {
    // SAFETY: the caller's arguments are valid for getaddrinfo.
    let call = || unsafe { libc::getaddrinfo(name, service, req, pai) };
    // SAFETY: `async_job`.
    unsafe { offloaded(Some(NATIVE.offload), -1, call) }
}

unsafe extern "C" {
    fn gethostbyname(name: *const c_char) -> *mut hostent;
}

/// Hooked `gethostbyname`, always on the pool: the lookup's `h_errno` (0 if the
/// pool refused it) becomes this thread's.
///
/// # Safety
/// `name` is a C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swoole_coroutine_gethostbyname(name: *const c_char) -> *mut hostent {
    let offload = Some(NATIVE.offload);
    let call = || {
        // SAFETY: the caller passes a C string.
        let entry = unsafe { gethostbyname(name) };
        (entry, swoole_rs_corebase::sys::h_errno())
    };
    // SAFETY: the caller passes native `async` or null.
    let (entry, error) = unsafe { offloaded(offload, (std::ptr::null_mut(), 0), call) };
    swoole_rs_corebase::sys::set_h_errno(error);
    entry
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
    fn sleep(seconds: f64) -> c_int {
        SLEPT.set(seconds);
        0
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
        set_host(&TEST_HOST);
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
            let lookup = |service: *const c_char, list: *mut *mut addrinfo| {
                swoole_coroutine_getaddrinfo(c"127.0.0.1".as_ptr(), service, std::ptr::null(), list)
            };
            // Upstream offloads the resolver even outside a coroutine.
            assert_eq!(with(Here, || lookup(c"80".as_ptr(), &raw mut list)), 0);
            assert_eq!(OFFLOADED.get(), 1);
            assert!(!list.is_null());
            libc::freeaddrinfo(list);
            assert_eq!(with(Refuse, || lookup(std::ptr::null(), &raw mut list)), -1);

            // The entry lives in the looking-up thread's storage, which may end with
            // the test's pool thread: read it only from a lookup run here.
            assert!(
                !with(Pool, || swoole_coroutine_gethostbyname(
                    c"127.0.0.1".as_ptr()
                ))
                .is_null()
            );
            let entry = with(Here, || {
                swoole_coroutine_gethostbyname(c"127.0.0.1".as_ptr())
            });
            assert_eq!((*entry).h_addrtype, libc::AF_INET);
            swoole_rs_corebase::sys::set_h_errno(3);
            assert!(
                with(Refuse, || swoole_coroutine_gethostbyname(
                    c"localhost".as_ptr()
                ))
                .is_null()
            );
            assert_eq!(swoole_rs_corebase::sys::h_errno(), 0);

            let mut fds = [0; 2];
            assert_eq!(libc::pipe(fds.as_mut_ptr()), 0);
            assert_eq!(swr_v1_hook_close_fd(fds[0], Some(pool)), 0);
            assert_eq!(swr_v1_hook_close_fd(fds[1], None), 0);
        }
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
