# Timer tests

Each test's docstring says what it checks.

| Test | dev.py phase | Needs |
| --- | --- | --- |
| `test_fork.py` | `timer-fork-active` | `--engine` |
| `test_bailout.py` | `timer-bailout` | `--engine`; the first is the control |
| `benchmark.py` | `timer-benchmark` | optimized builds, an idle host |
| `test_benchmark.py` | `timer-benchmark-artifacts` | Cargo artifact identity/profile rejection checks |

The throughput benchmark reads the Cargo extension's adjacent `build-manifest.json`
and verifies its checksum, pinned upstream, source commit and optimized profile.
The upstream control retains its pristine-source and optimized Makefile checks.

```sh
python3 -m pytest tests/modules/timer/test_bailout.py \
  --engine upstream=target/upstream/src/modules/swoole.so \
  --engine cargo=target/ext/swoole.so
```

The original timer PHPTs run in the PHPT gate (`corpus/upstream/run.py --module
swoole_timer`); the network-dependent ones run only with `--external-network`.
`bug_4794_4` prints from two processes without an ordering guarantee, so it is
recorded RACY (`corpus/upstream/run.py`, #3001). The native probe's scenarios (equal deadlines, removal and
updates from callbacks and destructors, nested selection, fork teardown, the
`swoole_timer_clear` wrapper) are Rust tests in `crates/corebase/src/timer/owner.rs`. `core-tests/src/os/timer.cpp` is Rust:
the `timer.*` cases of `hooks-system-rust` (tests/modules/hooks/README.md).

The `php::Reservation::commit` rustdoc compile-fail case rejects raw callback/data pairs.
PHP reserves a node before retaining `zend::Parsed` with the tick id. A refused
reservation retains nothing; commit transfers the owned callback into the node.
Dropping an empty reservation cancels it, and a removed or replaced timer releases
the retained callback instead of installing it. The runtime has no Zend dependency. The timer
PHPTs and bailout/fork gates above exercise the unchanged callback lifecycle.

In v6.2.6's `core-tests/src/os/timer_exception.cpp`,
`callback_self_removal_preserves_deferred_release` maps to the Rust owner test
of the same name for one-shot and persistent callbacks returning normally.
The throwing branch and the two tests `callback_exception_restores_removal_state`
and `callback_exception_allows_retry` have no Rust equivalent. The callback ABI
does not return a recoverable C++ exception. Zend bailout abandons the selection
and is covered by `escaped_callback_keeps_active_metadata_and_stops_later_timers`.
