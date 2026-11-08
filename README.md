# swoole-rs

[![Merge tier](https://github.com/loks0n/swoole-rs/actions/workflows/merge.yml/badge.svg?branch=main)](https://github.com/loks0n/swoole-rs/actions/workflows/merge.yml?query=branch%3Amain)
[![Upstream C++ replaced](https://img.shields.io/endpoint?url=https%3A%2F%2Fraw.githubusercontent.com%2Floks0n%2Fswoole-rs%2Fbadges%2Freplaced.json)](docs/migration.md#upstream-files-still-in-the-build)
[![Upstream PHPTs passing](https://img.shields.io/endpoint?url=https%3A%2F%2Fraw.githubusercontent.com%2Floks0n%2Fswoole-rs%2Fbadges%2Fphpt.json)](https://github.com/loks0n/swoole-rs/issues/58)
[![Speed vs upstream](https://img.shields.io/endpoint?url=https%3A%2F%2Fraw.githubusercontent.com%2Floks0n%2Fswoole-rs%2Fbadges%2Fspeed.json)](https://github.com/loks0n/swoole-rs/blob/badges/speed.md)
[![Unsafe blocks](https://img.shields.io/endpoint?url=https%3A%2F%2Fraw.githubusercontent.com%2Floks0n%2Fswoole-rs%2Fbadges%2Funsafe.json)](https://github.com/loks0n/swoole-rs/issues/1902)

Progress ([roadmap](docs/migration.md#roadmap)): phases 1 to 5 are done and nothing is
compiled from upstream; open are [dropping upstream from the build](https://github.com/loks0n/swoole-rs/issues/1921),
[every applicable PHPT passing](https://github.com/loks0n/swoole-rs/issues/1957),
[faster than upstream](https://github.com/loks0n/swoole-rs/issues/1894) and
[a hardened runtime](https://github.com/loks0n/swoole-rs/issues/1893). The badges come from `tools/badges.py`
(`.github/workflows/badges.yml`): upstream C++ replaced counts upstream first-party
lines the all-modules patch set deletes, and the PHPT badge counts tests the hybrid
passes in any of a merge-tier run's PHPT jobs, over those that can apply to 64-bit Linux
(`corpus/upstream/not_applicable.json` lists the rest), naming the commit it measured. The speed badge links to the latest merge-tier callgrind
results, per workload, against upstream. The unsafe badge counts `unsafe` blocks, fns and impls in
non-test crate code; `tools/unsafe_budget.py` (PR tier) fails a PR that raises any
crate above `tools/unsafe_baseline.json`, and `--update` lowers the baseline.

A gradual C/C++ to Rust migration of the [Swoole](https://github.com/swoole/swoole-src)
PHP extension. Rust is linked into the build of upstream Swoole v6.2.5 (pinned at
`76c657d8`), which now compiles little upstream code beyond its build system (see
the badge). Every build links
every module; the pinned upstream build is the oracle. See the
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
git config core.hooksPath .githooks   # pre-commit: fmt, clippy, manifest, patches, tooling tests (~5s warm)
python3 tools/dev.py integrate --module timer   # the timer gates, on the all-modules build
```

CI has two tiers. [PR](.github/workflows/pr.yml) runs Rust tests, tooling tests and the
application corpus against a cached hybrid build. [Merge](.github/workflows/merge.yml)
runs every gate and original PHPT for all modules on `main`.

## Modules, corpus and benchmarks

The [module manifest](migration/modules.json) ([fields](docs/migration-modules.md))
defines the modules, dependency waves, owned paths, ABI contracts and PHPT
families; `python3 tools/check_migration.py` validates it.

The [project corpus](corpus/README.md) runs application projects against any engine,
and the [original upstream PHPT selection](corpus/upstream/README.md) runs pinned
Swoole tests with their original assertions. Missing APIs are reported as
unsupported; behavioral failures fail the run. The [benchmark harness](benchmarks/README.md)
compares optimized hybrid and upstream builds. The [validation layers](docs/validation-suite.md)
add lifecycle scenarios, an Autobahn RFC profile and an
unchanged pinned Hyperf app.
