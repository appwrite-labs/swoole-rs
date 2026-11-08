# M05 reactor and stream

The Swoole reactor (backends, wait loop, handlers, callbacks, deferred queues),
`Socket` I/O and output queues. The header is `swoole_rs_reactor.h`. Rust sources are in
`crates/reactor/src/`; `crates/tls/src/upstream.rs` and `reactor_write.rs` hold the
`Socket` and write-path members that also touch TLS.

This directory's `swoole_socket.h` and `swoole_reactor.h` replace upstream's headers
under the same names. `swoole_rs_socket_methods.h` keeps the `Socket` members that
need C++ (exceptions, `std::string`, `std::vector`). `reactor.cc` keeps what stable
Rust can't express: the `Reactor` object's lifetime and the `std::function`
callbacks, tasks and deferrals it holds. An `IOVector` is a view of a Rust owner
(`iov_owned.rs`) holding both `iovec` spans; payload bytes are not copied.

## Contents

- [backend.md](backend.md): construction, backends, wait and kqueue signals.
- [callbacks.md](callbacks.md): ordered end/exit callbacks, handlers, builtin
  defer/idle/exit scheduling, deferred queue ownership.
- [output.md](output.md): Socket output queue progression and the writable callback.

## Checks

`cargo test -p swoole-rs-reactor` runs the Rust side. The former facade gates
over the constructor, the end/exit callbacks and the idle, try-exit and deferred
builtins are Rust tests named `former_initialize_gate_*`,
`former_callbacks_gate_*` and `former_builtins_gate_*`; they drive the Rust
exports through a Rust copy of `reactor.cc`'s glue. Upstream's
`reactor/base.cpp` GoogleTests are Rust cases over the hybrid
(`tests/modules/reactor/base`, `reactor-base-rust`).
