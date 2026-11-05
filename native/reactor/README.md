# M05 reactor and stream

Opt-in Rust policy for the Swoole reactor backends, callbacks, Socket I/O and
output queues. The frozen ABI is `docs/reactor-abi-v1.md`; the module-local
headers are `swoole_rs_reactor.h` and `swoole_rs_reactor_defer.h`. Rust sources
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

The optional M15 half-close patches apply first when requested and stay
byte-identical; the M05 hunks continue after their prefixes.

## Rust-owned storage

Every `SWOOLE_RS_REACTOR` (`--enable-swoole-rs-reactor`) build uses Rust
storage for IOVector arrays ([socket-io.md](socket-io.md)), the `defer_tasks`
and `destroy_callbacks` FIFOs ([callbacks.md](callbacks.md)). These change the
private Reactor layout, so every selected SDK consumer must rebuild. The socket
map stays upstream's `std::unordered_map`: a Rust registry can only reproduce
its iteration order per C++ standard library, and existed only for Apple libc++.
`SWOOLE_RS_BUFFER_TYPED` (on with Rust Buffer and reactor) makes Server callers
use the typed Rust Buffer queue ([output.md](output.md)).

## Contents

- [backend.md](backend.md): construction, poll/epoll/kqueue storage,
  registration, wait, batch decoding, readiness and signals.
- [callbacks.md](callbacks.md): ordered end/exit callbacks, handlers, builtin
  defer/idle/exit scheduling, deferred queue ownership.
- [socket-io.md](socket-io.md): the Rust-owned IOVector.
- [output.md](output.md): Socket output queue progression, writable/drain,
  `Reactor::write_func`, typed server Buffer consumers.
- [production.md](production.md): loaded-PHP consumer profiles.

## Checks

Build the archive, then run a facade gate against the pinned source:

```sh
cargo build --locked --release -p swoole-rs-reactor --target-dir PRIVATE_TARGET
python3 tests/modules/reactor/builtins.py --source PINNED --archive PRIVATE_TARGET/release/libswoole_rs_reactor.a --sanitize --report UNIQUE.json
```

Other gates under `tests/modules/reactor/` take the same
`--source/--archive/--report [--sanitize]` arguments: `backend_storage.py`,
`poll_snapshot.py`, `wait.py`, `initialize.py`, `callbacks.py`,
`callback_queues.py`, `flush.py` and `forwarding.py`.
The queued-output 53-case matrix lives in the Rust `output` unit tests.

These gates extract the complete original and selected method bodies and run
them against Native effect facades with positive inputs only. Error,
interruption, allocation-failure, reentry and lifetime paths are not executed.
Mixed Rust/Native ASan linking fails on macOS (runtime version symbol mismatch);
it works on Linux.
