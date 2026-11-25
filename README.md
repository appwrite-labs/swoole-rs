# swoole-rs

[![Merge tier](https://img.shields.io/endpoint?url=https%3A%2F%2Fraw.githubusercontent.com%2Fappwrite-labs%2Fswoole-rs%2Fbadges%2Fmerge.json)](https://github.com/appwrite-labs/swoole-rs/actions/workflows/merge.yml?query=branch%3Amain)
[![Upstream PHPTs passing](https://img.shields.io/endpoint?url=https%3A%2F%2Fraw.githubusercontent.com%2Fappwrite-labs%2Fswoole-rs%2Fbadges%2Fphpt.json)](https://github.com/appwrite-labs/swoole-rs/actions/workflows/merge.yml?query=branch%3Amain)
[![Speed vs upstream](https://img.shields.io/endpoint?url=https%3A%2F%2Fraw.githubusercontent.com%2Fappwrite-labs%2Fswoole-rs%2Fbadges%2Fspeed.json)](https://github.com/appwrite-labs/swoole-rs/blob/badges/speed.md)
[![Unsafe blocks](https://img.shields.io/endpoint?url=https%3A%2F%2Fraw.githubusercontent.com%2Fappwrite-labs%2Fswoole-rs%2Fbadges%2Funsafe.json)](https://github.com/appwrite-labs/swoole-rs/blob/badges/unsafe.md)
[![crates/ line coverage](https://img.shields.io/endpoint?url=https%3A%2F%2Fraw.githubusercontent.com%2Fappwrite-labs%2Fswoole-rs%2Fbadges%2Fcoverage.json)](https://github.com/appwrite-labs/swoole-rs/blob/badges/coverage.md)
[![Release](https://img.shields.io/github/v/release/appwrite-labs/swoole-rs?include_prereleases&label=release)](https://github.com/appwrite-labs/swoole-rs/releases)

[![Miri](https://github.com/appwrite-labs/swoole-rs/actions/workflows/miri.yml/badge.svg?branch=main)](https://github.com/appwrite-labs/swoole-rs/actions/workflows/miri.yml?query=branch%3Amain)
[![Kani](https://github.com/appwrite-labs/swoole-rs/actions/workflows/kani.yml/badge.svg?branch=main)](https://github.com/appwrite-labs/swoole-rs/actions/workflows/kani.yml?query=branch%3Amain)
[![AddressSanitizer](https://github.com/appwrite-labs/swoole-rs/actions/workflows/asan.yml/badge.svg?branch=main)](https://github.com/appwrite-labs/swoole-rs/actions/workflows/asan.yml?query=branch%3Amain)
[![ThreadSanitizer](https://github.com/appwrite-labs/swoole-rs/actions/workflows/tsan.yml/badge.svg?branch=main)](https://github.com/appwrite-labs/swoole-rs/actions/workflows/tsan.yml?query=branch%3Amain)
[![ClusterFuzzLite](https://github.com/appwrite-labs/swoole-rs/actions/workflows/cflite_batch.yml/badge.svg?branch=main)](https://github.com/appwrite-labs/swoole-rs/actions/workflows/cflite_batch.yml?query=branch%3Amain)
[![API fuzz](https://github.com/appwrite-labs/swoole-rs/actions/workflows/api-fuzz.yml/badge.svg?branch=main)](https://github.com/appwrite-labs/swoole-rs/actions/workflows/api-fuzz.yml?query=branch%3Amain)
[![WebSocket RFC conformance](https://github.com/appwrite-labs/swoole-rs/actions/workflows/conformance.yml/badge.svg?branch=main)](https://github.com/appwrite-labs/swoole-rs/actions/workflows/conformance.yml?query=branch%3Amain)
[![Soak](https://github.com/appwrite-labs/swoole-rs/actions/workflows/soak.yml/badge.svg?branch=main)](https://github.com/appwrite-labs/swoole-rs/actions/workflows/soak.yml?query=branch%3Amain)
[![PHP versions](https://github.com/appwrite-labs/swoole-rs/actions/workflows/php-versions.yml/badge.svg?branch=main)](https://github.com/appwrite-labs/swoole-rs/actions/workflows/php-versions.yml?query=branch%3Amain)
[![Debug PHP](https://github.com/appwrite-labs/swoole-rs/actions/workflows/debug.yml/badge.svg?branch=main)](https://github.com/appwrite-labs/swoole-rs/actions/workflows/debug.yml?query=branch%3Amain)
[![Bench](https://github.com/appwrite-labs/swoole-rs/actions/workflows/bench.yml/badge.svg?branch=main)](https://github.com/appwrite-labs/swoole-rs/actions/workflows/bench.yml?query=branch%3Amain)
[![Appwrite e2e](https://github.com/appwrite-labs/swoole-rs/actions/workflows/appwrite-e2e.yml/badge.svg?branch=main)](https://github.com/appwrite-labs/swoole-rs/actions/workflows/appwrite-e2e.yml?query=branch%3Amain)

[Swoole](https://github.com/swoole/swoole-src)'s PHP extension, rewritten in Rust.
`swoole.so` is built by Cargo from the crates in `crates/`. It presents the same PHP
classes, functions and constants as upstream Swoole v6.3.0, and upstream is built only
to compare against.

This page quotes no measurements, because they go stale. The badges above are live, each
links to its details, and [Badges](#badges) says what each one means.

## Status

Experimental. It passes upstream's own tests, every release is a pre-release, and nobody
runs it in production.

What is done:

- **No upstream source is compiled.** Nothing from swoole-src's `src/`, `ext-src/` or
  `include/` is in the build, and the build needs no swoole-src checkout.
- **Some C remains, on purpose.** None of the C and C++ that swoole-src bundles from
  elsewhere is built as upstream ships it. The cURL and ssh2 hooks, `proc_open`, the
  plain-files stream wrapper and the PDO drivers for PostgreSQL, SQLite, Firebird, Oracle
  and ODBC are Rust crates, with C glue in `native/hooks` for the parts tied to PHP's
  allocator and private layouts. The rest of our own C is shims, most of them in
  `crates/zend-sys`, over the parts of PHP's C API that Rust can't call
  ([what stays C](docs/zend.md#what-stays-c)). TLS is OpenSSL, as it is upstream.
- **Upstream's PHP tests pass wherever upstream passes them.** Every merge runs the pinned
  PHPTs on both builds with their original assertions, and fails a change that fails a
  test upstream passes. The PHPT badge counts what this build passes over the tests that
  can apply ([those that can't, with reasons](corpus/upstream/not_applicable.json)).
- **It uses fewer instructions than upstream on every benchmark workload.** Each workload
  has a limit that a merge can't exceed. See [the comparison](#compared-with-upstream-swoole).

What is not:

- **No production use.** Appwrite's end-to-end suite runs against it weekly, and its badge
  shows the latest result. No Appwrite deployment uses it.
- **Memory safety is not demonstrated.** The unsafe badge counts the `unsafe` blocks, fns
  and impls left and the crates that forbid `unsafe`
  ([per crate](https://github.com/appwrite-labs/swoole-rs/blob/badges/unsafe.md)). That count
  says how much the compiler can't check, not that the rest is right. The evidence is the
  second badge row: Miri every four hours, and AddressSanitizer, ThreadSanitizer, two
  fuzzers and a soak run every night. A failed run files an issue. The Kani job proves, for inputs up to a
  small bound, that the wire decoders reject incomplete input and neither panic nor
  overflow.
- **The tests don't reach all of the Rust.** The coverage badge is line coverage of
  `crates/` under the PHPTs, the corpus apps and swoole/library's suite. The coverage build
  has no thread support, so `Swoole\Thread`'s code counts as unreached. Its own PHPTs run
  in a separate job ([per crate](https://github.com/appwrite-labs/swoole-rs/blob/badges/coverage.md)).
- **One upstream version.** It tracks one pinned release
  ([`tools/build_upstream.py`](tools/build_upstream.py) has the commit) and reports itself
  as that version.
- **Linux is what CI tests.** Every merge runs x86-64 Linux with glibc and musl, and a
  thread-safe PHP. The other supported PHP versions and arm64 run nightly, and every
  release target is built and loaded nightly. The build scripts handle macOS, which CI
  doesn't run.
- **Pre-releases only, and no packages.** [Releases](#install) are prebuilt binaries for
  Linux. There is no PECL or PIE package and no Docker image, and nothing for macOS: build
  from source there.
- **Coding agents wrote it.** AI coding agents wrote nearly all of it, working through
  GitHub issues under one maintainer's direction ([AGENTS.md](AGENTS.md)). CI is the
  reviewer: a change lands when its gates pass, without a person reading each line.

## Goals

- **A drop-in replacement.** A PHP application written for the pinned Swoole version runs
  unchanged. Upstream's tests, run with their original assertions, are the definition of
  the same behaviour.
- **Faster than upstream on every workload**, by instruction count and on the wall
  clock. The instruction counts are there; the limits keep them there.
- **Memory-safe under load, with evidence**: clean sanitizer, fuzz, soak and Miri runs,
  and an `unsafe` count that only falls.
- **Differential fuzzing of every network-facing codec** against upstream (`fuzz/`).

The work in progress is the
[open goals](https://github.com/appwrite-labs/swoole-rs/issues?q=is%3Aissue+is%3Aopen+label%3Akind%3Agoal).

Not goals: rewriting mature third-party libraries (TLS, compression, database clients),
or putting an async runtime under Swoole's reactor (`deny.toml` bans them). The
[migration approach](docs/migration.md) has the rules the work follows.

## Features

The target is the full PHP API of the pinned Swoole version. The upstream tests measure compatibility.

- **Servers.** `Swoole\Server` over TCP, UDP and Unix sockets in base, process and thread
  mode, `Swoole\Http\Server`, `Swoole\WebSocket\Server`, `Swoole\Redis\Server`, HTTP/2,
  task workers, static files, TLS and DTLS, and the coroutine
  `Swoole\Coroutine\Http\Server`.
- **Coroutines.** The scheduler, `Coroutine\Channel`, `Coroutine\System`, and
  `Swoole\Runtime` hooks for streams, sockets, sleep, cURL and PDO.
- **Clients.** `Swoole\Client`, `Coroutine\Client`, `Coroutine\Socket`,
  `Coroutine\Http\Client`, `Coroutine\Http2\Client` and the DNS resolver.
- **Processes and shared memory.** `Swoole\Process`, `Process\Pool`, `Table`, `Atomic`
  and `Lock`.
- **Timers and the event loop.** `Swoole\Timer` and `Swoole\Event`.
- **Threads.** `Swoole\Thread` on a thread-safe PHP.

Optional parts are build switches. Each of upstream's `configure` switches goes in its own
`--configure=`:

| Switch | Adds |
| --- | --- |
| `--php-sockets` | PHP sockets integration |
| `--configure=--enable-swoole-curl` | the native cURL hook |
| `--configure=--enable-swoole-pgsql`, `--configure=--enable-swoole-sqlite`, `--configure=--with-swoole-firebird` | coroutine PDO drivers for PostgreSQL, SQLite and Firebird |
| `--configure=--with-swoole-odbc=unixODBC,/usr` | the coroutine PDO ODBC driver |
| `--configure=--with-swoole-oracle=instantclient,/usr/local/instantclient` | the coroutine PDO Oracle driver |
| `--configure=--with-swoole-ssh2` | the ssh2 hook |
| `--configure=--enable-cares` | c-ares for DNS |
| `--configure=--enable-iouring`, `--configure=--enable-uring-socket` | io_uring file and socket I/O |
| `--configure=--enable-swoole-thread` | `Swoole\Thread` (needs thread-safe PHP) |
| `--fix-halfclose` | see below |

## Compared with upstream Swoole

| | Upstream Swoole | swoole-rs |
| --- | --- | --- |
| Language | C and C++ | Rust, with C and C++ glue over PHP's C API |
| Build | `phpize`, `configure`, `make` | Cargo, through `tools/build_ext.py` |
| PHP API | the reference | the same, generated from upstream's stubs (`tools/arginfo.py`) |
| Upstream PHPTs | the reference | [![Upstream PHPTs passing](https://img.shields.io/endpoint?url=https%3A%2F%2Fraw.githubusercontent.com%2Fappwrite-labs%2Fswoole-rs%2Fbadges%2Fphpt.json)](https://github.com/appwrite-labs/swoole-rs/actions/workflows/merge.yml?query=branch%3Amain) |
| Instructions per request | the reference | [![Speed vs upstream](https://img.shields.io/endpoint?url=https%3A%2F%2Fraw.githubusercontent.com%2Fappwrite-labs%2Fswoole-rs%2Fbadges%2Fspeed.json)](https://github.com/appwrite-labs/swoole-rs/blob/badges/speed.md) |
| Releases and packages | yes | prebuilt Linux binaries, as pre-releases; no packages |
| Production use | years | none |

**Speed.** The merge tier counts instructions per request under Callgrind on every
workload and fails a change that raises one past its recorded limit
([per workload](https://github.com/appwrite-labs/swoole-rs/blob/badges/speed.md)). The largest
gains are in uploads, EOF-framed TCP and large WebSocket frames. The closest to upstream
are DNS, gzip and TLS, which spend their time in the resolver, zlib and OpenSSL that both
builds share. These are instruction counts, not requests per second. The nightly Bench
run compares wall-clock CPU time per request on a few HTTP and WebSocket workloads.

**Behaviour.** No difference is intended. One is offered: upstream truncates a queued
response when the client half-closes, and a build with `--fix-halfclose` repairs that for
synchronous, non-TLS responses in process mode. Responses held by a suspended coroutine
still truncate ([open problem](docs/http-response-lifetime.md)).

**How it is checked.** Every merge runs upstream's PHPTs on both builds, the
[corpus applications](corpus/README.md) including an unchanged Hyperf skeleton, h2spec,
Autobahn and swoole/library's suite. Nightly runs add Miri, the sanitizers, fuzzing, a
soak run, other PHP versions, arm64, a debug PHP and a dry run of the release build. See the
[validation layers](docs/validation-suite.md).

## Install

Each [release](https://github.com/appwrite-labs/swoole-rs/releases) has a `swoole.so` for
PHP 8.2 to 8.5 on Linux, named
`swoole-rs-<version>-php<series>-<nts|zts>-linux-<gnu|musl>-<x86_64|aarch64>.so`:

- `nts` or `zts`: what `php -i | grep 'Thread Safety'` says (`disabled` is `nts`).
- `gnu` is glibc 2.36 or newer (Debian 12 and later) and `musl` is Alpine. Both are
  built and load-tested in the official `php` Docker images.
- PHP needs OpenSSL 3, Brotli, zstd and zlib beside it as shared libraries. The official
  images have them.

```sh
repo=appwrite-labs/swoole-rs
version=$(curl -fsSL "https://api.github.com/repos/$repo/releases?per_page=1" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p')
file=swoole-rs-$version-php8.3-nts-linux-gnu-x86_64.so
url=https://github.com/$repo/releases/download/$version
curl -fsSL -O "$url/$file" -O "$url/SHA256SUMS"
grep " $file\$" SHA256SUMS | sha256sum -c -
cp "$file" "$(php -r 'echo ini_get("extension_dir");')/swoole.so"
echo extension=swoole > "$(php --ini | sed -n 's/^Scan for additional .ini files in: //p')/swoole.ini"
php --ri swoole
```

The second line picks the newest release. The last command prints `Version`, the upstream
version the build stands in for, and `swoole-rs`, its own version and commit.
`gh attestation verify "$file" --repo appwrite-labs/swoole-rs` checks that the file was
built by this repository's release workflow.

A release binary is the default build plus the native cURL hook: TLS, HTTP/2, Brotli and
zstd are in, and `zts` files add `Swoole\Thread`. The cURL hook needs PHP's curl
extension, which the official images build in. The other switches in the
[table above](#features) are not; build from source for those. The arm64 glibc files are
built with profile-guided optimisation.

Versions are swoole-rs's own, separate from the upstream version it reports. A
`0.x` release is a pre-release: the PHP API is upstream's and stays, but which targets and
switches a release has, and how its files are named, can change between any two of them.

## Build

You need:

- Rust through `rustup`. `rust-toolchain.toml` pins the version and `rustup` installs
  it on first use.
- PHP 8.2 to 8.5 with its development headers (`php-config`).
- A C and C++ compiler, libclang, and the OpenSSL and PCRE2 development files.
  [`.devcontainer/install.sh`](.devcontainer/install.sh) is the full package list for
  Ubuntu 24.04, and the one CI installs.

```sh
python3 tools/build_ext.py
php -n -d extension="$PWD/target/ext/swoole.so" -r 'echo SWOOLE_VERSION, "\n";'
```

That prints the upstream version. Add switches from the table above, for example
`python3 tools/build_ext.py --php-sockets --configure=--enable-swoole-curl`, and
`--php-config PATH` for a PHP that isn't first on `PATH`.

Without a matching machine, build in the image CI's packages come from:

```sh
docker build -t swoole-rs-dev .devcontainer
docker run --rm -it -v "$PWD:/work" -w /work swoole-rs-dev bash
```

Don't load this `swoole.so` alongside another Swoole extension. Both register the same
classes.

## Test

The tests compare against upstream, so build it first. It needs `phpize`, `autoconf` and
`make` as well.

```sh
python3 tools/build_upstream.py
python3 corpus/run.py --engine cargo=target/ext/swoole.so --engine upstream=target/upstream/src/modules/swoole.so
python3 tools/dev.py integrate --module timer   # one module's gates on the full build
```

[Local development](docs/development.md) covers the edit and candidate loops, the PHPT
runner and `dev.py docker`. `git config core.hooksPath .githooks` turns on the pre-commit
checks.

CI has three tiers: the pre-commit hook, [PR](.github/workflows/pr.yml) (the Rust tests,
and for what a change touches: its modules' gates and PHPTs, instruction counts, build
variants, h2spec and the sanitizers) and [Merge](.github/workflows/merge.yml), which
runs everything in `main`'s merge queue before a change lands and hourly on `main`.

## Layout

- `crates/`: the Rust. `crates/ext` builds `swoole.so`, and `crates/zend` is the layer
  over PHP's C API.
- `native/`: the C glue of the hooks, and the headers the Rust exports.
- `tests/`: each module's tests, the PHP-API differential fuzzer and the tools' own tests.
- `fuzz/`: the codec fuzz targets, compared against upstream.
- `tools/`: the build (`build_ext.py`, `build_upstream.py`), `dev.py` and `ci.py`.
- [`migration/modules.json`](migration/modules.json): the modules the migration was
  done in, with their paths and tests ([fields](docs/migration-modules.md)).
- [`corpus/`](corpus/README.md): the applications and the
  [upstream PHPT selection](corpus/upstream/README.md).
- [`benchmarks/`](benchmarks/README.md): the Callgrind workloads and their limits.

## Licence

[Apache-2.0](LICENSE), as upstream Swoole. The code kept from PHP retains the
[PHP License 3.01](native/swoole-src/thirdparty/LICENSE), and the Zend Engine file retains
[Zend License 2.00](native/swoole-src/thirdparty/LICENSE-ZEND). [`NOTICE`](NOTICE) lists the attributions.

## Badges

`tools/badges.py` writes the first row after each merge-tier run on `main`.

- **Merge tier**: the newest run on `main` that finished.
- **Upstream PHPTs passing**: tests this build passes in any of a run's PHPT jobs, over
  those that can apply, with the commit measured. A run in which a job produced no result
  is skipped.
- **Speed vs upstream**: how many workloads beat upstream, and the slowest.
- **Unsafe blocks**: the count in non-test crate code and how many crates forbid `unsafe`.
  `tools/budget.py` fails a PR that raises the workspace's count or drops a crate's `forbid`.
- **Coverage**: lines of `crates/` the tests ran, measured nightly.
- **Release**: the newest release, pre-releases included.

The second row is the latest completed run of each scheduled workflow on `main`. They run
nightly, except Miri, every four hours, and Appwrite e2e, weekly. A failed run opens an
issue (`scheduled-failures.yml`).
