//! Temporary file ownership from creation through handoff.
use crate::{log, sys};
use core::ffi::c_char;
use core::marker::PhantomData;
use core::mem::DropGuard;
use core::ptr::NonNull;
use std::ffi::{CStr, CString};
use std::fs::File;
use std::os::fd::OwnedFd;

/// A writable file name template, NUL-terminated within the bytes it borrows: what
/// `mkstemp` reads up to the NUL and rewrites (its `XXXXXX`) in place.
#[repr(transparent)]
struct Template<'a>(NonNull<c_char>, PhantomData<&'a mut [u8]>);

impl<'a> Template<'a> {
    /// `None` when `bytes` holds no NUL.
    fn new(bytes: &'a mut [u8]) -> Option<Self> {
        bytes
            .contains(&0)
            .then(|| Self(NonNull::from(bytes).cast(), PhantomData))
    }
}

// libc's, over a template they write only before its NUL; a template without the
// `XXXXXX` suffix is an error they return. The descriptor they return is a new one
// the caller owns, or -1 (`None`: `OwnedFd` never holds -1).
unsafe extern "C" {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    safe fn mkostemp(template: Template<'_>, flags: core::ffi::c_int) -> Option<OwnedFd>;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    safe fn mkstemp(template: Template<'_>) -> Option<OwnedFd>;
}

/// `swoole_tmpfile`: `mkostemp(O_WRONLY | O_CREAT)` on Linux, `mkstemp` elsewhere, over
/// the `XXXXXX` template up to the first NUL in `template`: the new descriptor, or
/// none (errno `EINVAL` when there is no NUL).
fn tmpfile(template: &mut [u8]) -> Option<OwnedFd> {
    let Some(template) = Template::new(template) else {
        sys::set_errno(libc::EINVAL);
        return None;
    };
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let fd = mkostemp(template, libc::O_WRONLY | libc::O_CREAT);
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let fd = mkstemp(template);
    fd
}

/// A file and its cleanup obligation. Dropping it unlinks the generated name.
pub struct TempFile<T = File> {
    pub file: DropGuard<T, fn(T)>,
    path: DropGuard<CString, fn(CString)>,
}

fn close<T>(file: T) {
    let errno = sys::errno();
    drop(file);
    sys::set_errno(errno);
}

fn remove(path: CString) {
    let errno = sys::errno();
    _ = sys::unlink(&path);
    drop(path);
    sys::set_errno(errno);
}

impl TempFile {
    /// Create and name a file in place, warning on failure as `swoole_tmpfile` does.
    pub fn create(template: &mut [u8]) -> Option<Self> {
        let fd = tmpfile(template);
        let name = CStr::from_bytes_until_nul(template).unwrap_or_default();
        let Some(fd) = fd else {
            let errno = sys::errno();
            log::sys_warning(
                c"swoole_tmpfile",
                format_args!("mkstemp('{}') failed", name.to_string_lossy()),
            );
            sys::set_errno(errno);
            return None;
        };
        Some(Self {
            file: DropGuard::new(File::from(fd), close),
            path: DropGuard::new(name.to_owned(), remove),
        })
    }
}

impl<T> TempFile<T> {
    #[must_use]
    pub fn path(&self) -> &CStr {
        &self.path
    }

    /// Change how the file is held while retaining ownership of its name.
    pub fn map_file<U>(self, f: impl FnOnce(T) -> U) -> TempFile<U> {
        TempFile {
            file: DropGuard::new(f(DropGuard::dismiss(self.file)), close),
            path: self.path,
        }
    }

    /// Transfer the name and file to their next owner.
    #[must_use]
    pub fn persist(self) -> (CString, T) {
        (DropGuard::dismiss(self.path), DropGuard::dismiss(self.file))
    }
}
