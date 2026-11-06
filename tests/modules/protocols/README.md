# Packet protocols and proxy codecs

`crates/protocols` implements MQTT framing, SOCKS5 wire/state/configuration
policy, RESP command/receive/reply policy and MessageBus chunking/fairness/
membership. The feature is opt-in (bit 128); the C contract is
`native/protocols/swoole_rs_protocols.h`.

## Boundaries and compatibility

Rust owns packet membership, protocol decisions and bounded encoding/decoding.
Native code owns Socket/Address execution, String allocation, shared-memory/header
layouts, logging and Zend execution. No native allocation, PHP conversion or
callback that can bail out runs beneath a Rust frame.

- MQTT keeps incremental/nonminimal remaining lengths and four-byte overflow
  termination; packet type is not newly validated.
- SOCKS5 keeps state-before-send behavior, unknown-state success, byte-truncated
  domain lengths and the deferred 480-byte proxy host check. Factory credentials
  reject either field above 250 bytes and ignore both when either is missing.
  Native address-validation failures keep the three-byte CONNECT prefix.
- RESP keeps C ASCII whitespace, signed-long saturation/int32 truncation,
  null-bulk omission, permissive prefix fallthrough, full-buffer dispatch and the
  original receive counters. Native callers set errno=ERANGE after Rust returns.
  Public unbounded `get_number` remains a libc adapter.
- PHP request decoding shares the bounded Rust RESP cursor. Zend arrays keep
  nulls and integer ordering, even before the first string command.
- Handler keys use one Rust normalizer for registration, property lookup and
  dispatch: `_handler_` prefix, 63-byte truncation, embedded-NUL stop, ASCII
  lowercase.
- PHP reply encoding: PHP NIL=1 differs from native codec NIL=7. Present-empty
  status/error differs from missing; PHP empty bulk emits `$0` while the primitive
  formatter writes nothing. Embedded-NUL status/key truncation, unsigned numeric
  map keys and 32-bit map-count wrapping remain.
- MessageBus: Rust owns the receive buffer's bookkeeping, the packets being
  reassembled and both read loops and the write loop. It keeps direct/chunk flags,
  retry after minimum-size reduction, legacy nonnegative chunk-write success and
  uint16 fairness wrapping (including buffered receive's double increment). End
  wins over the fairness threshold. The read returns to native code to allocate a
  packet (the allocator may be Zend's, which can bail out) and to log; it calls
  the packet allocator's `free`, and the write calls native `send`, neither of
  which bails out. `cargo test -p swoole-rs-protocols` round-trips packets over
  real socketpairs; upstream's `message_bus` GoogleTests run through `core.py`.

Undefined legacy overreads/overflows are not replayed: empty MQTT asks for more,
invalid/truncated RESP fails, oversized SOCKS output is rejected before overflow.
Original malformed RESP parsing dereferences nullptr under UBSan; numeric error
cases compare the defined `get_number` helper instead. Invalid non-array PHP
SET/MAP is excluded because the original throws then dereferences invalid memory.

## Rust and native differential

```sh
cargo test --manifest-path crates/protocols/Cargo.toml --target-dir target/protocols
cargo clippy --manifest-path crates/protocols/Cargo.toml --all-targets -- -D warnings
python3 tests/modules/protocols/native.py --source /pinned/reference \
  --archive /actual/prefix/lib/libswoole_rs_ffi.a --sanitize
```

`native.py` compiles unchanged pinned function bodies (`adapter.inc`, `native.cc`)
with modeled String allocation/socket I/O and compares their MQTT, SOCKS, RESP and
receive traces with the build's Rust: the upstream-symbol exports, the hybrid
`Socks5Proxy` header over them, and `swr_v1_resp_recv` over the modeled socket.
The build has no `redis::format`/`parse`; `redis_reply.inc` gives them over Rust's
codec to this gate and to `core.py`, whose `redis.*` GTests still call them. A seeded 10,000-wire RESP corpus also runs the exact original PHP
request-parser loop with modeled Zend parameters against Rust typed events. C++
ASan/UBSan does not instrument Rust. Failing cases print trial/seed.

## Actual PHP checks

```sh
for gate in network reply request handlers bailout lifecycle; do
  python3 tests/modules/protocols/$gate.py \
    --engine upstream=/reference/swoole.so --engine hybrid=/candidate/swoole.so
done
```

- `network.py`: MQTT/RESP base/process framing and worker health, packets over
  2MiB through native 64KiB IPC, and real sync/coroutine SOCKS5 clients against a
  local scripted proxy (`server.php`, `socks.php`). No remote service.
- `request.py`: typed callbacks across base/process and coroutines on/off; nulls
  and integers before the command, overflow/truncation, empty/binary/2MiB values,
  97 parameters, mixed-case/NUL commands, 63-byte routing.
- `handlers.py`: handler lookups, property replacement/collision and diagnostic
  order over all leading/trailing byte values; invalid registrations must hit
  the original fatal exit 255.
- `bailout.py`: callback throws and exits, compared across engines.
- `reply.py`: wire bytes, warnings, exceptions and callback order for replies,
  plus `reply_oom.php`, which must hit fatal allocation exit 255 during native
  reply-buffer growth.
- `lifecycle.py`: MQTT/RESP in base/process modes, eight concurrent 2MiB requests,
  exact response hashes, graceful master exit 0, no worker-error callbacks.

`bailout.py` has no request-side out-of-memory scenario (#506). After PHP logs
allocator exhaustion in a worker, upstream stays alive and unresponsive (base
process waits in `epoll_pwait`) instead of exiting 255 or restarting the worker,
so the outcome isn't decided by the engine.

## Original PHPTs

Upstream PHPTs for this module run in the merge tier's PHPT gate (the `phpt-*` jobs), which runs every pinned PHPT on upstream and the all-modules hybrid. To attribute a gate regression to this module, see *Attributing a regression* in [corpus/upstream/README.md](../../../corpus/upstream/README.md).

## Original core GTests (Linux)

Runs the eight unchanged `redis.*`, `message_bus.*` and `mqtt.echo` GTests (dev.py phase
`protocols-native-originals`, Linux) against
upstream and the archive's hybrid tree (every module's patches, `overlay_hybrid_tree`). Build `core.Dockerfile` on the repo dev image
(`dev.py docker` builds it the first time), under one fixed tag so a rebuild replaces it:

```sh
docker build -f tests/modules/protocols/core.Dockerfile -t swoole-protocols-core \
  --build-arg BASE_IMAGE=$(python3 -c 'import tools.dev; print(tools.dev.docker_image())') .
```

`mqtt.echo` needs Node with mqtt 4.3.8 and pino 6.14.0. Mount the repo at /repo
and pinned source at /ref read-only, private targets at /work, and a configured
Linux config.h at /configured.h:

```sh
python3 /repo/tests/modules/protocols/core.py --source /ref \
  --config /configured.h --archive /work/target/hybrid/<build>/rust-prefix/lib/libswoole_rs_ffi.a \
  --build-root /work --node-modules /work/node-fixture/node_modules \
  --require-all --output /work/reports/protocols-core.json
```

It needs an isolated network (Redis 6379, MQTT 9501) and refuses an existing
Redis service. Without Node, `mqtt.echo` is unsupported and `--require-all` exits 1.
