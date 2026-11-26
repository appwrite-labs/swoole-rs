# M05 reactor and stream

The Swoole reactor (backends, wait loop, handlers, callbacks, deferred queues),
`Socket` I/O and output queues. Rust sources are in
`crates/corebase/src/reactor/`; `crates/corebase/src/tls/upstream.rs` and `reactor_write.rs` hold the
`Socket` and write-path members that also touch TLS. The reactor reaches that write
path and the reactor create and destroy hooks only through `reactor::UpperLayer`, which
`swoole_init` registers (`register_upper_layer`).

`swoole_socket.h` exposes the socket members native code uses.
`reactor_state.rs` constructs and frees the reactor state
(`swoole_event_init`, `swoole_event_free`). An `IOVector` is a Rust owner
(`iov_owned.rs`) holding a typed copy of the caller's descriptors; payload bytes are not
copied.

The event-loop init, wait, availability and socket-lookup helpers are Rust APIs.
Original Rust fixtures call them directly; no native caller remains, so their
C++ declarations and exports are retired. Typed `isset_handler` also replaces the
unused integer-event wrapper.

## Contents

- [backend.md](backend.md): construction, backends, wait and kqueue signals.
- [callbacks.md](callbacks.md): end callbacks, exit conditions and their built-ins,
  handlers, deferred queues.
- [output.md](output.md): Socket output queue progression and the writable callback.

## Checks

`cargo test -p swoole-rs-corebase reactor::` runs the Rust side; `callbacks.rs`'s tests cover
the end callbacks, exit conditions and built-ins. Upstream's
`reactor/base.cpp` GoogleTests are Rust cases over the hybrid
(`tests/modules/reactor/base`, `reactor-base-rust`).
