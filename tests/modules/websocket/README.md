# WebSocket tests

The frame codec is `crates/websocket`, which forbids `unsafe`. Its C ABI is
`crates/websocket-ffi`: the mask export in `docs/abi-v1.md` and
`native/include/swoole_rs_websocket.h`, the codec/policy exports in
`native/websocket/swoole_rs_websocket_codec.h`, and `websocket.cc`'s C++-symbol
entry points (`frame_api`). The PHP side is `crates/phphttp/src/websocket.rs`.

## Codec

`cargo test -p swoole-rs-websocket -p swoole-rs-websocket-ffi` covers the codec
at every opcode/flags combination and length boundary, and the C exports'
pointer contracts. The `fuzz/` targets `ws_frame_length`, `ws_decode` and
`ws_mask` compare the codec with the pinned C++ bodies.

`test_codec.py` (`websocket-codec`) compiles the pinned codec function bodies and
PHP policy snippets from `target/upstream/src` and compares them with the Rust
exports exhaustively over opcode, flag and length inputs:

```sh
cargo build --release -p swoole-rs-ffi
python3 -m pytest tests/modules/websocket/test_codec.py \
  --archive target/release/libswoole_rs_ffi.a --sanitize
```

Its C++ sanitizer run excludes alignment: the original codec does unaligned
integer loads. For Rust ASan (Linux; macOS's runtime cannot link it), build the
archive with `RUSTC_BOOTSTRAP=1 RUSTFLAGS='-Zsanitizer=address
-Zexternal-clangrt=yes' cargo rustc -p swoole-rs-websocket-ffi --release --crate-type
staticlib` and add `--rust-asan`.

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
