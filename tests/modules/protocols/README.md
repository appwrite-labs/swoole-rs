# Packet protocols and proxy codecs

`crates/protocols` implements MQTT framing, SOCKS5 wire/state/configuration
policy and RESP command/receive/reply policy over borrowed slices, and forbids
`unsafe`. Its callers over the client's records are `crates/clients`' `mqtt`
(`set_protocol` and the length callback), `owned_proxy` (the `Socks5Proxy` entries)
and `resp_recv` (`redis::recv_packet`'s loop). `crates/bus` is MessageBus chunking/fairness/membership.

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
- MessageBus: Rust owns the receive buffer, the packets being
  reassembled and both read loops and the write loop. It keeps direct/chunk flags,
  retry after minimum-size reduction, legacy nonnegative chunk-write success and
  uint16 fairness wrapping (including buffered receive's double increment). End
  wins over the fairness threshold. The read returns to native code to allocate a
  packet (the allocator may be Zend's, which can bail out) and to log; it calls
  the packet allocator's `free`, and the write calls native `send`, neither of
  which bails out. `cargo test -p swoole-rs-bus` round-trips packets over
  real socketpairs, upstream's `message_bus` TESTs among them (see below).
  Packet storage reuses buffer's `OwnedString`; a delivered message takes its packet
  out of the bus (`take_packet`) and moves it into the receive callback's `RecvData`,
  whose `take_packet` is upstream's `move_packet`. A taken packet no longer counts
  toward the bus's retained memory. `moved_packets_are_not_freed_and_memory_counts_reassembly`
  checks the same allocation address across transfer and that the bus cannot free
  the new owner's payload. Allocation and resizing run outside the packet-table borrow.
  The receive allocation is also an `OwnedString`, freed by its own allocator on
  replacement, allocation failure or drop. Server and pool receive buffers use
  libc because their bus can survive PHP request shutdown; transferable packets
  retain their configured allocator. It is zeroed once at allocation, so
  reads borrow initialized bytes without per-read initialization bookkeeping.
  Reads decode header bytes into typed state once; subsequent flag/pass writes
  keep both forms in sync;
  passed payload pointers stay in a separate typed field. Whole-event callbacks
  keep the owner outside the bus, so a callback can replace or free the buffer
  without invalidating the bytes it was lent. Ownership tests cover short
  overwrites, callback reentry, replacement and allocation failure.

Rust's own bounds: empty MQTT asks for more, invalid/truncated RESP fails, and
oversized SOCKS output is rejected. Numeric error cases compare the `get_number`
helper. Invalid non-array PHP SET/MAP is excluded.

## Rust and native differential

```sh
cargo test -p swoole-rs-protocols -p swoole-rs-clients -p swoole-rs-bus
cargo clippy -p swoole-rs-protocols -p swoole-rs-clients -p swoole-rs-bus --all-targets -- -D warnings
```

`native.rs` (the `native` test of `swoole-rs-clients`) compares the Rust with
the pinned bodies `crates/oracle` compiles unchanged. The C++ gate it replaces
(`native.py`, `native.cc`, `adapter.inc`) compiled the same pinned MQTT and SOCKS5
bodies beside the hybrid `Socks5Proxy` header; that header is gone, and its parts are
one Rust test each, with the same inputs and expected values:

| `native.cc` part | Rust test | Pinned side |
| --- | --- | --- |
| MQTT remaining lengths at the varint boundaries, each prefix (30), and every two-byte head before a continued third (65,536) | `mqtt_lengths` | the pinned `mqtt::get_package_length` |
| `handshake` over six states, three methods, every reply version, five reply codes and three send outcomes (69,120) | `socks5_handshakes` | `crates/oracle`'s `socks5::handshake` |
| The three `pack_*_request` at target host and credential lengths around the 250- and 255-byte limits, and proxy and target hosts around 480 bytes over a filled packet (22) | `socks5_packs` | `socks5::pack` |
| `Socks5Proxy::strerror` from -32 to 288 and both extremes (323) | `socks5_strerror_codes` | `socks5::strerror` |
| `create` over seven user and password lengths around 250 bytes (with an embedded NUL), three proxy host lengths and three socket types (441) | `socks5_create` | `socks5::create` |

The gate's RESP parts had Rust subjects that nothing but the gate called. They are
`native.rs` (the `native` test of `swoole-rs-clients`), one Rust test per part,
each C export replaced by the safe function that was its body:

| `native.cc` part | Rust test | Export → function |
| --- | --- | --- |
| `receive_states`: every split of eight commands at three package limits against the pinned `redis::recv_packet` | `receive_states` | `swr_v1_resp_recv` over C effects → `recv_packet` over a `RespConnection` |
| `handler_key_trace`: 2432 keys against v6.3’s full-length ASCII case model | `handler_keys` | `swr_v1_resp_handler_key` → `resp_handler_key` |
| Five values and five integers as each of the seven reply types against the pinned `redis::format` overloads (70) | `reply_formats` | `swr_v1_resp_format` → `resp_format` |
| Integers, numbers at and past a signed long from errno 0, `EDOM` and `ERANGE`, 1 to 128 arguments, permissive fallthrough and bulk payloads against the pinned `redis::parse`, or `get_number` where parse reads outside its input (134) | `parse_replies` | `swr_v1_resp_scan`, `swr_v1_resp_parse_range` → `resp_spans` |
| `seeded_resp_trace`: 10,000 seeded wires through the pinned `redis::parse` and the pinned PHP request loop with modeled Zend parameters, strings, parameters and errno compared | `seeded_requests` | `swr_v1_resp_scan`, `swr_v1_resp_php_next` → `resp_spans`, `resp_php_next` |
| The bounded failures that leave their output alone | `bounded_failures_leave_output` | `swr_v1_mqtt_frame_length`, `swr_v1_socks_auth`, `swr_v1_socks_connect`, `swr_v1_resp_format` → `mqtt_frame_length`, `socks_auth`, `socks_connect`, `resp_format` |

The pinned sides are `crates/oracle`'s `protocols`: `redis::get_number`, `parse` and
`format` from the pinned redis.cc and the request loop from the pinned
`ext-src/swoole_redis_server.cc`, copied unchanged by its build script. `redis::parse`
over Rust's spans is `redis_reply.inc`'s, which the gate compiled: an integer span in
decimal, any other span's bytes, nothing for a malformed wire, and `ERANGE` set by the
caller. `seeded_requests` also asserts the gate's `resp_corpus_fnv1a64`
(`c51ae5cf4cf33d44`), so the wires are the ones `native.cc` generated.

Five of the gate's assertions checked the exports' own argument validation, which
went with the exports: a null pointer with a nonzero length (`swr_v1_mqtt_frame_length`,
`swr_v1_socks_auth`, `swr_v1_resp_format`), `swr_v1_resp_parse_range`'s -2 for a
non-null output of capacity 0, and `swr_v1_resp_receive_range`'s -1 for a length past
its capacity. Slices cannot carry the first four, and the receive loop, the codec's
one caller, never passes more bytes than its buffer holds.
`Socks5Proxy::handshake_step`, `admit` and `pack_negotiate_request` lost their exports
once crates/clients reached them through a registered host (#4371), since no native
code calls them; the null-reply assertions went with `handshake_step`'s pointer.

The pinned side of `receive_states` is `crates/oracle`'s `resp_stream::UPSTREAM_TRACE`:
the unchanged `recv_packet` over the oracle's socket model, which logs each call's
result, errno (at every package and on return), packages, buffer length and offset,
and request state; the Rust loop runs over the same model and must log the same
bytes. It makes the further calls the oracle's driver makes until the stream is
drained, where the C++ gate made one. `handler_key_trace` also checked a guard byte
either side of the export's 64-byte output, which went with the export:
`resp_handler_key` returns owned bytes.

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

Upstream PHPTs for this module run in the merge tier's PHPT gate (the `phpt-*` jobs), which runs every pinned PHPT on upstream and the Cargo build. To attribute a gate regression to this module, see *Attributing a regression* in [corpus/upstream/README.md](../../../corpus/upstream/README.md).

## Original core GTests

Upstream's `core-tests/src/protocol/redis.cpp`, `server/message_bus.cpp` and
`server/mqtt.cpp` TESTs are Rust, one test per TEST with the same inputs and
expected values:

| TEST | Rust test |
| --- | --- |
| `redis.get` | `redis.get` in `server.rs` |
| `redis.server` | `redis.server` in `server.rs` |
| `redis.format` | `format_empty_status_and_error` in `native.rs` |
| `redis.parse` | `parse_integer_reply` in `native.rs` |
| `redis.recv_packet_with_partially_filled_buffer` | `recv_packet_with_partially_filled_buffer` in `crates/ports/src/resp_recv.rs` |
| `message_bus.read` | `message_bus_read` in `crates/bus/src/lib.rs` |
| `message_bus.read_with_buffer` | `message_bus_read_with_buffer` in `crates/bus/src/lib.rs` (Linux) |
| `mqtt.echo` | `mqtt.echo` in `server.rs` |

`server.rs` is built into the http server case binary (`tests/modules/http/server`,
the merge tier's `http-server-rust` phase), whose `src/http1/case.rs` builds the
redis and mqtt Servers. With `--core-test-services`, the merge tier runs
`redis.get` against the PHPT Redis service on 127.0.0.1:6379 and `mqtt.echo`
with the pinned tree's `core-tests/js/mqtt.js` and its npm dependencies. Other
runs report those service-dependent cases as excluded.

MQTT length parsing returns `Result<Option<usize>, RemainingLengthTooLong>`:
a missing length header is incomplete, and a fourth continuation bit is malformed.
`crates/clients`' MQTT length callback retains the warning and 0/-1/count convention. Original
unit/oracle/fuzz expectations convert only at their test boundary; packet-type
bytes and nonminimal remaining lengths remain accepted.

`fuzz_request.rs` checks the independent input-domain model for the `resp_server`
differential and exercises the pinned PHP request loop on bounded numeric, binary,
null, prefix-fallthrough and retained-length inputs. Run with
`cargo test --manifest-path fuzz/Cargo.toml --lib fuzz_request_tests`; the fuzz
target retains the span/progress/command invariants on inputs outside that domain.
