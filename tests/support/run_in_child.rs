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
