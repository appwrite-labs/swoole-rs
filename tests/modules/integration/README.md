# Rust bridge integration gate

Build the pinned reference and Rust hybrid, then run:

```sh
python3 tests/modules/integration/test_bridge.py
python3 tests/modules/integration/test_bridge.py --hybrid target/hybrid/rust/modules/swoole.so
```

Requires PHP and C/C++ compilers. The selected build supplies its own copied
static archive and expected feature bits from its manifest, so compiling another
feature combination does not change this test's inputs. Override with
`--archive`/`--feature-bits`; select extensions with `--reference`/`--hybrid`,
and PHP with `--php`.

The tests execute C and C++ consumers, check the selected ABI exports and the
real reference/hybrid PHP markers and PHPinfo. They write only to temporary
directories and neither rebuild sources nor replace published modules.
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

`zts.py` and `zts_originals.py` are not phases: they need a ZTS PHP, which no
CI lane has.
