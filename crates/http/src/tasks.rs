//! Sole persistent `StaticHandler` task storage. Native owns one local immutable
//! send snapshot and executes RNG/MIME/date/allocation/IO effects after return.
use crate::range_response::{HttpRangeFormat, swr_v1_http_range_format};

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HttpStaticTask {
    pub offset: i64,
    pub length: usize,
    pub part_header: [u8; 256],
}
impl Default for HttpStaticTask {
    fn default() -> Self {
        Self {
            offset: 0,
            length: 0,
            part_header: [0; 256],
        }
    }
}
#[derive(Default)]
pub struct HttpTasks {
    items: Vec<HttpStaticTask>,
    scratch: HttpStaticTask,
}
const ALLOCATION: i32 = -3;

impl HttpTasks {
    /// Reset per-parse scratch, retaining previously committed tasks exactly as the
    /// original method's local task initialization and persistent vector do.
    pub(crate) fn begin(&mut self) {
        self.scratch = HttpStaticTask::default();
    }
    /// Commit the prepared value with fallible admission; error publishes no task.
    pub(crate) fn push(&mut self) -> i32 {
        if self.items.try_reserve(1).is_err() {
            return ALLOCATION;
        }
        self.items.push(self.scratch);
        0
    }
    pub(crate) fn count(&self) -> usize {
        self.items.len()
    }
    /// The completed tasks, which callers copy.
    pub(crate) fn items(&self) -> &[HttpStaticTask] {
        &self.items
    }
    /// Clear completed task values, retaining capacity and scratch for the original
    /// If-Range full-file replacement.
    pub(crate) fn clear(&mut self) {
        self.items.clear();
    }
    /// Prepare a full-file task: copied metadata only, keeping the previous scratch
    /// header bytes. This does not commit a task.
    pub(crate) fn prepare_full(&mut self, offset: i64, length: usize) {
        self.scratch.offset = offset;
        self.scratch.length = length;
    }
    /// Prepare a range task: copied metadata and the part's header in the scratch
    /// task, formatted with the range formatter's byte/errno-count policy, including
    /// retained initialized tail bytes. This does not commit a task.
    ///
    /// # Safety
    /// `format`'s spans name readable, initialized bytes.
    pub(crate) unsafe fn prepare_range(
        &mut self,
        offset: i64,
        length: usize,
        format: &HttpRangeFormat,
        written: &mut usize,
    ) -> i32 {
        self.prepare_full(offset, length);
        // SAFETY: the caller's readable spans; the scratch header holds 256 writable
        // bytes, and `written` is a live exclusive output.
        unsafe {
            swr_v1_http_range_format(format, self.scratch.part_header.as_mut_ptr(), 256, written)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::HttpProxyText;
    #[test]
    fn owned_tasks_copy_headers_and_snapshot_survives_owner_mutation() {
        let mut owner = HttpTasks::default();
        owner.begin();
        let mut boundary = b"fixed-boundary".to_vec();
        let format = HttpRangeFormat {
            boundary: HttpProxyText {
                data: boundary.as_ptr(),
                length: boundary.len(),
            },
            mime: HttpProxyText {
                data: b"text/plain".as_ptr(),
                length: 10,
            },
            offset: 3,
            length: 5,
            filesize: 100,
            part: 0,
            separator: 0,
        };
        let mut written = 0;
        assert_eq!(
            unsafe { owner.prepare_range(3, 5, &format, &mut written) },
            0
        );
        assert!(written > 0);
        assert_eq!(owner.push(), 0);
        boundary.fill(b'x');
        let snapshot = owner.items()[0];
        assert!(
            snapshot
                .part_header
                .windows(14)
                .any(|s| s == b"fixed-boundary")
        );
        owner.clear();
        assert_eq!(owner.count(), 0);
        owner.prepare_full(0, 100);
        assert_eq!(owner.push(), 0);
        let full = owner.items()[0];
        assert_eq!(full.part_header, snapshot.part_header);
        assert_eq!(full.offset, 0);
        assert_eq!(full.length, 100);
        assert_eq!(snapshot.offset, 3);
        assert_eq!(snapshot.length, 5);
        assert_eq!(owner.items(), [full]);
        assert!(owner.items().get(1).is_none());
        owner.begin();
        assert_eq!(owner.count(), 1);
        drop(owner);
        assert!(
            snapshot
                .part_header
                .windows(14)
                .any(|s| s == b"fixed-boundary")
        );
    }
}
