//! `SwooleG`'s storage. Rust defines the symbol; `swoole::Global`
//! (`native/corebase/swoole.h`) is its layout: the configuration fields native
//! code reads in place, then its empty accessor members (`RustField` over
//! `global.rs`'s record), whose bytes `views` reserves. Both sides assert the
//! layout.
use std::ffi::{CStr, CString, c_void};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicPtr, AtomicU8, AtomicU32, AtomicU64};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CoreAllocator {
    pub malloc: Option<unsafe extern "C" fn(usize) -> *mut c_void>,
    pub calloc: Option<unsafe extern "C" fn(usize, usize) -> *mut c_void>,
    pub realloc: Option<unsafe extern "C" fn(*mut c_void, usize) -> *mut c_void>,
    pub free: Option<unsafe extern "C" fn(*mut c_void)>,
}

/// A `CoreAllocator` whose callbacks keep the allocator contract: `malloc`,
/// `calloc` and `realloc` return null or a block of the size asked for, which
/// `realloc` and `free` take back, so a message bus may adopt what they return.
#[derive(Clone, Copy)]
pub struct TrustedAllocator(&'static CoreAllocator);

impl TrustedAllocator {
    /// # Safety
    /// `allocator`'s callbacks keep the allocator contract above.
    #[must_use]
    pub const unsafe fn new(allocator: &'static CoreAllocator) -> Self {
        Self(allocator)
    }

    #[must_use]
    pub const fn get(self) -> &'static CoreAllocator {
        self.0
    }
}

/// `swoole::Global`. Native code reads and writes the fields in place, as upstream's
/// plain `SwooleG`, so each is an atomic of its C type's size: Rust loads and stores
/// whole words and holds no reference to their contents. Rust writes them only in
/// `swoole_init` and `swoole_clean`, before and after any other thread, as upstream does.
#[repr(C)]
struct Global {
    max_sockets: AtomicU32,
    /// A `double`'s bits.
    dns_cache_refresh_time: AtomicU64,
    /// `swoole::Allocator`: `malloc`, `calloc`, `realloc` and `free`, or null.
    std_allocator: [AtomicPtr<c_void>; 4],
    /// The empty accessor members' bytes.
    views: [AtomicU8; 3],
}

const _: () = assert!(
    std::mem::offset_of!(Global, dns_cache_refresh_time) == 8
        && std::mem::offset_of!(Global, std_allocator) == 16
        && size_of::<Global>() == 56
        && size_of::<CoreAllocator>() == 32,
    "swoole.h asserts the same `swoole::Global` layout"
);

/// Upstream's `SwooleG = {}`.
#[cfg_attr(feature = "upstream-symbols", unsafe(export_name = "SwooleG"))]
static SWOOLE_G: Global = Global {
    max_sockets: AtomicU32::new(0),
    dns_cache_refresh_time: AtomicU64::new(0),
    std_allocator: [const { AtomicPtr::new(std::ptr::null_mut()) }; 4],
    views: [const { AtomicU8::new(0) }; 3],
};

/// `SwooleG.task_tmpfile`, `None` when empty. Its lock is `sw_thread_lock`, which
/// `swoole_set_task_tmpdir` holds. Readers clone the `Arc`, so a path another
/// thread replaces stays alive while they use it.
static TASK_TMPFILE: Mutex<Option<Arc<CStr>>> = Mutex::new(None);

fn task_slot() -> MutexGuard<'static, Option<Arc<CStr>>> {
    TASK_TMPFILE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `SwooleG.max_sockets`.
#[must_use]
pub fn max_sockets() -> u32 {
    SWOOLE_G.max_sockets.load(Relaxed)
}

/// A task file path; the paths built here hold no NUL.
fn task_path(path: Vec<u8>) -> Option<Arc<CStr>> {
    CString::new(path).ok().map(Arc::from)
}

/// `swoole_get_task_tmpdir()`: `SwooleG.task_tmpfile`, the template of a task's
/// temporary file, as an owned handle.
#[must_use]
pub fn task_tmpfile() -> Arc<CStr> {
    task_slot().clone().unwrap_or_else(|| Arc::from(c""))
}

/// `SwooleG.std_allocator = allocator`, null for none.
fn set_allocator(allocator: [*mut c_void; 4]) {
    for (slot, callback) in SWOOLE_G.std_allocator.iter().zip(allocator) {
        slot.store(callback, Relaxed);
    }
}

/// `SwooleG = {}`: the fields and the paths (lifecycle.rs clears the name resolvers with them).
pub(crate) fn reset() {
    SWOOLE_G.max_sockets.store(0, Relaxed);
    SWOOLE_G.dns_cache_refresh_time.store(0, Relaxed);
    set_allocator([std::ptr::null_mut(); 4]);
    for byte in &SWOOLE_G.views {
        byte.store(0, Relaxed);
    }
    *task_slot() = None;
}

/// `swoole_init`'s first step: libc's allocator.
pub(crate) fn init_allocator() {
    set_allocator([
        libc::malloc as *mut c_void,
        libc::calloc as *mut c_void,
        libc::realloc as *mut c_void,
        libc::free as *mut c_void,
    ]);
}

/// `swoole_init`'s configuration once the logger exists: the resolver and task
/// path and the descriptor limit (warning, as upstream, when getrlimit fails).
pub(crate) fn init() {
    let observed = crate::swr_v1_core_max_sockets(SW_SESSION_LIST_SIZE);
    let limit = u32::try_from(observed).unwrap_or(crate::constants::SW_MAX_SOCKETS_DEFAULT);
    SWOOLE_G.max_sockets.store(limit, Relaxed);
    if observed < 0 {
        crate::log::sys_warning(c"swoole_init", format_args!("getrlimit() failed"));
    }
    let task = [SW_TASK_TMP_DIR, "/", crate::constants::SW_TASK_TMP_FILE].concat();
    *task_slot() = task_path(task.into_bytes());
}

use crate::constants::{SW_SESSION_LIST_SIZE, SW_TASK_TMP_DIR};

/// Why a task directory was refused.
#[derive(Debug, PartialEq)]
enum TaskDir {
    Relative,
    /// `swoole_mkdir_recursive` failed; the failing prefix is in `scratch`.
    Mkdir(i32),
    TooLong,
}

/// Validate, create and publish `dir`'s task file. A path too long for
/// `SW_TASK_TMP_PATH_SIZE` is published before it is refused, as upstream does.
fn set_task_path(
    slot: &mut Option<Arc<CStr>>,
    dir: &CStr,
    scratch: &mut [u8],
) -> Result<(), TaskDir> {
    let bytes = dir.to_bytes();
    if bytes.first() != Some(&b'/') {
        return Err(TaskDir::Relative);
    }
    if crate::sys::access(dir, libc::R_OK) < 0 {
        let status = crate::swr_v1_corebase_mkdir_complete(bytes, scratch);
        if status != 0 {
            return Err(TaskDir::Mkdir(status));
        }
    }
    let path = [bytes, b"/", crate::constants::SW_TASK_TMP_FILE.as_bytes()].concat();
    let length = path.len();
    *slot = task_path(path);
    if length >= crate::constants::SW_TASK_TMP_PATH_SIZE as usize {
        return Err(TaskDir::TooLong);
    }
    Ok(())
}

/// `bool swoole_set_task_tmpdir(const std::string &dir)` under its lock, with
/// upstream's warnings.
#[expect(
    clippy::must_use_candidate,
    reason = "PHP's setters ignore it, as upstream's do"
)]
pub fn set_task_tmpdir(dir: &CStr) -> bool {
    let mut slot = task_slot();
    let mut scratch = [0u8; libc::PATH_MAX as usize];
    let result = set_task_path(&mut slot, dir, &mut scratch);
    drop(slot);
    let mkdir = c"swoole_mkdir_recursive";
    match result {
        Err(TaskDir::Mkdir(1)) => {
            let code = crate::sys::errno();
            let failed = CStr::from_bytes_until_nul(&scratch).unwrap_or_default();
            let failed = failed.to_string_lossy();
            crate::log::sys_warning_errno(code, mkdir, format_args!("mkdir('{failed}') failed"));
        }
        Err(TaskDir::Mkdir(_)) => crate::log::error_log(
            LOG_WARNING,
            NAME_TOO_LONG,
            mkdir,
            format_args!(
                "mkdir() failed. Path exceeds the limit of {} characters",
                libc::PATH_MAX - 1
            ),
        ),
        _ => {}
    }
    let origin = c"swoole_set_task_tmpdir";
    let dir = dir.to_string_lossy();
    match result {
        Ok(()) => return true,
        Err(TaskDir::Relative) => {
            crate::log::warning(origin, format_args!("wrong absolute path '{dir}'"));
        }
        Err(TaskDir::Mkdir(_)) => {
            crate::log::warning(origin, format_args!("create task tmp dir('{dir}') failed"));
        }
        Err(TaskDir::TooLong) => crate::log::warning(
            origin,
            format_args!(
                "task tmp_dir is too large, the max size is '{}'",
                crate::constants::SW_TASK_TMP_PATH_SIZE - 1
            ),
        ),
    }
    false
}

/// `SW_LOG_WARNING` and `SW_ERROR_NAME_TOO_LONG`.
const LOG_WARNING: i32 = 4;
const NAME_TOO_LONG: i32 = 504;

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    fn path() -> Vec<u8> {
        task_tmpfile().to_bytes().to_vec()
    }

    fn template(dir: &CStr) -> Vec<u8> {
        [
            dir.to_bytes(),
            b"/",
            crate::constants::SW_TASK_TMP_FILE.as_bytes(),
        ]
        .concat()
    }

    // One test owns the process-wide storage, so parallel tests never race on it.
    #[test]
    #[cfg_attr(miri, ignore = "Miri has no getrlimit")]
    fn storage_init_task_directory_and_reset() {
        init_allocator();
        init();
        let allocator = unsafe {
            std::ptr::from_ref(&SWOOLE_G.std_allocator)
                .cast::<CoreAllocator>()
                .read()
        };
        let allocation = unsafe { (allocator.malloc.unwrap())(19) };
        assert!(!allocation.is_null());
        unsafe { (allocator.free.unwrap())(allocation) };
        assert!(max_sockets() >= 1);
        assert_eq!(path(), b"/tmp/swoole.task.XXXXXX");

        let root = std::env::temp_dir().join(format!("swoole-core-config-{}", std::process::id()));
        let nested = root.join("nested");
        let dir = CString::new(nested.as_os_str().as_encoded_bytes()).unwrap();
        let mut scratch = [0u8; 4096];
        assert_eq!(set_task_path(&mut task_slot(), &dir, &mut scratch), Ok(()));
        assert!(nested.is_dir());
        let expected = template(&dir);
        assert_eq!(path(), expected);
        assert_eq!(
            set_task_path(&mut task_slot(), c"relative", &mut scratch),
            Err(TaskDir::Relative)
        );
        assert_eq!(path(), expected);
        let long = root
            .join("a".repeat(100))
            .join("b".repeat(100))
            .join("c".repeat(100));
        let long = CString::new(long.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(
            set_task_path(&mut task_slot(), &long, &mut scratch),
            Err(TaskDir::TooLong)
        );
        assert!(path().len() >= 256);

        // A reader's handle outlives any number of replacements on other threads.
        let other = CString::new(root.as_os_str().as_encoded_bytes()).unwrap();
        let held = task_tmpfile();
        // Readers may run before either setter, so start from one of the two.
        assert!(set_task_tmpdir(&dir));
        let templates = [template(&dir), template(&other)];
        std::thread::scope(|scope| {
            for dir in [&dir, &other] {
                scope.spawn(move || {
                    for _ in 0..200 {
                        assert!(set_task_tmpdir(dir));
                    }
                });
                scope.spawn(|| {
                    for _ in 0..200 {
                        assert!(templates.contains(&path()));
                    }
                });
            }
        });
        assert!(held.to_bytes().len() >= 256);
        std::fs::remove_dir_all(root).unwrap();

        reset();
        assert_eq!(max_sockets(), 0);
        let allocator = unsafe {
            std::ptr::from_ref(&SWOOLE_G.std_allocator)
                .cast::<CoreAllocator>()
                .read()
        };
        assert!(allocator.malloc.is_none());
        assert_eq!(path(), b"");
    }
}
