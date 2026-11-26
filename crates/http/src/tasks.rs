//! Sole persistent `StaticHandler` task storage. Native owns one local immutable
//! send snapshot and executes RNG/MIME/date/allocation/IO effects after return.
use crate::range_response::{HttpRangeFormat, range_format};

/// Maximum number of file sends retained by a static response.
pub const MAX_RANGE_TASKS: usize = 256;
const PART_HEADER: usize = 256;
#[cfg(test)]
const _: () = assert!(
    PART_HEADER == swoole_rs_oracle::http_range::PART_HEADER,
    "static range task must match SW_HTTP_SERVER_PART_HEADER"
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HttpStaticTask {
    pub offset: i64,
    pub length: usize,
    pub part_header: [u8; PART_HEADER],
}
impl Default for HttpStaticTask {
    fn default() -> Self {
        Self {
            offset: 0,
            length: 0,
            part_header: [0; PART_HEADER],
        }
    }
}
#[derive(Default)]
pub(crate) struct HttpTasks {
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
    #[cfg_attr(
        protocol_buffer_lint,
        expect(
            clippy::disallowed_methods,
            reason = "both task count and requested capacity are capped at MAX_RANGE_TASKS"
        )
    )]
    pub(crate) fn push(&mut self) -> Result<(), i32> {
        let count = self.items.len();
        if count >= MAX_RANGE_TASKS {
            return Err(crate::OVERFLOW);
        }
        if count == self.items.capacity() {
            let capacity = (count * 2).clamp(4, MAX_RANGE_TASKS);
            self.items
                .try_reserve_exact(capacity - count)
                .map_err(|_| ALLOCATION)?;
        }
        self.items.push(self.scratch);
        Ok(())
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
    pub(crate) fn prepare_range(
        &mut self,
        offset: i64,
        length: usize,
        format: &HttpRangeFormat<'_>,
        written: &mut usize,
    ) -> Result<bool, i32> {
        self.prepare_full(offset, length);
        range_format(format, &mut self.scratch.part_header, written)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn owned_tasks_copy_headers_and_snapshot_survives_owner_mutation() {
        let mut owner = HttpTasks::default();
        owner.begin();
        let mut boundary = b"fixed-boundary".to_vec();
        let format = HttpRangeFormat {
            boundary: &boundary,
            mime: b"text/plain",
            offset: 3,
            length: 5,
            filesize: 100,
            part: 0,
            separator: false,
        };
        let mut written = 0;
        assert_eq!(owner.prepare_range(3, 5, &format, &mut written), Ok(false));
        assert!(written > 0);
        assert_eq!(owner.push(), Ok(()));
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
        assert_eq!(owner.push(), Ok(()));
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
