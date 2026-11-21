// Descriptor-number assertions need a private process FD table: parallel tests
// may reopen a successfully closed number before fcntl observes it.
pub(crate) fn run_in_child(case: &str) -> bool {
    const CHILD: &str = "SWOOLE_RS_ISOLATED_TEST_CASE";
    if std::env::var(CHILD).ok().as_deref() == Some(case) {
        return false;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", case, "--nocapture", "--test-threads=1"])
        .env(CHILD, case)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{case}:\n{stdout}\n{stderr}");
    assert!(
        stdout.contains("1 passed; 0 failed; 0 ignored"),
        "child must execute the exact original test: {case}:\n{stdout}\n{stderr}"
    );
    true
}

pub(crate) unsafe fn replacement_survives(fd: libc::c_int, release: impl FnOnce()) {
    unsafe {
        assert_eq!(libc::fcntl(fd, libc::F_GETFD), -1);
        let replacement = std::thread::spawn(move || {
            let replacement = libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC);
            assert_eq!(replacement, fd);
            replacement
        })
        .join()
        .unwrap();
        assert_eq!(libc::fcntl(replacement, libc::F_GETFD), libc::FD_CLOEXEC);
        let mut info: libc::stat = std::mem::zeroed();
        assert_eq!(libc::fstat(replacement, &raw mut info), 0);
        assert_eq!(info.st_mode & libc::S_IFMT, libc::S_IFCHR);
        release();
        assert_eq!(libc::fcntl(replacement, libc::F_GETFD), libc::FD_CLOEXEC);
        assert_eq!(libc::close(replacement), 0);
    }
}
