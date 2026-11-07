# M05 reactor and stream

Opt-in Rust policy for the Swoole reactor backends, callbacks, Socket I/O and
output queues. The frozen ABI is `docs/reactor-abi-v1.md`; the module-local
header is `swoole_rs_reactor.h`. Rust sources
are in `crates/reactor/src/`.

General rule for every seam: Rust receives copied scalar facts and returns a
decision. Native code then performs the syscall, callback, warning, allocation,
exception or Zend effect. No Rust frame or borrow spans a Native effect, and
Rust never dereferences, drops or invokes a Native Socket/callback token.
Warnings stay in their original Native method so `__SW_FUNC__` is unchanged.

## Patched upstream files

| Patch | Upstream files |
| --- | --- |
| 0001-readiness-policy | `include/swoole_reactor.h`, `src/reactor/{poll,epoll,kqueue}.cc` |
| 0002-write-policy | `src/reactor/base.cc` |
| 0003-stream-framing | `src/network/stream.cc` |
| 0004-iov-owned | `include/swoole_socket.h` |
| 0004-socket-output | `src/network/socket.cc` |
| 0007-server-buffer-typed | `src/server/base.cc`, `src/server/reactor_thread.cc` |

`include/swoole_socket.h` and `include/swoole_reactor.h` are deleted; this
directory's `swoole_socket.h` and `swoole_reactor.h` replace them under the same
names, so their includers are unchanged.

The optional M15 half-close patches apply first when requested and stay
byte-identical; the M05 hunks continue after their prefixes.

## Rust-owned storage

Every Rust build uses Rust
storage for IOVector arrays ([socket-io.md](socket-io.md)), the `defer_tasks`
and `destroy_callbacks` FIFOs ([callbacks.md](callbacks.md)). These change the
private Reactor layout, so every selected SDK consumer must rebuild. The socket
map stays upstream's `std::unordered_map`: a Rust registry can only reproduce
its iteration order per C++ standard library, and existed only for Apple libc++.
Server callers use the typed Rust Buffer queue ([output.md](output.md)).

## Contents

- [backend.md](backend.md): construction, poll/epoll/kqueue storage,
  registration, wait, batch decoding, readiness and signals.
- [callbacks.md](callbacks.md): ordered end/exit callbacks, handlers, builtin
  defer/idle/exit scheduling, deferred queue ownership.
- [socket-io.md](socket-io.md): the Rust-owned IOVector.
- [output.md](output.md): Socket output queue progression, writable/drain,
  `Reactor::write_func`, typed server Buffer consumers.

## Checks

`cargo test -p swoole-rs-reactor` runs the Rust side. The former facade gates
that sliced the constructor, the end/exit callbacks and the idle, try-exit and
deferred builtins out of the former hybrid `src/wrapper/event.cc` are Rust tests named
`former_initialize_gate_*`, `former_callbacks_gate_*` and
`former_builtins_gate_*`, with the same cases and expected values; they drive
the Rust exports through a Rust copy of native/reactor/reactor.cc's glue. The queued-output
53-case matrix lives in the Rust `output` unit tests. Upstream's own
`reactor/base.cpp` GoogleTests run on both engines (`reactor-native-originals`).
