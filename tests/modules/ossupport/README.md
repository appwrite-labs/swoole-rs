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

`core-tests/src/os/file.cpp` tests `src/os/file.cc`, which is Rust
(`crates/ossupport/src/file.rs`, `file_api.rs`, `file_record.rs`):

| Upstream TEST | Where it runs |
| --- | --- |
| `file.open_twice` | `file_record::tests::file_open_twice`, over `FileRecord::open`/`close` (`cargo test -p swoole-rs-ossupport file_open_twice`) |
| the other eight | not yet ported (#3043) |
