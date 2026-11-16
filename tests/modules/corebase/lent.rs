//! Lent output counts stay within both the allocation and the syscall's count type.
use super::*;

#[test]
fn limited_output_counts_never_grow_or_wrap() {
    let mut slots = [0u8; 8];
    for (limit, expected) in [(0, 0), (3, 3), (8, 8), (9, 8), (usize::MAX, 8)] {
        lend_out(&mut slots, |_, count: Count<'_, c_int>| {
            assert_eq!(count.at_most(limit).0, expected);
        });
        lend_out(&mut slots, |_, count: Count<'_>| {
            assert_eq!(count.at_most(limit).0, usize::try_from(expected).unwrap());
        });
    }
    let empty: &mut [u8] = &mut [];
    lend_out(empty, |_, count: Count<'_, c_int>| {
        assert_eq!(count.at_most(usize::MAX).0, 0);
    });
}
