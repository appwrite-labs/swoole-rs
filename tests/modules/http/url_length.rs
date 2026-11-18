#[test]
fn impossible_lengths_fail_before_reading_input() {
    let limit = (usize::MAX - 1) / 3;
    assert_eq!(super::check_url_length(limit), Ok(()));
    for length in [limit + 1, usize::MAX] {
        assert_eq!(super::check_url_length(length), Err(super::OVERFLOW));
    }
}
