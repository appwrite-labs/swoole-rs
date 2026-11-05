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

`ServerReply` preserves fault1 for ordinary C++, fault2 for Zend bailout and
fault3 for sticky whole-thread exit, and fault4 for externally held ordinary
C++ provenance from the native OS entry. The native invocation snapshots the optional
MINIT provider once. Its C trap encloses ordinary C++ capture; Rust stops on the
first fault and releases local owners. C++ rethrow naturally destroys invocation;
Zend/thread resume happens after the invocation and its exception_ptr custody
leave scope; fault4 relays to the provider rather than rethrowing across an
outer Rust thread frame. PHP-disabled SDKs keep the unregistered provider path, without Zend
relocations. The legacy nonthrowing ABI remains; forced unwind is outside this
contract.

`server_effects.py` (`server-native-effects`, Linux integrate) reuses the completed
worker-retirement SDK when available, otherwise builds untouched original IPC
GoogleTests. It checks real production SDK absent/registered-provider behavior
across registration and Server translation units in C++14/17, last local exception
payload release before nonlocal resume, separate external C++ custody, and no later effects. Separate actual
archive probes check typed fault prefixes and production Rust String release,
with native ASan/UBSan runs; Rust instrumentation is not claimed.

Optional `--php`, `--php-config`, `--native-extension`, `--selected-extension` and
`--selected-source` inputs run a real ZTS pair through a companion resolving Core
Server methods from the loaded production DSO. Its PHP callback covers return,
fatal, custom whole-thread exit and a returned object's destructor through the
native Variable facade free. Status and post-call Atomic assertions are exact.
This fixture starts no server loop or fork. Abandoned fixture Server lifetime ends
with its isolated process; this does not qualify wider server lifetime or leaks.
