//! File descriptor, flags and path ownership: upstream's `File`.
use crate::file::{FileIo, file_close, file_open, read_all, write_all};
use libc::c_int;
use std::ffi::CStr;
use swoole_rs_corebase::sys;

pub struct FileRecord {
    fd: c_int,
    flags: c_int,
    path: Vec<u8>,
}
impl FileRecord {
    /// Empty owner used before publishing a copied request descriptor.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            fd: -1,
            flags: 0,
            path: Vec::new(),
        }
    }

    /// Own `fd`, opened with `flags`, with no path.
    #[must_use]
    pub const fn adopt(fd: c_int, flags: c_int) -> Self {
        Self {
            fd,
            flags,
            path: Vec::new(),
        }
    }

    /// Open the request's file and own its exact path bytes.
    #[must_use]
    pub fn open_readonly(path: &CStr) -> Option<Self> {
        let path_bytes = path_copy(path.to_bytes())?;
        let fd = file_open(-1, path, libc::O_RDONLY, 0);
        let file = Self {
            fd,
            flags: 0,
            path: path_bytes,
        };
        Some(file)
    }

    #[must_use]
    pub const fn descriptor(&self) -> c_int {
        self.fd
    }

    /// `File::ready`.
    #[must_use]
    pub const fn ready(&self) -> bool {
        self.fd != -1
    }

    /// `File::open`: closes a descriptor it still owns, then opens `path`.
    pub fn open(&mut self, path: &CStr, flags: c_int, mode: c_int) -> bool {
        self.fd = file_open(self.fd, path, flags, mode);
        self.path = path.to_bytes().to_vec();
        self.flags = flags;
        self.ready()
    }

    /// `File::close`: invalidates the record before the syscall; false when already closed.
    pub fn close(&mut self) -> bool {
        file_close(std::mem::replace(&mut self.fd, -1))
    }

    /// The path it was opened with (`File::get_path()`).
    #[must_use]
    pub fn path(&self) -> &[u8] {
        &self.path
    }

    /// `File::write_all`, appending when opened with `O_APPEND`.
    #[must_use]
    pub fn write_all(&self, data: &[u8]) -> FileIo {
        write_all(self.fd, self.flags & libc::O_APPEND != 0, data)
    }

    /// `File::read_all` from offset zero.
    #[must_use]
    pub fn read_all(&self, buf: &mut [impl swoole_rs_corebase::sys::ReadSlot]) -> FileIo {
        read_all(self.fd, buf)
    }
}
impl Drop for FileRecord {
    fn drop(&mut self) {
        if self.fd >= 0 {
            // This record owns the descriptor until close/release.
            sys::close(self.fd);
        }
    }
}
fn path_copy(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut path = Vec::new();
    path.try_reserve_exact(bytes.len()).ok()?;
    path.extend_from_slice(bytes);
    Some(path)
}
/// Current descriptor; ready is fd != -1, and release writes -1.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_os_file_record_fd(owner: &mut FileRecord, release: bool) -> c_int {
    let fd = owner.fd;
    if release {
        owner.fd = -1;
    }
    fd
}
#[cfg(test)]
mod tests {
    use super::*;

    /// Upstream's `file.open_twice`: `File::open` on an open File closes the old
    /// descriptor first, and a second `close` fails.
    #[test]
    fn file_open_twice() {
        // Upstream's fixed /tmp name, made unique per process so parallel runs
        // each create and remove their own file.
        let path =
            std::env::temp_dir().join(format!("swoole_file_open_twice.{}.txt", std::process::id()));
        let fname =
            std::ffi::CString::new(std::os::unix::ffi::OsStrExt::as_bytes(path.as_os_str()))
                .unwrap();
        let fname = fname.as_c_str();
        let mut file1 = FileRecord::empty();
        file1.open(fname, libc::O_WRONLY | libc::O_CREAT, 0);
        assert!(file1.ready());

        file1.open(fname, libc::O_RDONLY, 0);
        assert!(file1.ready());
        assert!(file1.close());
        assert!(!file1.close());

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri can't spawn a process")]
    fn owner_drop_preserves_a_reopened_file_descriptor_number() {
        if crate::isolated_process::run_in_child(
            "file_record::tests::owner_drop_preserves_a_reopened_file_descriptor_number",
        ) {
            return;
        }
        let mut owner = FileRecord::empty();
        owner.open(c"/dev/null", libc::O_RDONLY | libc::O_CLOEXEC, 0);
        let fd = owner.descriptor();
        assert!(fd >= 0);
        assert!(owner.close());
        assert_eq!(owner.descriptor(), -1);
        // SAFETY: `fd` is closed and this isolated child owns no other descriptor numbers.
        unsafe { crate::isolated_process::replacement_survives(fd, || drop(owner)) };
    }
}
