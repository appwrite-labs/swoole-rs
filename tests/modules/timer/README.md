# Timer tests

Each test's docstring says what it checks.

| Test | dev.py phase | Needs |
| --- | --- | --- |
| `test_kernel.py` | `timer-kernel` | builds `swoole-rs-timer` unless `--archive`; `--sanitize`, `--rust-asan` |
| `test_adapter.py` | `timer-native` | `--build-manifest` and `--archive` of `tools/build_ext.py --native`; `--sanitize` on macOS only (Linux ASan rejects PHP's `RTLD_DEEPBIND` FFI load) |
| `test_fork.py` | `timer-fork-active` | `--engine` |
| `test_bailout.py` | `timer-bailout` | `--engine`; the first is the control |
| `benchmark.py` | `timer-benchmark` | optimized builds, an idle host |

```sh
python3 -m pytest tests/modules/timer/test_bailout.py \
  --engine upstream=target/upstream/src/modules/swoole.so \
  --engine cargo=target/ext/swoole.so
```

The original timer PHPTs run in the PHPT gate (`corpus/upstream/run.py --module
timers`); the network-dependent ones run only with `--external-network`.
`bug_4794_4` prints from two processes without an ordering guarantee and is
flaky on both engines. `core-tests/src/os/timer.cpp` is Rust:
the `timer.*` cases of `hooks-system-rust` (tests/modules/hooks/README.md).
