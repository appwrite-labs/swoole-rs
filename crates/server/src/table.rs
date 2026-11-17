//! A server's shared tables, typed: a mapping of zeroed records that every process
//! sharing it reads and writes in place, through `&`.
use core::cell::Cell;
use core::marker::PhantomData;
use swoole_rs_corebase::memory::members::Shm;

/// A record a [`Table`] maps: all-zero bytes are a value of it, and its fields are
/// cells and atomics, so it is written only through `&`. Sealed: only the types below.
pub(crate) trait Record: sealed::Sealed {}
impl<T: sealed::Sealed> Record for T {}

mod sealed {
    pub(crate) trait Sealed {}
    // Atomics, cells of integers, floats, raw pointers and fieldless enums whose zero
    // is a variant (`WebSocketStatus::None`), and `Option`s of non-null handles and
    // boxes, which zero makes `None`.
    impl Sealed for crate::records::ConnectionRecord {}
    impl Sealed for crate::records::SessionRecord {}
    impl Sealed for swoole_rs_corebase::process::worker_record::WorkerRecord {}
    impl Sealed for core::sync::atomic::AtomicU32 {}
    impl Sealed for crate::records::ResultWords {}
    // The counters' atomics, and the pool records and barrier, each of the same kinds
    // (`IpcMode::None` is zero), as `swr_v1_server_new` maps them.
    impl Sealed for crate::shared::ServerShared {}
}

/// A shared mapping (a [`Shm`]) of `len` zeroed `T`s, or none. Only [`Table::calloc`]
/// and [`Table::calloc_private`] map one, of `len` records, and only [`Table::fill`]
/// moves one into an empty table; only [`Table::free`], through `&mut`, unmaps it. So
/// the `len` records stay mapped while the table is borrowed.
pub(crate) struct Table<T> {
    shm: Shm,
    len: Cell<usize>,
    records: PhantomData<T>,
}

impl<T> Default for Table<T> {
    fn default() -> Self {
        Self::empty()
    }
}

impl<T> Table<T> {
    /// No mapping.
    #[must_use]
    pub(crate) const fn empty() -> Self {
        Self {
            shm: Shm::empty(),
            len: Cell::new(0),
            records: PhantomData,
        }
    }

    /// `count` records in `shm`, which maps them zeroed, or none when it is null.
    fn of(shm: Shm, count: usize) -> Self {
        let len = if shm.is_null() { 0 } else { count };
        Self {
            shm,
            len: Cell::new(len),
            records: PhantomData,
        }
    }

    /// `sw_shm_calloc(count, sizeof(T))`: empty when it fails.
    #[must_use]
    pub(crate) fn calloc(count: usize) -> Self {
        Self::of(Shm::calloc(count, size_of::<T>()), count)
    }

    /// As [`Table::calloc`], but a fork copies the mapping rather than sharing it.
    #[must_use]
    pub(crate) fn calloc_private(count: usize) -> Self {
        Self::of(Shm::calloc_private(count, size_of::<T>()), count)
    }

    /// The first record, or null without a mapping: what native code reads.
    #[must_use]
    pub(crate) fn as_ptr(&self) -> *mut T {
        self.shm.as_ptr().cast()
    }

    #[must_use]
    pub(crate) fn is_null(&self) -> bool {
        self.shm.is_null()
    }

    /// `table = mapping` for an empty table, which takes `mapping`: whether it did. A
    /// table already mapped keeps its mapping, which a reference may borrow, and
    /// `mapping` is unmapped instead.
    pub(crate) fn fill(&self, mapping: Self) -> bool {
        let len = mapping.len.get();
        let filled = self.shm.fill(mapping.shm);
        if filled {
            self.len.set(len);
        }
        filled
    }

    /// Limit the visible records without changing the mapping's allocation.
    pub(crate) fn truncate(&mut self, count: usize) {
        self.len.set(self.len.get().min(count));
    }

    /// `SharedMemory::free`: unmaps the mapping, if any, and leaves the table empty.
    pub(crate) fn free(&mut self) {
        self.shm.free();
        self.len.set(0);
    }

    /// `SharedMemory::protect`: `mprotect`s the whole mapping.
    ///
    /// # Errors
    /// `mprotect`'s errno, or `EINVAL` without a mapping.
    pub(crate) fn protect(&self, flags: core::ffi::c_int) -> std::io::Result<()> {
        self.shm.protect(flags)
    }
}

impl<T: Record> Table<T> {
    /// The records; none without a mapping.
    #[must_use]
    #[inline]
    pub(crate) fn records(&self) -> &[T] {
        const { assert!(align_of::<T>() <= 8, "a mapping's payload is 8-aligned") };
        let base = self.as_ptr();
        if base.is_null() {
            return &[];
        }
        // SAFETY: the type's invariant: `len` zeroed `T`s at `base`, which is 8-aligned,
        // mapped while `self` is borrowed. All-zero is a `T` and `T` is written only
        // through `&` (`Record`), so they stay `T`s.
        unsafe { core::slice::from_raw_parts(base, self.len.get()) }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A table naming a test's own `count` records at `items`, so several states can
    /// share them as processes share a mapping. Nothing frees it: the test owns them.
    ///
    /// # Safety
    /// `items` holds `count` records that outlive every borrow of the table, and the
    /// test never frees it.
    pub(crate) unsafe fn naming_at<T>(items: *mut T, count: usize) -> Table<T> {
        // SAFETY: `Shm` is `repr(transparent)` over its payload pointer.
        let shm = unsafe { core::mem::transmute::<*mut core::ffi::c_void, Shm>(items.cast()) };
        Table::of(shm, count)
    }

    /// [`naming_at`] over `items`.
    pub(crate) fn naming<T>(items: &mut [T]) -> Table<T> {
        // SAFETY: the test owns `items` and frees no table.
        unsafe { naming_at(items.as_mut_ptr(), items.len()) }
    }

    #[test]
    fn a_table_lends_its_records_until_it_is_freed() {
        let mut table = Table::<core::sync::atomic::AtomicU32>::calloc_private(3);
        assert_eq!(table.records().len(), 3);
        table.records()[1].store(7, core::sync::atomic::Ordering::Relaxed);
        assert!(
            !table.fill(Table::calloc_private(5)),
            "a mapped table keeps its mapping"
        );
        assert_eq!(
            table.records()[1].load(core::sync::atomic::Ordering::Relaxed),
            7
        );
        table.truncate(2);
        assert_eq!(table.records().len(), 2);
        table.truncate(5);
        assert_eq!(table.records().len(), 2, "a limit cannot grow the mapping");
        assert_eq!(
            table.records()[1].load(core::sync::atomic::Ordering::Relaxed),
            7
        );
        table.free();
        assert!(table.records().is_empty());
        assert!(table.fill(Table::calloc_private(2)), "an empty table fills");
        assert_eq!(table.records().len(), 2);
        table.free();
    }
}
