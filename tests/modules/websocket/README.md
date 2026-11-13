# WebSocket tests

The frame codec is `crates/websocket`, which forbids `unsafe`. `crates/websocket-ffi`
(`frame_api`) is `websocket.cc`'s entry points over `swoole::String` and
`websocket::Frame`: safe functions over slices, and `get_package_length`, the one a
protocol table holds, also under its C++ symbol. `encode`, `decode`, `mask`,
`pack_close_frame` and `print_frame` had C++ symbols too, which only Rust tests linked;
those call the functions by path now. `mask`'s null-pointer and oversized-length
refusals went with its export, and their test (`tests/kernel.rs`'s
`invalid_mask_pointers_and_lengths_do_not_write`) with them: a slice and a key by value
cannot carry those arguments. The PHP side is `crates/phphttp/src/websocket.rs`.

## Codec

`cargo test -p swoole-rs-websocket -p swoole-rs-websocket-ffi` (`websocket-unit`) covers
the codec at every opcode/flags combination and length boundary, and the mask kernel
(`tests/kernel.rs`). The `fuzz/` targets `ws_frame_length`, `ws_decode` and
`ws_mask` compare the codec with the pinned C++ bodies.

`codec.rs` (the `codec` test of `swoole-rs-websocket-ffi`) compares the codec with the
pinned codec function bodies and PHP policy snippets, which `crates/oracle` compiles
unchanged, exhaustively over opcode, flag and length inputs. It is the former C++
gate `codec.cc`, one Rust test per block of checks, each C export replaced by the
safe function that was its body:

| `codec.cc` checks | Rust test | Export → function |
| --- | --- | --- |
| The three `static_assert`s on struct sizes | `layouts` | `swr_ws_decoded`, `swr_ws_context`, `swr_ws_decision` → `DecodedFrame`, `Context`, `Decision` |
| `php_policy` loop against the pinned pack and message policy | `php_policy` | `swr_v1_ws_pack_policy`, `swr_v1_ws_message_policy` → `pack_policy`, `message_policy` |
| `encoded` and `decoded` loops against pinned `encode` and `decode` | `encode_and_decode_prefixes` | `swr_v1_ws_encoded_size`, `swr_v1_ws_encode`, `swr_v1_ws_control_invalid`, `swr_v1_ws_decode` → `encoded_size`, `encode_parts`, `control_invalid`, `decode_in_place` |
| `closes` loop against pinned `pack_close_frame` | `close_frames` | `swr_v1_ws_encode_close` → `encode_close_into` |
| Every first byte, masked and unmasked, against pinned `decode` | `decoder_reads_every_first_byte` | `swr_v1_ws_decode` → `decode_in_place` |
| `check(...)` policy table | `decision_policy` | `swr_v1_ws_decide` → `decide_frame` |
| Output capacities 0 to 6 leave the output untouched | `short_output_is_left_untouched` | `swr_v1_ws_encode` → `encode_parts` |
| `encoded_size(SIZE_MAX)`, empty decode, a frame outside its wire | `rejected_arguments` | `encoded_size`, `decode_in_place`, `decide_frame` |
| Non-minimal extended ping | `extended_ping_dispatches_from_offset_two` | `swr_v1_ws_decide` → `decide_frame` |

Ten of the gate's assertions passed null pointers or a `SIZE_MAX` length or capacity
to the exports. They checked the exports' own argument validation, which went with
the exports: slices and references cannot carry those arguments.

## PHP behaviour

```sh
python3 -m pytest tests/modules/websocket/test_differential.py \
  tests/modules/websocket/test_invalid.py tests/modules/websocket/test_close.py \
  --engine upstream=target/upstream/src/modules/swoole.so \
  --engine cargo=target/ext/swoole.so
```

- `test_differential.py` (`websocket-differential`): PHP `Frame` pack/unpack
  outcomes, compared with the first engine.
- `test_invalid.py` (`websocket-invalid`): malformed and boundary frames on fresh
  sockets; rejected connections must terminate and HTTP must stay healthy.
- `test_close.py` (`websocket-close`): close and text frames behind a response
  barrier, then post-close health.

The original `tests/swoole_websocket_server` PHPTs run in the PHPT gate.
`send_close_frame.phpt` is racy on upstream too and `corpus/upstream/run.py`
records it unsupported (`RACY`).

`autobahn.py` runs the Autobahn fuzzing client in Docker against each engine
(`--full` or `--cases GLOB` widen the default profile). It tests RFC
conformance, separate from Swoole compatibility, and fails a case that does
worse than on the first engine (`test_autobahn_verdict.py`).

## Benchmark

`benchmark.py` (`websocket-benchmark`) times PHP `Frame::pack` + `Frame::unpack`
round trips; run it on an idle host. `--kind binary|object|close|compressed`
picks the workload and `--check` verifies the workloads without timing.
