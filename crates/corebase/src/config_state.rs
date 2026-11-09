//! `SwooleG`'s storage. Rust defines the symbol; `swoole::Global`
//! (`native/corebase/swoole.h`) is its layout: the configuration fields native
//! code reads in place, then its empty accessor members (`RustField` over
//! `global.rs`'s record, `RustPath` over the paths here), whose bytes `views`
//! reserves. Both sides assert the layout.
use crate::{StringSlot, swr_v1_core_string_set, swr_v1_core_string_view};
use std::cell::UnsafeCell;
use std::ffi::{CStr, c_char, c_void};
use std::sync::{Mutex, PoisonError};

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CoreAllocator {
    pub malloc: Option<unsafe extern "C" fn(usize) -> *mut c_void>,
    pub calloc: Option<unsafe extern "C" fn(usize, usize) -> *mut c_void>,
    pub realloc: Option<unsafe extern "C" fn(*mut c_void, usize) -> *mut c_void>,
    pub free: Option<unsafe extern "C" fn(*mut c_void)>,
}

impl CoreAllocator {
    const NONE: Self = Self {
        malloc: None,
        calloc: None,
        realloc: None,
        free: None,
    };
}

/// `swoole::Global`.
#[repr(C)]
pub struct Global {
    pub max_sockets: u32,
    pub dns_cache_refresh_time: f64,
    pub std_allocator: CoreAllocator,
    /// The empty accessor members' bytes.
    views: [u8; 8],
}

const _: () = assert!(
    std::mem::offset_of!(Global, dns_cache_refresh_time) == 8
        && std::mem::offset_of!(Global, std_allocator) == 16
        && size_of::<Global>() == 56,
    "swoole.h asserts the same `swoole::Global` layout"
);

impl Global {
    /// Upstream's `SwooleG = {}`.
    const ZERO: Self = Self {
        max_sockets: 0,
        dns_cache_refresh_time: 0.0,
        std_allocator: CoreAllocator::NONE,
        views: [0; 8],
    };
}

struct Storage(UnsafeCell<Global>);

// SAFETY: native code reads the fields in place, as upstream's plain `SwooleG`;
// Rust writes them only in `swoole_init` and `swoole_clean`, before and after
// any other thread, as upstream does.
unsafe impl Sync for Storage {}

#[cfg_attr(feature = "upstream-symbols", unsafe(export_name = "SwooleG"))]
static SWOOLE_G: Storage = Storage(UnsafeCell::new(Global::ZERO));

/// `SwooleG.task_tmpfile`.
static TASK_TMPFILE: StringSlot = StringSlot::new();

/// `sw_thread_lock`, which `swoole_set_task_tmpdir` holds.
static TASK_TMPDIR_LOCK: Mutex<()> = Mutex::new(());

fn global() -> *mut Global {
    SWOOLE_G.0.get()
}

/// `SwooleG.max_sockets`.
#[must_use]
pub fn max_sockets() -> u32 {
    // SAFETY: a plain read of the live static, written only by init and clean.
    unsafe { (*global()).max_sockets }
}

fn set_path(slot: &StringSlot, path: &[u8]) -> bool {
    // SAFETY: `path` lends its bytes to a live slot.
    unsafe { swr_v1_core_string_set(slot, path.as_ptr().cast(), path.len()) == 0 }
}

fn path(slot: &StringSlot) -> *const c_char {
    // SAFETY: a live slot, changed only by init, clean and the task-dir setter.
    unsafe { swr_v1_core_string_view(slot) }.data
}

/// `SwooleG.task_tmpfile.c_str()`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_core_task_tmpfile() -> *const c_char {
    path(&TASK_TMPFILE)
}

/// `SwooleG = {}`: the fields and the paths (lifecycle.rs clears the name resolvers with them).
pub(crate) fn reset() {
    // SAFETY: init and clean run with no other thread using `SwooleG`.
    unsafe { global().write(Global::ZERO) };
    set_path(&TASK_TMPFILE, b"");
}

/// `swoole_init`'s first step: libc's allocator.
pub(crate) fn init_allocator() {
    // SAFETY: as `reset`.
    unsafe {
        (*global()).std_allocator = CoreAllocator {
            malloc: Some(libc::malloc),
            calloc: Some(libc::calloc),
            realloc: Some(libc::realloc),
            free: Some(libc::free),
        };
    };
}

/// `swoole_init`'s configuration once the logger exists: the resolver and task
/// path and the descriptor limit (warning, as upstream, when getrlimit fails).
pub(crate) fn init() {
    let observed = crate::swr_v1_core_max_sockets(SW_SESSION_LIST_SIZE);
    let limit = u32::try_from(observed).unwrap_or(crate::constants::SW_MAX_SOCKETS_DEFAULT);
    // SAFETY: as `reset`.
    unsafe { (*global()).max_sockets = limit };
    if observed < 0 {
        crate::log::sys_warning(c"swoole_init", format_args!("getrlimit() failed"));
    }
    let task = [SW_TASK_TMP_DIR, "/", crate::constants::SW_TASK_TMP_FILE].concat();
    set_path(&TASK_TMPFILE, task.as_bytes());
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
fn set_task_path(dir: &CStr, scratch: &mut [u8]) -> Result<(), TaskDir> {
    let bytes = dir.to_bytes();
    if bytes.first() != Some(&b'/') {
        return Err(TaskDir::Relative);
    }
    // SAFETY: a NUL-terminated path.
    if unsafe { libc::access(dir.as_ptr(), libc::R_OK) } < 0 {
        // SAFETY: `dir` lends its bytes; `scratch` is disjoint and writable.
        let status = unsafe {
            crate::swr_v1_corebase_mkdir_complete(
                bytes.as_ptr(),
                bytes.len(),
                scratch.as_mut_ptr(),
                scratch.len(),
            )
        };
        if status != 0 {
            return Err(TaskDir::Mkdir(status));
        }
    }
    let path = [bytes, b"/", crate::constants::SW_TASK_TMP_FILE.as_bytes()].concat();
    set_path(&TASK_TMPFILE, &path);
    if path.len() >= crate::constants::SW_TASK_TMP_PATH_SIZE as usize {
        return Err(TaskDir::TooLong);
    }
    Ok(())
}

/// `bool swoole_set_task_tmpdir(const std::string &dir)` under its lock, with
/// upstream's warnings.
/// # Safety
/// `dir` is a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_set_task_tmpdir(dir: *const c_char) -> bool {
    let _lock = TASK_TMPDIR_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    // SAFETY: the caller's NUL-terminated string.
    let dir = unsafe { CStr::from_ptr(dir) };
    let mut scratch = [0u8; libc::PATH_MAX as usize];
    let result = set_task_path(dir, &mut scratch);
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

    fn task_path() -> &'static [u8] {
        unsafe { CStr::from_ptr(swr_v1_core_task_tmpfile()) }.to_bytes()
    }

    // One test owns the process-wide storage, so parallel tests never race on it.
    #[test]
    fn storage_init_task_directory_and_reset() {
        init_allocator();
        init();
        let allocator = unsafe { (*global()).std_allocator };
        let allocation = unsafe { (allocator.malloc.unwrap())(19) };
        assert!(!allocation.is_null());
        unsafe { (allocator.free.unwrap())(allocation) };
        assert!(max_sockets() >= 1);
        assert_eq!(task_path(), b"/tmp/swoole.task.XXXXXX");

        let root = std::env::temp_dir().join(format!("swoole-core-config-{}", std::process::id()));
        let nested = root.join("nested");
        let dir = CString::new(nested.as_os_str().as_encoded_bytes()).unwrap();
        let mut scratch = [0u8; 4096];
        assert_eq!(set_task_path(&dir, &mut scratch), Ok(()));
        assert!(nested.is_dir());
        let expected = [
            dir.as_bytes(),
            b"/",
            crate::constants::SW_TASK_TMP_FILE.as_bytes(),
        ]
        .concat();
        assert_eq!(task_path(), expected);
        assert_eq!(
            set_task_path(c"relative", &mut scratch),
            Err(TaskDir::Relative)
        );
        assert_eq!(task_path(), expected);
        let long = root
            .join("a".repeat(100))
            .join("b".repeat(100))
            .join("c".repeat(100));
        let long = CString::new(long.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(set_task_path(&long, &mut scratch), Err(TaskDir::TooLong));
        assert!(task_path().len() >= 256);
        std::fs::remove_dir_all(root).unwrap();

        reset();
        assert_eq!(max_sockets(), 0);
        assert!(unsafe { (*global()).std_allocator.malloc }.is_none());
        assert_eq!(task_path(), b"");
    }
}
