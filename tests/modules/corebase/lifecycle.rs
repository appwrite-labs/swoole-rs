//! Complete SDK initialization, idempotence, cleanup and reinitialization.
use swoole_rs_corebase as core;

pub(super) const CASES: super::Cases = &[("reinitialize", reinitialize)];

fn reinitialize() {
    for _ in 0..3 {
        assert!(core::swr_v1_core_initialized() && core::swr_v1_core_running());
        assert!(core::swr_v1_core_fatal_error().is_some());
        assert!(core::swr_v1_core_stdout().is_some());
        let logger = core::swr_v1_core_logger();
        let buffer = core::thread_buffer();
        let report = core::bug_report_message();
        assert!(!logger.is_null() && !buffer.is_null() && !report.is_empty());
        core::set_fatal_error(None);
        swoole_rs_ffi::swoole_init();
        assert!(core::swr_v1_core_fatal_error().is_none());
        assert_eq!(core::swr_v1_core_logger(), logger);
        assert_eq!(core::thread_buffer(), buffer);
        assert_eq!(core::bug_report_message(), report);
        core::swoole_clean();
        assert!(!core::swr_v1_core_initialized() && !core::swr_v1_core_running());
        assert!(core::swr_v1_core_logger().is_null());
        assert!(core::thread_buffer().is_null());
        assert!(core::swr_v1_core_stdout().is_none());
        assert!(core::bug_report_message().is_empty());
        assert_eq!(core::swoole_fork(8), -1);
        core::swoole_clean();
        swoole_rs_ffi::swoole_init();
    }
}
