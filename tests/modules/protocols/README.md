# Packet protocols and proxy codecs

`crates/protocols` implements MQTT framing, SOCKS5 wire/state/configuration
policy and RESP command/receive/reply policy over borrowed slices, and forbids
`unsafe`. `crates/protocols-ffi` is the C contract over it
(`native/protocols/swoole_rs_protocols.h`) and the upstream-symbol `mqtt` and
`Socks5Proxy` entries. `crates/bus` is MessageBus chunking/fairness/membership.

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
  which bails out. `cargo test -p swoole-rs-bus` round-trips packets over
  real socketpairs, upstream's `message_bus` TESTs among them (see below).

Undefined legacy overreads/overflows are not replayed: empty MQTT asks for more,
invalid/truncated RESP fails, oversized SOCKS output is rejected before overflow.
Original malformed RESP parsing dereferences nullptr under UBSan; numeric error
cases compare the defined `get_number` helper instead. Invalid non-array PHP
SET/MAP is excluded because the original throws then dereferences invalid memory.

## Rust and native differential

```sh
cargo test -p swoole-rs-protocols -p swoole-rs-protocols-ffi -p swoole-rs-bus
cargo clippy -p swoole-rs-protocols -p swoole-rs-protocols-ffi -p swoole-rs-bus --all-targets -- -D warnings
python3 tests/modules/protocols/native.py --source /pinned/reference \
  --archive /actual/prefix/lib/libswoole_rs_ffi.a --sanitize
```

`native.py` compiles unchanged pinned function bodies (`adapter.inc`, `native.cc`)
with modeled String allocation/socket I/O and compares their MQTT, SOCKS, RESP and
receive traces with the build's Rust: the upstream-symbol exports, the hybrid
`Socks5Proxy` header over them, and `swr_v1_resp_recv` over the modeled socket.
The build has no `redis::format`/`parse`; `redis_reply.inc` gives them over Rust's
codec to this gate. A seeded 10,000-wire RESP corpus also runs the exact original PHP
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

`bailout.py` has no request-side out-of-memory scenario. After PHP logs
allocator exhaustion in a worker, upstream stays alive and unresponsive (base
process waits in `epoll_pwait`) instead of exiting 255 or restarting the worker,
so the outcome isn't decided by the engine.

## Original PHPTs

Upstream PHPTs for this module run in the merge tier's PHPT gate (the `phpt-*` jobs), which runs every pinned PHPT on upstream and the all-modules hybrid. To attribute a gate regression to this module, see *Attributing a regression* in [corpus/upstream/README.md](../../../corpus/upstream/README.md).

## Original core GTests

Upstream's `core-tests/src/protocol/redis.cpp`, `server/message_bus.cpp` and
`server/mqtt.cpp` TESTs are Rust, one test per TEST with the same inputs and
expected values:

| TEST | Rust test |
| --- | --- |
| `redis.get` | `redis.get` in `server.rs` |
| `redis.server` | `redis.server` in `server.rs` |
| `redis.format` | `format_empty_status_and_error` in `crates/protocols-ffi/src/redis.rs` |
| `redis.parse` | `parse_integer_reply` in `crates/protocols-ffi/src/redis.rs` |
| `redis.recv_packet_with_partially_filled_buffer` | `recv_packet_with_partially_filled_buffer` in `crates/ports/src/resp_recv.rs` |
| `message_bus.read` | `message_bus_read` in `crates/bus/src/lib.rs` |
| `message_bus.read_with_buffer` | `message_bus_read_with_buffer` in `crates/bus/src/lib.rs` (Linux) |
| `mqtt.echo` | `mqtt.echo` in `server.rs` |

`server.rs` is built into the http server case binary (`tests/modules/http/server`,
the merge tier's `http-server-rust` phase), whose `src/http1/case.rs` builds the
redis and mqtt Servers. That phase excludes `redis.get`, which needs a Redis server on
127.0.0.1:6379, and `mqtt.echo`, which needs Node with the mqtt and pino packages
(`core-tests/js/mqtt.js`).
