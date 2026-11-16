use super::*;

#[test]
fn a_thread_without_a_buffer_lends_none() {
    // A new thread never ran `swoole_thread_init`, so its `sw_tg_buffer()` is null.
    std::thread::spawn(|| {
        assert_eq!(try_with_thread_buffer(|buffer| buffer.size()), None);
        // The lend ended with the first call, so a second one isn't refused as nested.
        assert_eq!(try_with_thread_buffer(|_| ()), None);
    })
    .join()
    .unwrap();
}
