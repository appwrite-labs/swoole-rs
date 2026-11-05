# Original timer tests

## Original PHPTs

All 35 `tests/swoole_timer/*.phpt` files are in the `timers` group of
`corpus/upstream/manifest.json` and run one per process group with PHP's
official runner (`corpus/upstream/run.py`). Assertions, expected output, skip
probes and real sleeps are unchanged.

```sh
python3 corpus/upstream/run.py --module timers --require-all \
  --engine upstream=/absolute/path/to/upstream/swoole.so \
  --engine timer=/absolute/path/to/timer/swoole.so
```

By default the 31 local cases run and the four network-dependent cases
(`bug_4794_3` qq.com, `bug_4794_5` negative DNS, `bug_4794_6` baidu.com,
`bug_4794_7` 11.11.11.11 timeout) are listed as `unmeasured_cases`, not passes.
`--external-network` runs them online (`--online` or `SKIP_ONLINE_TESTS=0`,
depending on the runner). `--case PATH` selects single originals. With
`--require-all`, every selected case must pass with no skip, warning, XFAIL or
missing result.

`bug_4794_8` binds fixed port 127.0.0.1:9601; the runner checks the port first
and records an occupied port as a failure without touching the listener. Run no
other 9601 workload concurrently.

At v6.2.2, all 33 then-pinned cases passed per engine on macOS PHP 8.5 and Linux
PHP 8.2 (with `--external-network`). v6.2.4 added `clear_all_destructor_clears`
and `memory_limit_fatal_error`.

### Known failure: `bug_4794_4` output order

The original prints from a parent and child process around atomic wakeups and
SIGUSR2 without a cross-process ordering guarantee. Over 100 Linux trials per
engine, upstream had 94 passed / 4 warned / 2 failed and Rust 94 passed /
6 warned; all mismatches were one of three adjacent-pair reorderings (1/2, 5/6,
7/8). The strict gate fails intermittently on both engines; the official
runner's retry turns these into WARNED.

## Original native GoogleTests

`core-tests/src/os/timer.cpp` runs unchanged on the reference and the hybrid
tree as the `timer-native-originals` phase (`tools/core_tests.py`, Linux), like
the other modules' GoogleTests (`CORE_TESTS` in `tools/dev.py`). The original
`timer.delay` case prints "timer is not available" warnings; they are original
output.

## Other timer gates

`benchmark.py`: optimized PHP timer throughput (cancel/dispatch/repeat), with
an engine named `upstream` as control. It requires matching PHP builds, `-O2`,
`-O3` or `-Os` native flags, a clean upstream tree at the pin, and extension
hashes matching their build manifests. Run only with no concurrent builds/tests.

```sh
python3 tests/modules/timer/benchmark.py \
  --engine upstream=/absolute/upstream/swoole.so \
  --engine timer=/absolute/timer/swoole.so --repeat 7
```

See also: [kernel.md](kernel.md), [hybrid.md](hybrid.md),
[FORK.md](FORK.md), [bailout.md](bailout.md),
[destructor_bailout.md](destructor_bailout.md).
