use super::*;

#[test]
#[cfg_attr(miri, ignore = "Miri has no shared mappings")]
fn bump_pages_match_alignment_zeroing_capacity_and_growth() {
    for shared in [false, true] {
        let mut pages = Global::new(64, shared);
        assert_eq!(pages.memory_size(), 0);
        let first = pages.alloc(1);
        assert!(!first.is_null());
        assert_eq!(pages.memory_size(), 64);
        let page = first.wrapping_byte_sub(8);
        for i in 0..4 {
            let block = if i == 0 { first } else { pages.alloc(1) };
            assert_eq!(block, page.wrapping_byte_add(i * 16 + 8));
            unsafe {
                assert_eq!(block.cast::<u64>().read(), 0);
                assert_eq!(block.wrapping_byte_sub(8).cast::<u32>().read(), 8);
                block.cast::<u64>().write(42);
            }
            assert_eq!(pages.capacity(), 64 - (i + 1) * 16);
        }
        // A full page starts a new one; the old one keeps its blocks.
        assert!(!pages.alloc(1).is_null());
        assert_eq!(pages.memory_size(), 128);
        assert_eq!(pages.capacity(), 48);
        assert_eq!(unsafe { first.cast::<u64>().read() }, 42);
    }
}

#[test]
fn oversized_requests_never_fit_and_page_sizes_round_to_system_pages() {
    let pages = Global::new(64, false);
    assert!(pages.fits(56));
    assert!(!pages.fits(57));
    assert!(!pages.fits(u32::MAX));
    assert_eq!(page_size(4096, 16384), Some(16384));
    assert_eq!(page_size(4097, 4096), Some(8192));
    assert_eq!(page_size(4096, 3), None);
    assert_eq!(page_size(u32::MAX, 16384), None);
}
