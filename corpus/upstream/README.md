# Original upstream Swoole tests

The default selection (`manifest.json`) runs original PHPT tests from Swoole **v6.2.5**, commit
`76c657d88be39b7af6bf85e46bcbac0da7387760`. The manifest records each source path,
SHA-256, subsystem, and required PHP APIs. The PHPT files are extracted with
`git archive` from the pinned Git object and checked against these hashes before
execution. Local checkout edits cannot silently change the selected tests.

The selection covers incremental HTTP request parsing, channel payloads/closure/
stats, typed Table rows and deletion, timer ordering/cancellation, coroutine
lifecycle/defer, atomics, and the original WebSocket tests. Failures are
recorded without retries to green. Network cases use ephemeral local loopback
listeners. The merge tier runs the whole pinned suite (below).

```sh
python3 tools/build_upstream.py
python3 corpus/upstream/run.py \
  --engine upstream=target/upstream/src/modules/swoole.so --require-all
python3 tools/build_ext.py
python3 corpus/upstream/run.py \
  --engine cargo=target/ext/swoole.so
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
fails on unsupported and skipped cases.

There are three explicit **test-support adaptations** in the disposable copy:

1. A small PSR-4 autoloader loads original `SwooleTest` helpers instead of installing
   unrelated Composer dependencies. Selected tests do not use those packages.
2. The original bootstrap's unrelated remote-object daemon initialization is
   removed. It otherwise attempts to write under HOME and start a daemon even for
   an atomic/codec unit test. The adapter requires the exact expected setup block
   and fails closed if it changes. All actual test bodies and assertions remain.
3. A case with an `--EXPECT_85--`, `--EXPECTF_85--` or `--EXPECTREGEX_85--`
   section gets upstream's own `tests/run-tests` selection
   (`tools/phpt_expectations.py`): on PHP >= 8.5 that body replaces the base
   expectation, below 8.5 the section is dropped. PHP's runner doesn't know these
   sections and would read one as part of the base expectation, so the case fails
   on every engine. No other byte changes.

Reports record the pinned commit, original support Git tree, unchanged runner
checksum, effective adapted-bootstrap checksum, and adapter description. HOME
and TMPDIR point into the disposable tree, and `PHP_INI_SCAN_DIR` adds an ini that
loads the engine, so a child a test starts as `$TEST_PHP_EXECUTABLE` without `-n`
(`start_server`, `ulimit.phpt`) has it, as in upstream's CI. Each test process group has an outer
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
depends on the runner's resolver reply: both engines fail on some replies (#428).
`close_max_fd.phpt` SIGTERMs its server before its first client closes, so
`onClose` for that client runs only when the worker beats the master's shutdown:
upstream 27 of 30 runs, hybrid 23, the Cargo build 21; closing the client before
the kill, 20 of 20 (#2018).

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

## Every PHPT, gated on upstream

The merge tier (`.github/workflows/phpt.yml`, called from `merge.yml`) runs every pinned
PHPT against the upstream reference and the Cargo build, both built
with PHP sockets support, in 6 shards per engine. The shards don't run as root, so the five
tests that skip unless root run again as root in the `phpt-root` job, with their own
re-runs and gate (`phpt-root.json`). The shards run:

```sh
python3 corpus/upstream/inventory.py --shard 3/6 --output manifest.json
python3 corpus/upstream/run.py --manifest manifest.json --engine cargo=/absolute/swoole.so --output cargo-3.json
python3 corpus/upstream/gate.py upstream-*.json cargo-*.json --output phpt.json
```

`inventory.py` lists every `.phpt` at the pin (family = its `tests/` directory)
and every other file under `tests/` as a support file. `--plan` prints the shard
matrix for the families selected, one shard per 150 tests up to 6, as a JSON list of
indexes (`--module swoole_server`: `[1, 2]`). `gate.py` fails if any test
that passes on upstream does not pass on the Cargo build, including when its result is
missing, and refuses interrupted shards. Tests failing on both are listed, not
hidden. Tests of upstream fixes to code that is Rust now and not yet ported
(`PENDING_PORTS`, each under its open issue) are listed as pending, not gated;
any of them cargo passes is listed so its entry is dropped. It records each result and, for skips, the SKIPIF reason and a category
(service, network, ZTS, stdext, platform, extension, other). Only the runner's
PASSED counts as passed.

A test the gate flags is re-run 3 times on both engines (in `phpt.yml`'s `gate` job,
unless more than 20 are flagged). If upstream fails any re-run, the test is listed
as flaky on upstream and doesn't fail the gate, provided cargo passes one of its
re-runs or upstream then passes fewer than a third of 9 re-runs (a test both engines
almost always fail, whose first upstream pass was the outlier). If upstream passes every re-run and cargo fails any, it is a regression; when
cargo both passed and failed, upstream first gets up to 24 re-runs in all, and an upstream
failure among them makes the test flaky on both engines instead.
If both pass every re-run, it is listed as passed on re-run and doesn't fail the gate.
Cargo never gets retried to green: it must pass every re-run.

A re-run whose test passed but whose runner then died with `corrupted double-linked
list` / `Aborted` (#200: both engines hit it at the same rate) is inconclusive on
either engine. It doesn't count towards the verdict and gets one more re-run in its
place. A test whose re-runs on an engine are all inconclusive fails the gate, listed
as inconclusive.

Before that, the PR tier's `phpt` job runs the families of the modules a PR
touches (each module in `migration/modules.json` lists its `phpt_families`; every family
for a change to shared build inputs) through the same `phpt.yml`, on the Cargo build alone,
6 shards with the services those families reach. Upstream doesn't change between PRs, so the gate takes upstream's
reports and `upstream.so` from main's latest merge-tier run whose upstream shards passed
(`tools/ci.py phpt-baseline`), cut to those families. That baseline holds only while
what upstream's results depend on is unchanged since its commit: the pin and its build,
the runner, the services and packages (`UPSTREAM_KEY` and `BASELINES` in `tools/ci.py`).
A PR that changes one of those, or anything under `corpus/upstream/` (pr.yml's own
check), runs every family, and upstream too. `--variant` finds the c-ares, io_uring and thread-context
workflows' upstream reports the same way, and `--php` also requires the PHP release.

`services.yml` holds those services, and `services.py` prints the ones some families reach
(every one for a family it doesn't list); `.github/actions/phpt-env` starts what its
`services` input names. Locally, with the names in `/etc/hosts` as phpt-env writes them:
`docker compose -f corpus/upstream/services.yml up -d --wait $(python3 corpus/upstream/services.py swoole_runtime)`.

To run some families on both engines locally, `families.sh` re-runs and gates them the same
way (`dev.py candidate` does it for the modules it selects):

```sh
corpus/upstream/families.sh target/reports/phpt target/upstream/src/modules/swoole.so \
  target/ext/swoole.so --module swoole_timer
```

Those run on Ubuntu's PHP 8.3. Every night `.github/workflows/php-versions.yml`
builds upstream and the Cargo build against each other release upstream supports
(8.2, 8.4, 8.5, from source by `.devcontainer/php.sh` with `PHP_SERIES` set)
and gates every PHPT the same way, in 4 shards per version and without services.

## On a debug PHP

`.github/workflows/debug.yml` (nightly) builds PHP with `--enable-debug`, builds
upstream and the Cargo swoole.so against it, and runs every pinned PHPT on both in 6
shards, with the re-runs and gate above (`phpt-debug.json`). A debug PHP prints Zend's
leak report at request end and aborts on a failed assertion, so either fails the
test. The summary lists each failure that carries one by engine, upstream's own
included; only a cargo failure of a test upstream passes fails the gate. Each
cargo-only report gets a `kind:bug`. There are no services and no external network,
so tests that need them fail or skip on both engines.

## Attributing a regression

The gate runs every PHPT once against the Cargo build. When it reports a test that
passes on upstream but not on cargo, reproduce just that test and bisect the commit range:

```sh
python3 tools/build_ext.py
python3 corpus/upstream/run.py --source target/upstream/src \
  --case tests/swoole_http_server/example.phpt \
  --engine upstream=target/upstream/src/modules/swoole.so \
  --engine cargo=target/ext/swoole.so
```

Add `--external-network` for tests that need outside services.

The ZTS original `swoole_thread/server/exit.phpt` stays registered unchanged, but
its startup output interleaves on both engines (#1120), so on a verified ZTS
runtime it is recorded as `excluded`. The ZTS workflow runs the repository-owned
companion `tests/modules/integration/server_exit.py` instead, which keeps the
original's exit, restart and output assertions; its report does not claim the
original passed.
