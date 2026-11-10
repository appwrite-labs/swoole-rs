# Reactor tests

`reactor-unit` runs `crates/reactor`'s tests.
`network/socket.cpp`'s TESTs are Rust cases in `hooks-system-rust`
(tests/modules/hooks/README.md), so the `reactor` core-tests lane
(`reactor-native-originals`) is gone.

`reactor/base.cpp`'s 26 TESTs are Rust (`base/`, the merge tier's `reactor-base-rust`
phase): a case binary over the crates and system.py's static libswoole.a (the pin's thirdparty and native/'s Zend-free sources), built and run by
`tests/modules/hooks/system.py --package tests/modules/reactor/base --fixture
tests/modules/reactor/base/fixture.cc`. `fixture.cc` gives C entry points to what needs
C++ (the `Reactor` constructor, members and `std::function` slots, `Pipe` and
`UnixSocket`, and the C++-linkage event, hook, signal and timer calls); the handlers,
callbacks and every assertion are in `base/src/main.rs`, one function per TEST under
its own name (`write` is `write_case`): `create`, `create_failure`, `set_handler`,
`wait`, `get_missing_socket`, `write`, `wait_timeout`, `wait_error`, `writev`,
`write_2m`, `bad_fd`, `epoll`, `poll`, `poll_readiness_after_fd_reuse`, `poll_extra`,
`poll_extra2`, `add_or_update`, `defer_task`, `cycle`, `priority_idle_task`, `hook`,
`set_fd`, `error_event`, `error`, `drain_write_buffer`, `handle_fail`. Each case runs
in its own process, so the hook-table save and restore upstream wraps around
`create_failure` and `hook` has nothing to do. A failed EXPECT is printed and fails the
case at exit, as GoogleTest's nonfatal assertions do.

`network/stream.cpp`'s TESTs are Rust tests of `network::Stream`, which is Rust
(`crates/process/src/stream.rs`: `cargo test -p swoole-rs-process stream::`). A
socket pool's dispatch was its only production user; `src/network/stream.cc` and
the TESTs left the build (#1674).

| Upstream TEST | Rust test | Same input | Assertions |
| --- | --- | --- | --- |
| `recv_sync_rejects_truncated_packet` | `recv_sync_rejects_truncated_packet` | a Unix socket pair: 3 bytes of a 2048 length, then length 8 and `ab`, each then `SHUT_WR`; a 2048-byte buffer | `recv_sync` is `SW_ERR` both times |
| `send` | `send` | 64 KiB of bytes; a regular file as the Unix path, TCP port 39999 with nothing listening, a length-checked TCP listener | the file's stream is null; port 39999's is created, then its response is `nullptr` with `errCode` `ECONNREFUSED`; the server reads the body sent and the response is `"Server: "` + body |

The server in `send` is a plain TCP listener replying as upstream's `Server`
`onReceive` does (`"Server: "` + body, framed, then a zero-length frame). The
Rust test also checks that each response callback runs exactly once.
