//! Both fork branches return from main so `LeakSanitizer` checks their own shutdown.
use std::sync::atomic::{AtomicU32, Ordering::SeqCst};
use swoole_rs_corebase::{memory::members::pool, sys};

fn main() -> std::process::ExitCode {
    assert!(
        swoole_rs_corebase::swr_v1_core_init(),
        "initialize the runtime"
    );
    pool::init();
    let Some(word) = pool::PooledWord::<AtomicU32>::allocate() else {
        return std::process::ExitCode::FAILURE;
    };
    let size = swoole_rs_corebase::swr_v1_core_pagesize() as usize;
    let page = std::ptr::from_ref(&*word)
        .cast_mut()
        .cast::<libc::c_void>()
        .map_addr(|addr| addr & !(size - 1));
    let probe = || unsafe { libc::mprotect(page, 1, libc::PROT_READ | libc::PROT_WRITE) };
    assert_eq!(probe(), 0, "new page is mapped");
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0, "fork the page owner");
    if pid == 0 {
        assert!(pool::memory_size() > 0, "child installed its own pool");
        pool::delete();
        word.store(73, SeqCst);
    } else {
        let mut status = 0;
        assert!(
            matches!(sys::waitpid(pid, Some(&mut status), 0), Ok(Some(reaped)) if reaped == pid),
            "reaped the forked child"
        );
        assert_eq!(status, 0, "child released its page and exited cleanly");
        assert_eq!(word.load(SeqCst), 73, "parent sees the child’s update");
        pool::delete();
    }
    assert_eq!(probe(), 0, "word still owns its page");
    drop(word);
    assert_eq!(probe(), -1, "last owner released the mapping");
    assert_eq!(sys::errno(), libc::ENOMEM, "page is unmapped");
    swoole_rs_corebase::swoole_clean();
    std::process::ExitCode::SUCCESS
}
