# WebSocket tests

The frame codec is `crates/websocket`, which forbids `unsafe`. `crates/ports`'
`frame_api`, its only caller with upstream's records, is `websocket.cc`'s entry points
over `swoole::String` and `websocket::Frame`, as safe functions over slices;
`websocket::get_package_length` is shared by clients and ports in `swoole_rs_clients::ws::websocket_length`. They had C++
symbols, which only Rust linked; Rust calls them by path now. The safe decoder returns `Result<DecodedFrame,
DecodeError>`; its error preserves the partial header and distinguishes incomplete
input, length overflow, and compressed control frames. `DecodedFrame`, `Context`
and `Decision` are plain Rust records. Decoded opcodes use `Opcode`, including
`Reserved(u8)` so unknown wire codes retain their original policy and diagnostics;
PHP and native frame adapters convert them back to bytes. Decoded flags use
`FrameFlags` with named final/masked/compressed queries, and `FrameMetadata` keeps
fragment opcode and flags together without packing them. Only native/test
boundaries pack dispatch metadata; outgoing encoder arguments still use raw bits; the native `websocket::Frame` layout stays
in `frame_api`. Frame measurement returns `Result<FrameLength, LengthError>`
with `usize` sizes; incomplete and overflowing headers retain their incremental
header length. Only the native framing adapters convert those outcomes to zero
or -1. The kernel, codec and fuzz tests adapt typed outcomes to the former integer
protocol so their original assertions remain unchanged. Encoded sizes and frame
writers return `Result<usize, EncodeError>`; success reports initialized bytes,
while overflow, invalid control frames, mismatched payload lengths and short
output leave the output untouched. `encoding_errors_leave_output_untouched`
checks those errors directly, including declared/supplied payload mismatches. `mask`'s null-pointer and oversized-length
refusals went with its export, and their test (`tests/mask.rs`'s
`invalid_mask_pointers_and_lengths_do_not_write`) with them: a slice and a key by value
cannot carry those arguments. The PHP frame classes and client state live in `crates/phpclients/src/websocket.rs`. The server dispatches frames in `crates/phphttp/src/websocket.rs`.

## Codec

`cargo test -p swoole-rs-websocket` (`websocket-unit`) covers
the codec at every opcode/flags combination and length boundary (`tests/kernel.rs`), and
the mask kernel (`tests/mask.rs`). The `fuzz/` targets `ws_frame_length`, `ws_decode` and
`ws_mask` compare the codec with the pinned C++ bodies.

`codec.rs` (the `codec` test of `swoole-rs-websocket`) compares the codec with the
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
  barrier, then post-close health. Its `test_frame_during_handshake` is 6.3's ordering
  (7bd6134c4): a frame sent while a custom `onHandshake` yields never reaches
  `onMessage` before the 101, in base and process mode; the v6.2.6 control skips it.

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

Encoder/header and pack/delivery policy arguments use `Opcode` and `FrameFlags`.
PHP/native callers and the original codec/kernel test adapters convert raw bytes
at their boundaries; reserved values and every original expected byte remain.
