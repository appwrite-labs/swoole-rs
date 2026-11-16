//! Exercise the production Allocator with a checked request-heap shim.

#[path = "../../../crates/zend/src/allocator.rs"]
mod allocator;
pub use allocator::Emalloc;
#[cfg(test)]
mod tests {
    use super::Emalloc;
    use std::{
        alloc::{Allocator, Global, Layout},
        cell::{Cell, RefCell},
        ptr::NonNull,
        rc::Rc,
    };

    thread_local! {
        static LIVE: RefCell<Vec<(usize, Layout)>> = const { RefCell::new(Vec::new()) };
    }

    #[unsafe(no_mangle)]
    extern "C" fn swr_zend_emalloc(size: usize) -> NonNull<u8> {
        let layout = Layout::from_size_align(size, 8).expect("valid request layout");
        let allocation = Global
            .allocate(layout)
            .expect("test allocation succeeds")
            .cast::<u8>();
        LIVE.with_borrow_mut(|live| {
            let address = allocation.as_ptr().addr();
            assert!(
                !live.iter().any(|(at, _)| *at == address),
                "fresh allocation has a unique address"
            );
            live.push((address, layout));
        });
        allocation
    }

    #[unsafe(no_mangle)]
    extern "C" fn swr_zend_efree(allocation: NonNull<u8>) {
        let layout = LIVE.with_borrow_mut(|live| {
            let index = live
                .iter()
                .position(|(at, _)| *at == allocation.as_ptr().addr())
                .expect("free an owned allocation exactly once");
            live.swap_remove(index).1
        });
        unsafe { Global.deallocate(allocation, layout) };
    }

    #[test]
    fn zero_size_and_alignment_do_not_touch_the_heap() {
        assert_eq!(size_of::<Emalloc>(), 0);
        let allocator = Emalloc::default();
        for align in [1, 2, 4, 8] {
            let layout = Layout::from_size_align(0, align).unwrap();
            let allocation = allocator.allocate(layout).unwrap();
            assert_eq!(allocation.len(), 0);
            assert_eq!(allocation.as_ptr().cast::<u8>().addr() % align, 0);
            unsafe { allocator.deallocate(allocation.cast(), layout) };
        }
        for size in [0, 1, 64] {
            assert!(
                allocator
                    .allocate(Layout::from_size_align(size, 16).unwrap())
                    .is_err()
            );
        }
        LIVE.with_borrow(|live| assert_eq!(live.as_slice(), []));
    }

    #[test]
    fn boxed_value_drops_once_and_frees_its_request_allocation() {
        struct Count(Rc<Cell<usize>>);
        impl Drop for Count {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }
        let drops = Rc::new(Cell::new(0));
        let value = Box::new_in(Count(drops.clone()), Emalloc::default());
        LIVE.with_borrow(|live| assert_eq!(live.len(), 1));
        let (raw, allocator) = Box::into_raw_with_allocator(value);
        drop(unsafe { Box::from_raw_in(raw, allocator) });
        assert_eq!(drops.get(), 1);
        LIVE.with_borrow(|live| assert_eq!(live.as_slice(), []));
    }

    #[test]
    fn vector_growth_and_shrink_preserve_values_and_release_every_block() {
        let mut values = Vec::new_in(Emalloc::default());
        values.extend(0_u64..4096);
        assert!(values.iter().copied().eq(0..4096));
        values.truncate(17);
        values.shrink_to_fit();
        assert!(values.iter().copied().eq(0..17));
        assert_eq!(values.capacity(), 17);
        LIVE.with_borrow(|live| assert_eq!(live.len(), 1));
        drop(values);
        LIVE.with_borrow(|live| assert_eq!(live.as_slice(), []));
    }
}
