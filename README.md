# swoole-rs

[![Merge tier](https://github.com/loks0n/swoole-rs/actions/workflows/merge.yml/badge.svg?branch=main)](https://github.com/loks0n/swoole-rs/actions/workflows/merge.yml?query=branch%3Amain)
[![Upstream C++ replaced](https://img.shields.io/endpoint?url=https%3A%2F%2Fraw.githubusercontent.com%2Floks0n%2Fswoole-rs%2Fbadges%2Freplaced.json)](docs/migration.md#upstream-files-still-in-the-build)
[![Graduated leaf modules](https://img.shields.io/endpoint?url=https%3A%2F%2Fraw.githubusercontent.com%2Floks0n%2Fswoole-rs%2Fbadges%2Fgraduated.json)](https://github.com/loks0n/swoole-rs/issues/125)
[![Upstream PHPTs passing](https://img.shields.io/endpoint?url=https%3A%2F%2Fraw.githubusercontent.com%2Floks0n%2Fswoole-rs%2Fbadges%2Fphpt.json)](https://github.com/loks0n/swoole-rs/issues/58)

Progress ([roadmap](docs/migration.md#roadmap)): phases 1 to 3 are done; open are
[Phase 4, deepen thin modules](https://github.com/loks0n/swoole-rs/issues/439),
[Phase 5, Zend surface and host](https://github.com/loks0n/swoole-rs/issues/754),
[speed](https://github.com/loks0n/swoole-rs/issues/366), [differential fuzzing](https://github.com/loks0n/swoole-rs/issues/100) and
[lint-enforced Rust](https://github.com/loks0n/swoole-rs/issues/160). The badges come from `tools/badges.py`
(`.github/workflows/badges.yml`): upstream C++ replaced counts upstream first-party
lines the all-modules patch set deletes, and the PHPT badge names the commit its
merge-tier run measured.

A gradual C/C++ to Rust migration of the [Swoole](https://github.com/swoole/swoole-src)
PHP extension. Upstream Swoole v6.2.4 (pinned at `c7e51450`) keeps its PHP runtime;
bounded subsystems are replaced with Rust linked into the upstream extension through
a C ABI. Every build links every module; the pinned upstream build is the oracle.
Full coverage, parity and performance remain in progress. See the
[migration approach](docs/migration.md) and [hybrid variants and evidence](docs/hybrid-build.md).

## Build

Requires Rust/Cargo, PHP CLI with matching development headers (`php-config`), and
the upstream build toolchain described in the [corpus instructions](corpus/README.md).

```sh
python3 tools/build_upstream.py
python3 tools/build_hybrid.py
php -n -d extension="$PWD/target/hybrid/rust/modules/swoole.so" -r 'echo SWOOLE_VERSION, "\n";'
```

Do not load the hybrid alongside another Swoole extension: both register the same classes.

## Verify

For edit/candidate loops, incremental hybrid builds and the reusable Linux toolchain,
see [local development](docs/development.md).

```sh
git config core.hooksPath .githooks   # pre-commit: fmt, clippy, manifest, tooling tests (~30s)
python3 tools/dev.py integrate --module timer   # the timer gates, on the all-modules build
```

CI has two tiers. [PR](.github/workflows/pr.yml) runs Rust tests, tooling tests and the
application corpus against a cached hybrid build. [Merge](.github/workflows/merge.yml)
runs every gate and original PHPT for all modules on `main`.

## Modules, corpus and benchmarks

The [migration work packages](docs/migration-modules.md) and
[module manifest](migration/modules.json) define the modules, dependency waves,
exclusive paths, ABI ownership and acceptance gates for concurrent agents. Run
`python3 tools/check_migration.py` before dispatching work.

The [project corpus](corpus/README.md) runs application projects against any engine,
and the [original upstream PHPT selection](corpus/upstream/README.md) runs pinned
Swoole tests with their original assertions. Missing APIs are reported as
unsupported; behavioral failures fail the run. The [benchmark harness](benchmarks/README.md)
compares optimized hybrid and upstream builds. The [validation layers](docs/validation-suite.md)
add allocation-failure models, lifecycle scenarios, an Autobahn RFC profile and an
unchanged pinned Hyperf app.
