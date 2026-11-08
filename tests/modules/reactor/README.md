# Reactor gates

`python3 tools/dev.py` registers most of these as phases when the `reactor`
module is selected. Scripts can also be run directly from the repo root.

## Former facades

The `callbacks`, `initialize` and `builtins` gates sliced function bodies out of
the former hybrid `src/wrapper/event.cc`. Their cases are `crates/reactor` tests named
`former_callbacks_gate_*`, `former_initialize_gate_*` and
`former_builtins_gate_*` (`callbacks.rs`, `initialize.rs`, `builtins.rs`).
The 53 ordinary queued-output compositions formerly in `output.cc` now call the
shipped Rust exports in `crates/reactor/src/output.rs`'s
`ordinary_data_positive_matrix` and `ordinary_file_positive_matrix` tests. `reactor-unit` runs them; the redundant
`reactor-output` phase is removed. The wait frame formerly in `wait.cc` is
`wait_loop.rs`'s `whole_wait_runs_upstreams_frame_on_each_backend`. The backends' add, set,
del and constructors are `swr_v1_reactor_change` and `swr_v1_reactor_state_open`; `change.rs` and
`reactor_state.rs` test them over real pipes, so the `backend_storage` and `poll_snapshot` facades are gone. `Socket::sendfile` and `send_async` are Rust (`crates/tls/src/upstream.rs`), whose tests
cover them, so the `forwarding.py` facade is gone.

`reactor-native-originals` (`tools/core_tests.py`) runs upstream's own
`network/socket.cpp` GoogleTests on both engines. `socket.connect_sync` and
`socket.fail` are excluded: each connects to `TEST_HTTP_DOMAIN:80` within 0.3s, which
the CI runner's external network doesn't reliably allow.

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

Compile gate:

- `compile_gate.py --source S --archive A --report R`: original and hybrid backend translation units compile as C++14.


## Unit tests

    cd tests/modules/reactor && python3 -m unittest test_*.py

`test_sockets_source.py` reads historical
commits with `git show` and fails in checkouts that lack them.
