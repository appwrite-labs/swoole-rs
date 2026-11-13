# M05 reactor and stream

The Swoole reactor (backends, wait loop, handlers, callbacks, deferred queues),
`Socket` I/O and output queues. The header is `swoole_rs_reactor.h`. Rust sources are in
`crates/reactor/src/`; `crates/tls/src/upstream.rs` and `reactor_write.rs` hold the
`Socket` and write-path members that also touch TLS.

This directory's `swoole_socket.h` and `swoole_reactor.h` replace upstream's headers
under the same names, each keeping only the members the build's C uses. `Reactor` is a methods-only
class over `swr_v1_reactor_head`, which Rust's reactor state begins with, so a
`Reactor *` is that state; `reactor_state.rs` constructs and frees it
(`swoole_event_init`, `swoole_event_free`). An `IOVector` is a Rust owner
(`iov_owned.rs`) holding both `iovec` spans; payload bytes are not copied.

## Contents

- [backend.md](backend.md): construction, backends, wait and kqueue signals.
- [callbacks.md](callbacks.md): end callbacks, exit conditions and their built-ins,
  handlers, deferred queues.
- [output.md](output.md): Socket output queue progression and the writable callback.

## Checks

`cargo test -p swoole-rs-reactor` runs the Rust side; `callbacks.rs`'s tests cover
the end callbacks, exit conditions and built-ins. Upstream's
`reactor/base.cpp` GoogleTests are Rust cases over the hybrid
(`tests/modules/reactor/base`, `reactor-base-rust`).
