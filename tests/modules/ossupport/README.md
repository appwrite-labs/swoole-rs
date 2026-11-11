# ossupport tests

## Ported GoogleTests

`core-tests/src/os/msg_queue.cpp` no longer builds: `include/swoole_msg_queue.h`
left the build. Its TEST is a Rust test over `crates/ossupport/src/msg_queue.rs`
(`cargo test -p swoole-rs-ossupport msg_queue`), with the same inputs and expected
values:

| Upstream TEST | Rust test |
| --- | --- |
| `msg_queue.rbac` | `msg_queue::tests::msg_queue_rbac` |

`core-tests/src/os/os.cpp` tests `src/os/base.cc`, which is Rust
(`crates/ossupport/src/system.rs`); its TESTs are tests there, same inputs and expected
values (`cargo test -p swoole-rs-ossupport --all-features os_`):

| Upstream TEST | Rust test or reason |
| --- | --- |
| `os.daemon` | `os.daemon` in `tests/modules/hooks/system/src/os.rs` (`hooks-system-rust`), since it needs `swoole_init` |
| `os.cpu_affinity` | `system::tests::os_cpu_affinity` (Linux). `swoole_set_cpu_affinity` left the build (crates/process `affinity.rs` pins workers), so the case sets through its body, `sched_setaffinity(getpid())` |
| `os.thread_name` | `system::tests::os_thread_name` (Linux, whose 15-byte limit fails the second name) |
| `os.thread_id` | none: `swoole_thread_id_to_str` left the build with its one caller, the async-thread warnings, so there is no function to test |
| `os.set_isolation` | `system::tests::os_set_isolation`, over `swr_v1_os_set_isolation` |

`core-tests/src/os/file.cpp` tests `src/os/file.cc`, which is Rust (`crates/ossupport`'s
`file.rs`, `file_api.rs` and `file_record.rs`). Its `ASSERT_ERREQ`s read the last error, which only the log
host `swoole_init` installs sets, so its TESTs are `file.<name>` in
`tests/modules/hooks/system/src/file.rs` (`hooks-system-rust`), same inputs and expected values:

| Upstream TEST | Rust test or reason |
| --- | --- |
| `file.read_line` | none: `File::read_line` left the build (not in `native/ossupport/swoole_file.h`, no Rust body, no caller) |
| `file.read_line_no_crlf` | none, as `file.read_line` |
| `file.file_put_contents` | `file.file_put_contents`, over `file_api::file_put_contents` |
| `file.file_get_contents` | `file.file_get_contents`, over `file_api::file_get_contents`; the large file is the test binary, as upstream's is core-tests' |
| `file.file_get_size` | `file.file_get_size`, over `file::file_size` (`file_get_size(int)`), which returns the error its caller publishes. The path overload left the build, so its ENOENT check has no subject |
| `file.open_twice` | `file_record::tests::file_open_twice`, over `FileRecord::open`/`close` (`cargo test -p swoole-rs-ossupport file_open_twice`) |
| `file.error` | `file.error`: `FileRecord::read_all`'s failure and errno (its callers warn), `fd_write_all` (`File::write_all`, which warns), and `file_size`'s `fstat` for `File::stat` |
| `file.tmp_file` | `file.tmp_file` (`swoole_tmpfile`), and crates/process's `task_file::tests::make_tmpfile_in_a_missing_directory` for `make_tmpfile` |
| `file.empty_file` | `file.empty_file`, over `FileRecord::read_all` |
