# Rust bridge integration gate

Build the pinned reference and Rust hybrid, then run:

```sh
python3 tests/modules/integration/test_bridge.py
python3 tests/modules/integration/test_bridge.py --hybrid target/hybrid/rust-websocket/modules/swoole.so
```

Requires PHP, C/C++ compilers, `ar`, `make`, and the hybrid's existing libtool
object files. The selected build supplies its own copied static archive and
expected feature bits from its manifest, so compiling another feature combination
does not change this test's inputs. Override with `--archive`/`--feature-bits`;
select extensions with `--reference`/`--hybrid`, and PHP with `--php`.

The tests execute C and C++ consumers, check the real reference/hybrid PHP
markers and PHPinfo, and prove PHP rejects an incompatible bridge ABI. That
negative test relinks existing hybrid objects against a temporary C archive
returning ABI 999. For a WebSocket-enabled build it also proves startup rejects
an ABI-1 archive missing the required feature bit. Temporary fixtures export all
required symbols so the loader reaches the actual module startup check on Linux.
It runs libtool in a temporary directory and loads only the
temporary extension. It neither rebuilds sources nor replaces published modules.
No unsupported or missing artifact is reported as a pass.
`--startup-only` runs only the positive startup and flag checks; the candidate
lane's `abi` phase uses it, and the integrate lane runs every test.

## Other gates

- `zts.py --checkout DIR --source S --rust-export export.json --output R` needs
  Linux with a ZTS PHP. It builds native and selected ZTS extensions from a
  verified Rust export and checks startup, ZTS, feature bits and the
  `Swoole\Thread\Lock` class. `--locks-threads` also runs
  `tests/modules/locks/threads.py`.
- `zts_originals.py --checkout DIR --source S --zts-report R --output R2` reuses
  the worlds that `zts.py` built. `--profile stdext-originals` (the default)
  runs the complete stdext PHPT originals. `collections` runs the
  phpcollections fixtures and `statistics` runs the read-only statistics
  fixture. Every profile requires both engines to pass and agree.
- `test_bridge_inventory.py` is a plain unittest (no build needed) for the
  selected-export check in `test_bridge.py`.

## Gate audit (#486)

Kept (5):

- `ownership` (`tools/check_migration.py`) and `reference-build`: the migration
  manifest check and the upstream build that every comparison needs.
- `abi`: the only C and C++ consumer checks of the bridge headers, and the only
  check that PHP rejects an incompatible or feature-short bridge archive.
- `api-surface`: the full reflected Swoole API against upstream.
- `reference-identity`: the reference extension's identity, which the
  base64/crc32 consumer phases read.

Deleted (1): `abi-startup` ran `test_bridge.py --startup-only`, four of the six
tests that `abi` runs in full in the same integrate lane.

`zts.py` and `zts_originals.py` are not phases: they need a ZTS PHP, which no
CI lane has.

## Captured Server effects

`server_faults.py --archive <build>/rust-prefix/lib/libswoole_rs_ffi.a --output R`
(`server-native-faults`) links the actual optimized archive and its copied native
exception helper. C++14/17 runs cover typed and non-standard exceptions, nested
Server operations, and exact effect prefixes after every injected failure in
create/destroy, both with native facts and with the canonical Rust configuration
records. The latter asserts those facts do not fall back through the native
callback. Native ASan/UBSan runs are separate; Rust instrumentation is not
claimed. Linux additionally wraps `free` to prove the production Rust task-path
String is released before the captured exception is rethrown. Other hosts report
that drop observation as unmeasured.

`ServerReply` carries an ordinary C++ exception outcome; the native invocation
owns its `exception_ptr`. Rust uses `Result` to stop before any later effect and
release local owners, then the native caller rethrows. The legacy nonthrowing
Server ABI remains. This guard does not transport Zend bailout, custom whole-thread
exit or pthread forced unwind. Generic callback ownership remains gated on their
separate composed transport (#1145/#1167).
