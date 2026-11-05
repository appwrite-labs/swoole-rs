# Original upstream Swoole tests

This selection runs **92 original PHPT tests** from Swoole **v6.2.4**, commit
`c7e514509451beafd4d70ce5b78943d51e76381c`. The manifest records each source path,
SHA-256, subsystem, and required PHP APIs. The PHPT files are extracted with
`git archive` from the pinned Git object and checked against these hashes before
execution. Local checkout edits cannot silently change the selected tests.

The selection covers incremental HTTP request parsing, channel payloads/closure/
stats, typed Table rows and deletion, timer ordering/cancellation, coroutine
lifecycle/defer, atomics, and WebSocket frame encoding/decoding. All 33 original
WebSocket tests exercise receive decoding, send encoding, fragmented messages,
ping/pong, malformed data, close flags/status/reasons, handshake token order
and callback/property registration. Some retain upstream randomized server modes
or close-code branches; failures are recorded without retries to green. Network cases use ephemeral local loopback
listeners; no external databases or internet services are required. The frame test includes
1,000 randomized round trips with invariant assertions; this is not the whole
upstream suite and does not replace its larger regression/stress suites.

```sh
python3 tools/build_upstream.py
python3 corpus/upstream/run.py \
  --engine upstream=target/upstream/src/modules/swoole.so --require-all
python3 corpus/upstream/run.py \
  --engine hybrid=target/hybrid/rust/modules/swoole.so
# Target a migration module:
python3 corpus/upstream/run.py \
  --engine upstream=target/upstream/src/modules/swoole.so --module timers --require-all
```

Each manifest entry has `path`, `sha256`, `module` (its selection group),
`requires` (engine capabilities) and optionally `network: true` for cases that
need DNS or external routes. Network cases are left out and listed as
`unmeasured_cases` unless `--external-network` is given, which runs them online.
The `timers` group holds all 35 pinned `tests/swoole_timer` cases. Cases that
bind a fixed address (`FIXED_PORTS` in `run.py`) fail without running if a live
listener holds it.

`--engine` is repeatable; `--php` and `--php-extension` behave like the application
corpus runner. `--source` selects an existing checkout at exactly the pinned
commit. `--module` is repeatable; groups are `http-request`, `channels`, `table`,
`timers`, `coroutines`, `atomics`, `websocket`, and `websocket-network`. Unknown/empty selections fail.
On systems with dynamic modules, load `pdo`, `pdo_sqlite`, `curl`, and `sockets`
uniformly through repeatable `--php-extension` flags. Built-in macOS modules
need no flags; duplicate module warnings can affect exact PHPT outputs.
The builder supplies `run-tests.php` via `phpize`; its checksum is recorded in
the report alongside the PHP version. This generated runner is not a Swoole Git file.

Tests execute through **PHP's official `run-tests.php` PHPT runner**, supplied by the matching PHP installation through `phpize`, in a
disposable tree. PHPT `FILE`, `SKIPIF`, expected-output sections, assertions, and
the official PHP runner itself are unmodified. Swoole's custom runner is intentionally
not used: it contains failure cleanup that can kill unrelated listeners by port.
The wrapper instead cleans up only each test's isolated process group. Its `-W` result ledger is parsed; only `PASSED`
counts as a pass. Failures, unexpected statuses, runner errors, and timeouts fail
the command. Unsupported APIs are reported separately; `--require-all` also
fails on unsupported and skipped cases. Rust currently cannot run this selection
because its APIs and upstream bootstrap prerequisites are missing. Those
unsupported results are compatibility gaps, not passes.

There are two explicit **test-support adaptations** in the disposable copy:

1. A small PSR-4 autoloader loads original `SwooleTest` helpers instead of installing
   unrelated Composer dependencies. Selected tests do not use those packages.
2. The original bootstrap's unrelated remote-object daemon initialization is
   removed. It otherwise attempts to write under HOME and start a daemon even for
   an atomic/codec unit test. The adapter requires the exact expected setup block
   and fails closed if it changes. All actual test bodies and assertions remain.

Reports record the pinned commit, original support Git tree, unchanged runner
checksum, effective adapted-bootstrap checksum, and adapter description. HOME
and TMPDIR point into the disposable tree. Each test process group has an outer
timeout and is terminated with its children, including on failure. This is
process/filesystem isolation for the selected trusted tests, not an OS security
sandbox. Bootstrap-dependent tests explicitly declare `upstream_test_bootstrap`
as well as their subsystem capabilities. Network cases additionally probe
`coroutine_http_client` or `coroutine_tcp_client`; raw synchronous helpers require `sync_tcp_client`,
plus `php_sockets` for
the helper’s socket calls; if these APIs exist but misbehave, tests
run and fail normally.

JSON defaults to `target/reports/upstream-phpt.json`. Adjacent `*-artifacts/`
directories retain runner logs, its result ledger, and available expected/actual/
diff/generated-PHP files per engine/test. Tests are never blessed or rewritten to
match the prototype. Source is fetched rather than vendored; upstream copyright,
license notices, helpers, and license file are preserved in the disposable copy.

`send_close_frame.phpt` is racy on upstream: its parent kills the server as soon
as the client's two pushes return, usually before the server has handled the
second frame, so the expected `lalalala` is never printed. On Linux upstream
passed 1 of 20 runs; with a 500 ms wait before the kill, 20 of 20 (#111). It
stays registered with its assertions intact, and `RACY` in `run.py` records it
unsupported instead of running it. So does `dnslookup_ipv6.phpt`, whose verdict
depends on the runner's resolver reply: both engines fail on some replies (#428),
and `shutdown_in_master.phpt`, which calls `shutdown()` from `onStart` before its
random 1–4 workers are up and sometimes hangs until the deadline: upstream 2 of 8
runs, hybrid 3 of 8 in the dev container (#438).

The expanded audit registers all 33 original WebSocket PHPTs, plus timer `info`,
`stats`, `clearAll`, `after_fork`, `reinit_1` and `reinit_2`. Original timer assertions
run through the official runner before any M03 rewrite. `memory.phpt` retains its original
`skip_if_no_top()` and is counted as skipped on macOS; skips never become passes.

Module owners can pass `--manifest /absolute/selection.json` to select additional
original PHPTs without changing the central selection. Use the same pinned
`commit` and test fields (`path`, `sha256`, `module`, `requires`). Original bytes
are checked from pinned Git objects; capability failures and strict skip handling
are unchanged. Reports record the selection path and SHA256. New selections do
not expand the runner's audited parallel-safe allowlist.

Owned selections may include `support_files: [{"path": "examples/test.jpg",
"sha256": "..."}]`. The runner extracts those files from the same pinned Git
commit, verifies their hashes before executing tests, and records the declarations
in the report. Paths must be relative repository files. Working-copy assets are
ignored; a missing asset or checksum mismatch fails the run. This adds fixtures
without modifying the original PHPT assertions or capability probes.

The separate `versioned.py` profile supports the pinned upstream `_85` expectation
sections using the exact `PHP_VERSION_ID >= 80500` selection in `tests/run-tests`.
It reads that file only to verify the frozen rules hash; execution still uses the
matching installation's official PHP runner. Original FILE/SKIPIF/other bodies
and the selected expectation body are byte-identical. Receipts include original
and adapted PHPT hashes, protected-section hashes, selected body hash, PHP version,
actual extension and official-runner identities. PHP8.5 uses explicitly recorded
`fatal_error_backtraces=On` to match the pinned stack-trace expectation.

`framing-versioned-expectation` is an additional, explicitly adapted diagnostic
for `wrong_eof_setting.phpt`. The normal complete framing inventory remains
untouched and retains every first failed/skipped/warned/unsupported outcome. A
passing adapted case is not a passing untouched original or whole inventory;
those report fields stay false. Unknown/duplicate or competing assertion sections
are not silently discarded to get a pass.


`tree.py --source PINNED --group swoole_stdext --module phphelpers --requires
stdext --output PRIVATE_MANIFEST` inventories every original PHPT and support
blob from the pinned tree, without exclusions or assertion changes. It rejects
changed tracked originals/support, wrong source heads and traversal groups.
The shared Git blob reader batches content extraction. The31 stdext cases require
actual loaded stdext functions; disabled builds report unsupported. A syntax-only
SDK profile does not establish actual stdext runtime support. The canonical
`tools/dev.py integrate --module phphelpers --stdext` plan builds genuine enabled
native and all-modules worlds before running the official original runner.

## Every PHPT, gated on upstream

The merge tier (`phpt-*` jobs in `.github/workflows/merge.yml`) runs every pinned
PHPT against the upstream reference and the all-modules hybrid, both built with
PHP sockets support, in 6 shards per engine:

```sh
python3 corpus/upstream/inventory.py --shard 3/6 --output manifest.json
python3 corpus/upstream/run.py --manifest manifest.json --engine hybrid=/absolute/swoole.so --output hybrid-3.json
python3 corpus/upstream/gate.py upstream-*.json hybrid-*.json --output phpt.json
```

`inventory.py` lists every `.phpt` at the pin (family = its `tests/` directory)
and every other file under `tests/` as a support file. `gate.py` fails if any test
that passes on upstream does not pass on the hybrid, including when its result is
missing, and refuses interrupted shards. Tests failing on both are listed, not
hidden. It records each result and, for skips, the SKIPIF reason and a category
(service, network, ZTS, stdext, platform, extension, other). Only the runner's
PASSED counts as passed.

A test the gate flags is re-run 3 times on both engines (in the `phpt-gate` job,
unless more than 20 are flagged). If upstream fails any re-run, the test is listed
as flaky on upstream and doesn't fail the gate, provided the hybrid passes one of its
re-runs or upstream then fails up to 9 re-runs too (a test both engines almost always
fail, whose first upstream pass was the outlier). If upstream passes every re-run and the hybrid fails any, it is a regression; when the
hybrid both passed and failed, upstream first gets up to 9 re-runs in all, and an upstream
failure among them makes the test flaky on both engines instead.
If both pass every re-run, it is listed as passed on re-run and doesn't fail the gate.
The hybrid never gets retried to green: it must pass every re-run.

A re-run whose test passed but whose runner then died with `corrupted double-linked
list` / `Aborted` (#200: both engines hit it at the same rate) is inconclusive on
either engine. It doesn't count towards the verdict and gets one more re-run in its
place. A test whose re-runs on an engine are all inconclusive fails the gate, listed
as inconclusive.

Before that, the PR tier's `gates` job runs the families of the modules a PR
touches: each module in `migration/modules.json` lists its `phpt_families`, and
`dev.py candidate` hands them to `families.sh`, which runs them on both engines,
re-runs and gates them the same way. A change that selects every module is left
to the merge tier.

```sh
corpus/upstream/families.sh target/reports/phpt target/upstream/src/modules/swoole.so \
  /absolute/hybrid/swoole.so --module swoole_timer
```

## Attributing a regression

The gate runs every PHPT once against the all-modules hybrid, the only hybrid
there is. When it reports a test that passes on upstream but not on the hybrid,
reproduce just that test and bisect the commit range:

```sh
python3 tools/build_hybrid.py
python3 corpus/upstream/run.py --source target/upstream/src \
  --case tests/swoole_http_server/example.phpt \
  --engine upstream=target/upstream/src/modules/swoole.so \
  --engine hybrid=target/hybrid/rust/modules/swoole.so
```

Add `--external-network` for tests that need outside services, and repeat with
the next candidate module until the failure follows one.

The ZTS original `swoole_thread/server/exit.phpt` remains registered with its
unchanged source and expectation. Its two startup echo operands interleave on
native and hybrid (#1120: native attempt 44 and hybrid attempts 4/17/30/32 of the
paired 45-run stress). Only a verified ZTS runtime with the Thread class records
this named original as `excluded`, separately from passed, skipped and unsupported.
The ZTS workflow requires `tests/modules/integration/server_exit.py` instead: its
repository-owned companion retains the exit-235, failed HTTP request, replacement
worker, final count and exact output assertions, writes complete lines before
readiness, waits for replacement readiness explicitly and retains the original
200 ms shutdown delay. A separate process requests master shutdown with SIGTERM;
it does not call shutdown from a participating Server Thread. This avoids the
distinct native shutdown hang reproduced by the earlier companion. The companion
retains the original exit/restart assertions; it does not qualify the added-process
shutdown calling context. Its report does not claim the original passed. WARNED original runs remain failed.
