# Reactor tests

`reactor-unit` runs `crates/reactor`'s tests.
`network/socket.cpp`'s TESTs are Rust cases in `hooks-system-rust`
(tests/modules/hooks/README.md), so the `reactor` core-tests lane
(`reactor-native-originals`) is gone.

`reactor/base.cpp`'s 26 TESTs are Rust (`base/`, the merge tier's `reactor-base-rust`
phase): a case binary over the crates and system.py's static libswoole.a (the pin's thirdparty and native/'s Zend-free sources), built and run by
`tests/modules/hooks/system.py --package tests/modules/reactor/base`. The cases call
the crates' Rust API by path, with no C++ fixture: one function per TEST under its own
name (`write` is `write_case`, `error` is `error_case`), with upstream's inputs, counts
and expected values: `create`, `create_failure`, `set_handler`, `wait`,
`get_missing_socket`, `write`, `wait_timeout`, `wait_error`, `writev`, `write_2m`,
`bad_fd`, `epoll`, `poll`, `poll_readiness_after_fd_reuse`, `poll_extra`,
`poll_extra2`, `add_or_update`, `defer_task`, `cycle`, `priority_idle_task`, `hook`,
`set_fd`, `error_event`, `error`, `drain_write_buffer`, `handle_fail`. Each case runs
in its own process, so the hook-table save and restore upstream wraps around
`create_failure` and `hook` has nothing to do. A failed EXPECT is printed and fails the
case at exit, as GoogleTest's nonfatal assertions do.

| Upstream call | Rust the cases call |
| --- | --- |
| `new Reactor(n, type)`, `delete reactor` | `swr_v1_reactor_new`, `swr_v1_reactor_delete` |
| `reactor->add`, `set`, `del` | `reactor_upstream::change` with `ChangeOp::Add`, `Set`, `Del` |
| `reactor->wait()` | `reactor_wait` |
| `reactor->set_handler`, `isset_handler`, `get_handler`, `default_write_handler` | `reactor_upstream::set_handler`, `reactor_isset_handler`, `reactor_get_handler`, `reactor_default_write_handler` |
| `reactor->exists(fd)`, `get_event_num()` | `swr_v1_reactor_registry_get`, `swr_v1_reactor_registry_count` |
| `reactor->set_end_callback`, `erase_end_callback` | `swr_v1_reactor_set_end` |
| the `Reactor`'s fields (`max_event_num`, `once`, `ptr`, `future_task`, ...) | `ReactorFields` through `ReactorPrepared::of`, and `Running`'s setters |
| `reactor->drain_write_buffer` | crates/tls's `reactor_write::reactor_drain_write_buffer` |
| `swoole_event_init`, `free`, `wait`, `add`, `add_or_update`, `del`, `write`, `writev`, `defer`, `set_handler`, `get_socket`, `is_available` | `swoole_event_init`, `swoole_event_free`, `event_api::swoole_event_wait`, `event_add`, `add_or_update`, `event_del`, `event_write` (bytes of upstream's over-long length for `write_2m`), `swoole_event_writev`, `swoole_event_defer_value` (`Reactor::defer` of a Rust value), `set_handler`, `swoole_event_get_socket`, `swoole_event_is_available` |
| `new UnixSocket`/`Pipe`, `delete`, `ready`, `get_socket`, `set_blocking`, `set_buffer_size`, `write`, `close` | crates/tls's `socket_pair::unix_socket`, `pipe`, `drop`, both ends set, `raw_ends`, `set_blocking`, `unix_socket_set_buffer_size`, `write_from`, `close` |
| `make_socket(fd, type)`, `socket->free()`, `move_fd()`, `wait_event`, `get_out_buffer_length()` | `socket_upstream::make_socket_fd`, crates/tls's `upstream::socket_free`, `fd = -1`, `Socket::wait_event`, `swr_cpp_buffer_length` of the output buffer |
| `swoole_add_hook`, `swoole_clear_last_error`, `swoole_set_print_backtrace_on_error`, `swoole_signal_set`, `swoole_signal_block_all`, `swoole_random_bytes`, `swoole_timer_add`, `sw_tg_buffer()` | `swoole_add_hook`, `log::set_last_error(0)`, `swoole_set_print_backtrace_on_error`, `signal::swoole_signal_set_action`, `signal_block_all`, `random_bytes`, `swoole_rs_timer::add_fn`, `thread_buffer` |

`network/stream.cpp`'s TESTs are Rust tests of `network::Stream`, which is Rust
(`crates/corebase/src/process/stream.rs`: `cargo test stream::`). A
socket pool's dispatch was its only production user; `src/network/stream.cc` and
the TESTs left the build (#1674).

| Upstream TEST | Rust test | Same input | Assertions |
| --- | --- | --- | --- |
| `recv_sync_rejects_truncated_packet` | `recv_sync_rejects_truncated_packet` | a Unix socket pair: 3 bytes of a 2048 length, then length 8 and `ab`, each then `SHUT_WR`; a 2048-byte buffer | `recv_sync` is `SW_ERR` both times |
| `send` | `send` | 64 KiB of bytes; a regular file as the Unix path, TCP port 39999 with nothing listening, a length-checked TCP listener | the file's stream is null; port 39999's is created, then its response is `nullptr` with `errCode` `ECONNREFUSED`; the server reads the body sent and the response is `"Server: "` + body |

The server in `send` is a plain TCP listener replying as upstream's `Server`
`onReceive` does (`"Server: "` + body, framed, then a zero-length frame). The
Rust test also checks that each response callback runs exactly once.

Deferred-queue Rust regressions also check callbacks appended during destruction
retain FIFO order, and a caught nested escape leaves its unrun tasks to the
reactor while the outer pass completes.
