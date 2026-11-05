# WebSocket codec and policy checks

The Rust WebSocket module replaces frame codec and pure dispatch/PHP policy.
Native execution, fragment storage, handshake, zlib and PHP objects remain
upstream. The kernel ABI is in `docs/abi-v1.md` and
`native/include/swoole_rs_websocket.h`; the codec/policy seam is described in
[codec.md](codec.md).

## Native kernel

```sh
cargo test -p swoole-rs-websocket
cargo build --release -p swoole-rs-ffi --features websocket
python3 -m pytest tests/modules/websocket/test_kernel.py --sanitize --archive target/release/libswoole_rs_ffi.a
```

`test_kernel.py` calls the C exports and checks every opcode/flags combination at
the 7/16/64-bit length boundaries, progressive incomplete masked headers,
unaligned input/output, signed total-length overflow, unchanged short outputs,
masking involution, and every mask-key placement across eight payload alignments
and lengths 0-33, with bytes outside the payload unchanged. `--sanitize`
instruments the C++ caller only.

Pass `--archive /absolute/libswoole_rs_ffi.a` for an archive copied by a hybrid
build. The default `target/release/libswoole_rs_ffi.a` is overwritten by builds
with other feature flags; rebuild it with `--features websocket` first. A
bridge-only archive has no kernel exports and must fail to link.

`test_prefix.py` compares the pinned C++ `get_package_length` with Rust for all valid
header prefixes. It reads a hybrid `--manifest` and refuses an archive or
ABI header whose checksum differs from that manifest:

```sh
python3 -m pytest tests/modules/websocket/test_prefix.py --sanitize \
  --manifest target/hybrid/rust/build-manifest.json
```

`test_codec.py` is the differential for the codec/policy seam; see [codec.md](codec.md).

## PHP behaviour

```sh
python3 -m pytest tests/modules/websocket/test_differential.py \
  --engine upstream=target/upstream/src/modules/swoole.so \
  --engine hybrid=target/hybrid/rust/modules/swoole.so
```

`test_differential.py` compares 778 PHP frame outcomes with the first engine: 58 valid
FIN/mask/opcode/length cases including close-frame fields, 576 compression,
continuation, control-normalization and PHP object lifetime cases (finish
property precedence, source destruction/GC, cloning, input mutation during
unpack), and 144 pack-only control-RSV1 cases that preserve upstream behaviour
without asserting a roundtrip. It compares packed bytes and decoded values.

```sh
python3 -m pytest tests/modules/websocket/test_invalid.py tests/modules/websocket/test_close.py \
  --engine hybrid=/absolute/hybrid/swoole.so
```

`test_invalid.py` sends malformed and boundary frames on fresh sockets against pinned
policy: rejected connections must terminate (timeout fails), accepted controls
need a text reply, and HTTP health is checked after every case. `test_close.py`
sends close and text frames with a response barrier and checks post-close health.

## Original PHPTs and conformance

All 36 pinned `tests/swoole_websocket_server` originals are in the `websocket` and
`websocket-network` groups of `corpus/upstream/manifest.json`:

```sh
python3 corpus/upstream/run.py --module websocket --module websocket-network --require-all \
  --engine upstream=/absolute/reference.so --engine hybrid=/absolute/hybrid.so
```

`autobahn.py` runs the Autobahn fuzzing client in Docker against each engine
(bounded profile by default, `--full` or `--cases GLOB` to change it). It tests
RFC conformance, separate from pinned Swoole compatibility.

## Benchmark

Run only with no concurrent builds or tests:

```sh
python3 tests/modules/websocket/benchmark.py \
  --engine upstream=target/upstream/src/modules/swoole.so \
  --engine hybrid=target/hybrid/rust/modules/swoole.so \
  --iterations 100000 --repeat 5
```

It measures PHP `Frame::pack` + `Frame::unpack` round trips per second and
verifies each one. `--kind binary|object|close|compressed` selects the workload:
object mode uses a false finish property to override FIN, close mode packs
CloseFrame code 1000 with COMPRESS and false finish, compressed mode does real
zlib round trips. Close reasons must be at most 125 bytes. `--check` verifies
the workloads without timing:

```sh
python3 tests/modules/websocket/benchmark.py --check --iterations 3 \
  --kind close --sizes 1 123 125 --engine upstream=/absolute/reference.so \
  --engine hybrid=/absolute/hybrid.so
```

## Rust ASan (Linux)

Instrument the Rust module itself and rerun the native gates:

```sh
RUSTC_BOOTSTRAP=1 RUSTFLAGS='-Zsanitizer=address -Zexternal-clangrt=yes' \
  cargo rustc -p swoole-rs-websocket --release --offline --locked \
  --target-dir target/websocket-rust-asan --crate-type staticlib -v
python3 -m pytest tests/modules/websocket/test_kernel.py --sanitize \
  --archive target/websocket-rust-asan/release/libswoole_rs_websocket.a
python3 -m pytest tests/modules/websocket/test_codec.py --sanitize --rust-asan \
  --archive target/websocket-rust-asan/release/libswoole_rs_websocket.a
```

## Known failures

- The original `send_close_frame` PHPT is racy: its parent sends twice then kills
  the server with no receive barrier. Its assertion stays unchanged, and
  `run.py` records it unsupported (`RACY`, #111).
- On macOS, the instrumented Rust archive compiles but fails to link because the
  runtime lacks `___asan_version_mismatch_check_v8`.
- The C++ oracle in `test_codec.py` excludes alignment sanitizing because the upstream
  codec does unaligned integer loads. Rust stdlib stays uninstrumented.
