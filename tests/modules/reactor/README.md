# Reactor tests

`reactor-unit` runs `crates/reactor`'s tests. `reactor-native-originals`
(`tools/core_tests.py`) runs upstream's `reactor/base.cpp` and
`network/socket.cpp` GoogleTests on both engines, with the exclusions listed in
`CORE_TESTS` in `tools/dev.py`. `reactor-compile` runs
`compile_gate.py --source S --archive A --report R`: the original and hybrid
backend translation units compile as C++14.

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
