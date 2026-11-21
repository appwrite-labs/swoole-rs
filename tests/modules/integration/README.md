# Rust bridge integration gate

Build the pinned reference and the Cargo extension (`tools/build_ext.py`), then run:

```sh
python3 tests/modules/integration/test_bridge.py --startup-only
python3 tests/modules/integration/test_bridge.py --archive path/to/libswoole_rs_ffi.a
```

Requires PHP and C/C++ compilers. Select extensions with `--reference` and
`--extension` (default `target/ext/swoole.so`) and PHP with `--php`. The expected feature bits are
every module's. The C and C++ consumers link a Zend-free `--archive`.

The tests execute C and C++ consumers, check the ABI exports every module's
headers declare under the build's `config.h` (which `build_ext.py` writes beside
`swoole.so`), and check the real reference/Cargo PHP markers and PHPinfo. They
write only to temporary directories and neither rebuild sources nor replace
published modules.
No unsupported or missing artifact is reported as a pass.
`--startup-only` runs only the positive startup and flag checks; the candidate
lane's `abi` phase uses it, and the integrate lane runs every test.

## Other gates

- `test_bridge_inventory.py` is a plain unittest (no build needed) for the
  ABI export check in `test_bridge.py`.
