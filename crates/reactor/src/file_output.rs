//! Sendfile requests: the file, its path and the range still to send, owned by
//! `Socket::sendfile_sync` or by the output queue entry `sendfile_async` pushes.
use crate::{OutputEffects, OutputResult};
use libc::{c_int, off_t};
use std::ffi::{CStr, c_void};
use swoole_rs_corebase::CCast;
use swoole_rs_ossupport::FileRecord;

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
    /// `path` opened read-only, to send from `offset`. A file that fails to open is
    /// still a request, which [`Self::check`] refuses; `None` when out of memory.
    #[must_use]
    pub fn open(path: &CStr, offset: off_t) -> Option<Box<Self>> {
        let mut request = swoole_rs_buffer::try_box(|| Self {
            file: FileRecord::empty(),
            open_error: 0,
            fields: SocketFileFields {
                begin: offset,
                ..SocketFileFields::default()
            },
        })?;
        let file = FileRecord::open_readonly(path)?;
        if file.descriptor() == -1 {
            request.open_error = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        }
        request.file = file;
        Some(request)
    }

    /// The output queue's disposer for a request it holds as `Box::into_raw`.
    ///
    /// # Safety
    /// `token` is such a request, not yet freed.
    pub unsafe extern "C" fn dispose(token: *mut c_void) {
        // SAFETY: as the caller promises.
        drop(unsafe { Box::from_raw(token.cast::<Self>()) });
    }

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

    /// Range validation over the descriptor, `length` 0 meaning to the end of the
    /// file: 0 valid, 1 open, 2 stat, 3 empty, 4 range.
    pub fn check(&mut self, length: usize) -> c_int {
        if self.file.descriptor() == -1 {
            crate::socket_sync::set_errno(self.open_error);
            return 1;
        }
        let status = crate::sendfile_range(
            self.file.descriptor(),
            self.fields.begin,
            length,
            &mut self.fields.end,
        );
        if status == 0 { 0 } else { status + 1 }
    }

    /// The remaining range, with the original unsigned size conversion.
    #[must_use]
    pub fn length(&self) -> usize {
        {
            self.fields
                .end
                .wrapping_sub(self.fields.begin)
                .c_cast::<usize>()
        }
    }

    /// `Socket::handle_sendfile` before its send: [`crate::sendfile_cork`].
    pub fn cork(&mut self, hooks: &impl OutputEffects) {
        let SocketFileFields { begin, end, .. } = self.fields;
        crate::sendfile_cork(
            hooks,
            &mut self.fields.corked,
            begin,
            end,
            i64::from(swoole_rs_corebase::constants::SW_SOCKET_CORK_MIN_SIZE),
        );
    }

    /// `Socket::handle_sendfile` after its send: [`crate::sendfile_sent`].
    pub fn sent(
        &mut self,
        hooks: &impl OutputEffects,
        sent: isize,
        requested: usize,
    ) -> OutputResult {
        let SocketFileFields { begin, end, .. } = self.fields;
        crate::sendfile_sent(hooks, sent, requested, &mut self.fields.corked, begin, end)
    }
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
        let mut request = SocketFileRequest::open(&name, 2).unwrap();
        let fd = request.descriptor();
        let file = |fd| {
            let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
            (unsafe { libc::fstat(fd, &raw mut stat) } == 0).then_some((stat.st_dev, stat.st_ino))
        };
        let opened = file(fd);
        assert!(opened.is_some());
        assert_eq!(request.check(0), 0);
        assert_eq!(request.fields().end, 6);
        assert_eq!(request.length(), 4);
        drop(request);
        // Closed: a test running in parallel may reuse the number at once, but
        // then it names another file.
        assert_ne!(file(fd), opened);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn failed_open_remains_a_request_and_invalid_ranges_match_legacy() {
        let missing = c"/nonexistent/swoole-sendfile-request";
        assert_eq!(SocketFileRequest::open(missing, 0).unwrap().check(0), 1);
        let path =
            std::env::temp_dir().join(format!("swoole-request-range-{}", std::process::id()));
        std::fs::write(&path, b"abcdef").unwrap();
        let name = CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        let mut request = SocketFileRequest::open(&name, -1).unwrap();
        assert_eq!(request.check(0), 4);
        request.fields().begin = 1;
        assert_eq!(request.check(3), 0);
        assert_eq!(request.fields().end, 4);
        drop(request);
        std::fs::remove_file(path).unwrap();
    }
}
