//! Static filesystem operations. Production uses libc directly; the immutable
//! optional provider admits the original scripted filesystem probes unchanged.
use super::{
    HttpContextServeObservation, HttpProxyText, HttpStaticContext, HttpStaticObservation, INVALID,
    swr_v2_http_context_observed, swr_v2_http_context_scan, swr_v2_http_context_serve,
};
use core::ffi::{CStr, c_char, c_void};

/// All functions are synchronous, do not unwind, and retain no supplied pointer.
/// Directory entries remain valid until the next read or close.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HttpStaticIo {
    pub stat: unsafe extern "C" fn(*const c_char, *mut HttpStaticObservation) -> bool,
    pub canonical: unsafe extern "C" fn(*const c_char, *mut c_char) -> *mut c_char,
    pub open: unsafe extern "C" fn(*const c_char) -> *mut c_void,
    pub read: unsafe extern "C" fn(*mut c_void) -> *const c_char,
    pub close: unsafe extern "C" fn(*mut c_void),
}
unsafe extern "C" fn stat(path: *const c_char, output: *mut HttpStaticObservation) -> bool {
    // SAFETY: zero is a valid initial stat buffer, filled by lstat on success.
    let mut value: libc::stat = unsafe { core::mem::zeroed() };
    // SAFETY: path is terminated and value is writable for libc.
    if unsafe { libc::lstat(path, &raw mut value) } != 0 {
        return false;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "Preserve native off_t to size_t conversion, including negative and narrow-target values"
    )]
    let filesize = value.st_size as usize;
    let facts = HttpStaticObservation {
        filesize,
        mtime: value.st_mtime,
        kind: if value.st_mode & libc::S_IFMT == libc::S_IFDIR {
            16
        } else if value.st_mode & libc::S_IFMT == libc::S_IFREG {
            32
        } else if value.st_mode & libc::S_IFMT == libc::S_IFLNK {
            8
        } else {
            0
        },
        ..HttpStaticObservation::default()
    };
    // SAFETY: caller provides aligned writable observation.
    unsafe { output.write(facts) };
    true
}
unsafe extern "C" fn canonical(path: *const c_char, output: *mut c_char) -> *mut c_char {
    // SAFETY: path is terminated; output holds PATH_MAX bytes.
    unsafe { libc::realpath(path, output) }
}
unsafe extern "C" fn open(path: *const c_char) -> *mut c_void {
    // SAFETY: path is terminated and retained only during this call.
    unsafe { libc::opendir(path).cast() }
}
unsafe extern "C" fn read(directory: *mut c_void) -> *const c_char {
    // SAFETY: directory is an open live DIR returned by opendir.
    let entry = unsafe { libc::readdir(directory.cast()) };
    if entry.is_null() {
        core::ptr::null()
    } else {
        // SAFETY: readdir returned an initialized entry valid until the next call.
        unsafe { (*entry).d_name.as_ptr() }
    }
}
unsafe extern "C" fn close(directory: *mut c_void) {
    // SAFETY: unique open directory handle, closed once.
    unsafe { libc::closedir(directory.cast()) };
}
const NATIVE: HttpStaticIo = HttpStaticIo {
    stat,
    canonical,
    open,
    read,
    close,
};

/// Copy a path before calling a provider: callbacks never run with an owner borrow.
unsafe fn filename(owner: *const HttpStaticContext) -> Option<[c_char; libc::PATH_MAX as usize]> {
    // SAFETY: caller provides a null or live context for this short immutable access.
    let owner = unsafe { owner.as_ref() }?;
    let path = owner.text(super::FILENAME)?;
    let mut output = [0; libc::PATH_MAX as usize];
    if path.len() >= output.len() {
        return None;
    }
    for (to, from) in output.iter_mut().zip(path) {
        *to = c_char::from_ne_bytes([*from]);
    }
    Some(output)
}
unsafe fn provider(io: *const HttpStaticIo) -> HttpStaticIo {
    // SAFETY: table is null or initialized and live; it is copied before any callback.
    unsafe { io.as_ref() }.copied().unwrap_or(NATIVE)
}
/// Stat the current owned filename, publishing facts only on success.
/// # Safety
/// Owner is null or uniquely lent. Provider is null or immutable and follows `HttpStaticIo`'s contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v3_http_context_stat(
    owner: *mut HttpStaticContext,
    io: *const HttpStaticIo,
) -> i32 {
    // SAFETY: context is live for copying its filename.
    let Some(path) = (unsafe { filename(owner) }) else {
        return INVALID;
    };
    // SAFETY: provider follows the caller's contract.
    let io = unsafe { provider(io) };
    let mut facts = HttpStaticObservation::default();
    // SAFETY: independent terminated path and writable facts; no owner borrow spans this effect.
    if !unsafe { (io.stat)(path.as_ptr(), &raw mut facts) } {
        return 0;
    }
    // SAFETY: live unique owner and independent initialized observation.
    let result = unsafe { swr_v2_http_context_observed(owner, 0, &raw const facts) };
    if result < 0 { result } else { 1 }
}
/// Complete filesystem path actions, leaving configuration and MIME effects to native callers.
/// # Safety
/// Same contracts as `context_serve` and `HttpStaticIo`; providers do not unwind or retain spans.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v3_http_context_serve(
    owner: *mut HttpStaticContext,
    root: *const HttpProxyText,
    observation: *const HttpContextServeObservation,
    io: *const HttpStaticIo,
) -> i32 {
    // SAFETY: live independent descriptors and unique context, as required by the caller.
    let mut action = unsafe { swr_v2_http_context_serve(owner, root, observation) };
    // SAFETY: the immutable provider is copied before effects.
    let provider = unsafe { provider(io) };
    loop {
        let mut observed = HttpContextServeObservation {
            event: 0,
            observed: 0,
            has_locations: 0,
            has_index: 0,
            autoindex: 0,
            value: HttpProxyText {
                data: core::ptr::null(),
                length: 0,
            },
        };
        let mut absolute = [0; libc::PATH_MAX as usize];
        match action {
            4 => {
                observed.event = 2;
                // SAFETY: same unique context and immutable provider; effects finish before publication.
                let result = unsafe { swr_v3_http_context_stat(owner, io) };
                if result < 0 {
                    return result;
                }
                observed.observed = result.cast_unsigned();
            }
            5 => {
                // SAFETY: filename is copied before provider invocation.
                let Some(path) = (unsafe { filename(owner) }) else {
                    return INVALID;
                };
                // SAFETY: terminated path and PATH_MAX writable output, with no owner borrow.
                let result = unsafe { (provider.canonical)(path.as_ptr(), absolute.as_mut_ptr()) };
                observed.event = 3;
                if !result.is_null() {
                    observed.observed = 1;
                    // SAFETY: successful canonical returns a terminated string valid through this resume.
                    let bytes = unsafe { CStr::from_ptr(result) }.to_bytes();
                    observed.value = HttpProxyText {
                        data: bytes.as_ptr(),
                        length: bytes.len(),
                    };
                }
            }
            _ => return action,
        }
        // SAFETY: temporary canonical backing remains live until serve has copied it.
        action = unsafe { swr_v2_http_context_serve(owner, root, &raw const observed) };
    }
}
/// Collect directory entries into this context with a scoped directory handle.
/// # Safety
/// Unique live owner; immutable synchronous provider follows `HttpStaticIo`'s contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v3_http_context_directory(
    owner: *mut HttpStaticContext,
    io: *const HttpStaticIo,
) -> i32 {
    // SAFETY: provider is copied before callbacks.
    let io = unsafe { provider(io) };
    let mut directory = core::ptr::null_mut();
    // SAFETY: unique context with an empty initial observation.
    let mut action = unsafe { swr_v2_http_context_scan(owner, 0, core::ptr::null(), 0) };
    loop {
        let (event, data, length) = match action {
            2 => {
                // SAFETY: copy completes before the open effect.
                let Some(path) = (unsafe { filename(owner) }) else {
                    return INVALID;
                };
                // SAFETY: terminated independent path, no owner borrow.
                directory = unsafe { (io.open)(path.as_ptr()) };
                (
                    if directory.is_null() { 2 } else { 1 },
                    core::ptr::null(),
                    0,
                )
            }
            3 => {
                // SAFETY: scan requests reads only after a successful open.
                let entry = unsafe { (io.read)(directory) };
                if entry.is_null() {
                    (4, core::ptr::null(), 0)
                } else {
                    // SAFETY: read returns a terminated name valid until the next read/close.
                    let bytes = unsafe { CStr::from_ptr(entry) }.to_bytes();
                    (3, bytes.as_ptr(), bytes.len())
                }
            }
            4 => {
                // SAFETY: scan has finished reads; directory is live and closed exactly once.
                unsafe { (io.close)(directory) };
                directory = core::ptr::null_mut();
                (5, core::ptr::null(), 0)
            }
            _ => {
                if !directory.is_null() {
                    // SAFETY: error cleanup closes the sole remaining live handle.
                    unsafe { (io.close)(directory) };
                }
                return action;
            }
        };
        // SAFETY: independent entry bytes remain valid until this admission finishes.
        action = unsafe { swr_v2_http_context_scan(owner, event, data, length) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::static_context::{swr_v2_http_context_drop, swr_v2_http_context_new};
    use std::os::unix::{ffi::OsStrExt, fs::symlink};

    #[test]
    fn production_filesystem_keeps_lstat_symlinks_directory_cache_and_failed_stat_facts() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/tmp")
            .join(format!("http-static-fs-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("index.html"), b"payload").unwrap();
        symlink("index.html", root.join("link")).unwrap();
        let owner =
            unsafe { swr_v2_http_context_new(core::ptr::null(), 0, libc::PATH_MAX as usize) };
        assert!(!owner.is_null());
        let set = |path: &std::path::Path| {
            let bytes = path.as_os_str().as_bytes();
            assert_eq!(
                unsafe { (*owner).publish(super::super::FILENAME, bytes) },
                0
            );
        };
        set(&root.join("index.html"));
        assert_eq!(
            unsafe { swr_v3_http_context_stat(owner, core::ptr::null()) },
            1
        );
        assert_eq!(unsafe { (*owner).facts.filesize }, 7);
        assert_eq!(unsafe { (*owner).facts.kind }, crate::static_path::FILE);
        let mtime = unsafe { (*owner).facts.mtime };
        set(&root.join("absent"));
        assert_eq!(
            unsafe { swr_v3_http_context_stat(owner, core::ptr::null()) },
            0
        );
        assert_eq!(unsafe { (*owner).facts.mtime }, mtime);
        set(&root.join("link"));
        assert_eq!(
            unsafe { swr_v3_http_context_stat(owner, core::ptr::null()) },
            1
        );
        assert_eq!(unsafe { (*owner).facts.kind }, crate::static_path::LINK);
        let path = unsafe { filename(owner) }.unwrap();
        let mut absolute = [0; libc::PATH_MAX as usize];
        assert!(!unsafe { canonical(path.as_ptr(), absolute.as_mut_ptr()) }.is_null());
        assert_eq!(
            unsafe { CStr::from_ptr(absolute.as_ptr()) }.to_bytes(),
            std::fs::canonicalize(root.join("index.html"))
                .unwrap()
                .as_os_str()
                .as_bytes()
        );
        set(&root);
        assert_eq!(
            unsafe { swr_v3_http_context_stat(owner, core::ptr::null()) },
            1
        );
        assert_eq!(
            unsafe { swr_v3_http_context_directory(owner, core::ptr::null()) },
            1
        );
        assert!(unsafe { (*owner).directory.contains(b"index.html") }.unwrap());
        assert!(unsafe { (*owner).directory.contains(b"link") }.unwrap());
        std::fs::remove_dir_all(&root).unwrap();
        // Existing nonempty cache avoids reopening even after removal.
        assert_eq!(
            unsafe { swr_v3_http_context_directory(owner, core::ptr::null()) },
            1
        );
        unsafe { swr_v2_http_context_drop(owner) };
    }
}
