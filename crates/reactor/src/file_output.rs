//! Queued and synchronous sendfile requests: descriptor/path lifetime, cork state,
//! range validation and the actual file transfer belong to one Rust owner.
use crate::{OutputHooks, OutputResult, SocketRecord};
use libc::{c_char, c_int, off_t};
use std::{ffi::CStr, ptr};
use swoole_rs_ossupport::FileRecord;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SocketFileFields {
    pub corked: i8,
    pub begin: off_t,
    pub end: off_t,
}

pub struct SocketFileRequest {
    file: FileRecord,
    open_error: c_int,
    fields: SocketFileFields,
}

impl SocketFileRequest {
    /// The file's descriptor, -1 when it failed to open.
    #[must_use]
    pub const fn descriptor(&self) -> c_int {
        self.file.descriptor()
    }

    /// The path it was opened with.
    #[must_use]
    pub fn path(&self) -> &[u8] {
        self.file.path()
    }

    /// The range still to send and the cork state.
    pub const fn fields(&mut self) -> &mut SocketFileFields {
        &mut self.fields
    }
}

/// Stable slots for the native compatibility view; the request owns all of them.
#[repr(C)]
pub struct SocketFilePrepared {
    pub owner: *mut SocketFileRequest,
    pub file: *mut FileRecord,
    pub fields: *mut SocketFileFields,
}

impl SocketFilePrepared {
    fn refused() -> Self {
        Self {
            owner: ptr::null_mut(),
            file: ptr::null_mut(),
            fields: ptr::null_mut(),
        }
    }
    fn publish(mut request: Box<SocketFileRequest>) -> Self {
        let file = &raw mut request.file;
        let fields = &raw mut request.fields;
        Self {
            owner: Box::into_raw(request),
            file,
            fields,
        }
    }
}

/// Construct without range validation, matching the public request constructor.
/// Null filename creates the empty destination used by the guarded native copy constructor.
/// # Safety
/// filename is null or a readable terminated path. The resulting owner is released once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socket_file_new(
    filename: *const c_char,
    offset: off_t,
) -> SocketFilePrepared {
    let Some(mut request) = crate::owner::try_owner(|| SocketFileRequest {
        file: FileRecord::empty(),
        open_error: 0,
        fields: SocketFileFields {
            begin: offset,
            ..SocketFileFields::default()
        },
    }) else {
        return SocketFilePrepared::refused();
    };
    if !filename.is_null() {
        // SAFETY: the caller lends a readable terminated filename.
        let path = unsafe { CStr::from_ptr(filename) };
        let Some(file) = FileRecord::open_readonly(path) else {
            return SocketFilePrepared::refused();
        };
        if file.descriptor() == -1 {
            request.open_error = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        }
        request.file = file;
    }
    SocketFilePrepared::publish(request)
}

/// Drop the request and its descriptor after the native views have torn down.
/// # Safety
/// owner is null or this API's unique live request, and no borrowed view remains.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socket_file_free(owner: *mut SocketFileRequest) {
    // SAFETY: forwards the unique owner contract.
    unsafe { crate::owner::release(owner) };
}

/// Range validation over the owned descriptor: 0 valid,1 open,2 stat,3 empty,4 range.
/// # Safety
/// owner is live/exclusive.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socket_file_check(
    owner: *mut SocketFileRequest,
    length: usize,
) -> c_int {
    // SAFETY: the request is live/exclusive; no native callback occurs here.
    let owner = unsafe { &mut *owner };
    if owner.file.descriptor() == -1 {
        crate::socket_sync::set_errno(owner.open_error);
        return 1;
    }
    let status = crate::sendfile_range(
        owner.file.descriptor(),
        owner.fields.begin,
        length,
        &mut owner.fields.end,
    );
    if status == 0 { 0 } else { status + 1 }
}

/// Remaining range uses the original unsigned size conversion.
/// # Safety
/// owner is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socket_file_length(owner: *const SocketFileRequest) -> usize {
    // SAFETY: the request remains live for this immutable observation.
    let fields = unsafe { (*owner).fields };
    #[expect(
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation,
        reason = "the native off_t difference converts to size_t"
    )]
    {
        fields.end.wrapping_sub(fields.begin) as usize
    }
}

/// # Safety
/// owner and hooks are live; hooks obey `OutputHooks`' nonthrowing contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socket_file_cork(
    owner: *mut SocketFileRequest,
    hooks: *const OutputHooks,
) {
    // SAFETY: copy range facts without borrowing across a native cork effect.
    let fields = unsafe { (*owner).fields };
    // SAFETY: owner contains the stable cork slot.
    let corked = unsafe { &raw mut (*owner).fields.corked };
    // SAFETY: the native effect accepts its socket; no owner reference is held.
    unsafe {
        crate::swr_v1_socket_sendfile_cork(
            hooks,
            corked,
            fields.begin,
            fields.end,
            i64::from(swoole_rs_corebase::constants::SW_SOCKET_CORK_MIN_SIZE),
        );
    };
}

/// Complete classification/corking and publish socket wait flags in Rust.
/// # Safety
/// All owners are live/exclusive; hooks obey their nonthrowing contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socket_file_sent(
    owner: *mut SocketFileRequest,
    socket: *mut SocketRecord,
    hooks: *const OutputHooks,
    sent: isize,
    requested: usize,
) -> OutputResult {
    // SAFETY: copy range facts without borrowing across native effects.
    let fields = unsafe { (*owner).fields };
    // SAFETY: the stable request cork slot.
    let corked = unsafe { &raw mut (*owner).fields.corked };
    // SAFETY: all owners and effects remain live; no record borrow crosses effects.
    let result = unsafe {
        crate::swr_v1_socket_sendfile_sent(hooks, sent, requested, corked, fields.begin, fields.end)
    };
    if result.close_wait {
        // SAFETY: effects returned; the caller still lends the socket exclusively.
        unsafe { (*socket).output.close_wait = 1 };
    }
    if result.send_wait {
        // SAFETY: effects returned; the caller still lends the socket exclusively.
        unsafe { (*socket).output.send_wait = 1 };
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn request_owns_descriptor_and_range() {
        let path = std::env::temp_dir().join(format!("swoole-request-{}", std::process::id()));
        std::fs::write(&path, b"abcdef").unwrap();
        let name = CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        unsafe {
            let request = swr_v1_socket_file_new(name.as_ptr(), 2);
            assert!(!request.owner.is_null());
            let fd = (*request.file).descriptor();
            assert_eq!(swr_v1_socket_file_check(request.owner, 0), 0);
            assert_eq!((*request.fields).end, 6);
            assert_eq!(swr_v1_socket_file_length(request.owner), 4);
            swr_v1_socket_file_free(request.owner);
            assert_eq!(libc::fcntl(fd, libc::F_GETFD), -1);
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::EBADF)
            );
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn failed_open_remains_a_request_and_invalid_ranges_match_legacy() {
        let missing = CString::new("/nonexistent/swoole-sendfile-request").unwrap();
        unsafe {
            let request = swr_v1_socket_file_new(missing.as_ptr(), 0);
            assert!(!request.owner.is_null());
            assert_eq!(swr_v1_socket_file_check(request.owner, 0), 1);
            swr_v1_socket_file_free(request.owner);
        }
        let path =
            std::env::temp_dir().join(format!("swoole-request-range-{}", std::process::id()));
        std::fs::write(&path, b"abcdef").unwrap();
        let name = CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        unsafe {
            let request = swr_v1_socket_file_new(name.as_ptr(), -1);
            assert_eq!(swr_v1_socket_file_check(request.owner, 0), 4);
            (*request.fields).begin = 1;
            assert_eq!(swr_v1_socket_file_check(request.owner, 3), 0);
            assert_eq!((*request.fields).end, 4);
            swr_v1_socket_file_free(request.owner);
        }
        std::fs::remove_file(path).unwrap();
    }
}
