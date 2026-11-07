# ossupport tests

## Ported GoogleTests

`core-tests/src/os/msg_queue.cpp` no longer builds: `include/swoole_msg_queue.h`
left the build. Its TEST is a Rust test over `crates/ossupport/src/msg_queue.rs`
(`cargo test -p swoole-rs-ossupport msg_queue`), with the same inputs and expected
values:

| Upstream TEST | Rust test |
| --- | --- |
| `msg_queue.rbac` | `msg_queue::tests::msg_queue_rbac` |
