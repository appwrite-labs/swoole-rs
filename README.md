# swoole-rs

[![Merge tier](https://img.shields.io/endpoint?url=https%3A%2F%2Fraw.githubusercontent.com%2Fappwrite-labs%2Fswoole-rs%2Fbadges%2Fmerge.json)](https://github.com/appwrite-labs/swoole-rs/actions/workflows/merge.yml?query=branch%3Amain)
[![Upstream PHPTs passing](https://img.shields.io/endpoint?url=https%3A%2F%2Fraw.githubusercontent.com%2Fappwrite-labs%2Fswoole-rs%2Fbadges%2Fphpt.json)](https://github.com/appwrite-labs/swoole-rs/issues/58)
[![Speed vs upstream](https://img.shields.io/endpoint?url=https%3A%2F%2Fraw.githubusercontent.com%2Fappwrite-labs%2Fswoole-rs%2Fbadges%2Fspeed.json)](https://github.com/appwrite-labs/swoole-rs/blob/badges/speed.md)
[![Unsafe blocks](https://img.shields.io/endpoint?url=https%3A%2F%2Fraw.githubusercontent.com%2Fappwrite-labs%2Fswoole-rs%2Fbadges%2Funsafe.json)](https://github.com/appwrite-labs/swoole-rs/blob/badges/unsafe.md)
[![crates/ line coverage](https://img.shields.io/endpoint?url=https%3A%2F%2Fraw.githubusercontent.com%2Fappwrite-labs%2Fswoole-rs%2Fbadges%2Fcoverage.json)](https://github.com/appwrite-labs/swoole-rs/blob/badges/coverage.md)

[![Miri](https://github.com/appwrite-labs/swoole-rs/actions/workflows/miri.yml/badge.svg?branch=main)](https://github.com/appwrite-labs/swoole-rs/actions/workflows/miri.yml?query=branch%3Amain)
[![AddressSanitizer](https://github.com/appwrite-labs/swoole-rs/actions/workflows/asan.yml/badge.svg?branch=main)](https://github.com/appwrite-labs/swoole-rs/actions/workflows/asan.yml?query=branch%3Amain)
[![ThreadSanitizer](https://github.com/appwrite-labs/swoole-rs/actions/workflows/tsan.yml/badge.svg?branch=main)](https://github.com/appwrite-labs/swoole-rs/actions/workflows/tsan.yml?query=branch%3Amain)
[![ClusterFuzzLite](https://github.com/appwrite-labs/swoole-rs/actions/workflows/cflite_batch.yml/badge.svg?branch=main)](https://github.com/appwrite-labs/swoole-rs/actions/workflows/cflite_batch.yml?query=branch%3Amain)
[![API fuzz](https://github.com/appwrite-labs/swoole-rs/actions/workflows/api-fuzz.yml/badge.svg?branch=main)](https://github.com/appwrite-labs/swoole-rs/actions/workflows/api-fuzz.yml?query=branch%3Amain)
[![WebSocket RFC conformance](https://github.com/appwrite-labs/swoole-rs/actions/workflows/conformance.yml/badge.svg?branch=main)](https://github.com/appwrite-labs/swoole-rs/actions/workflows/conformance.yml?query=branch%3Amain)
[![Soak](https://github.com/appwrite-labs/swoole-rs/actions/workflows/soak.yml/badge.svg?branch=main)](https://github.com/appwrite-labs/swoole-rs/actions/workflows/soak.yml?query=branch%3Amain)
[![PHP versions](https://github.com/appwrite-labs/swoole-rs/actions/workflows/php-versions.yml/badge.svg?branch=main)](https://github.com/appwrite-labs/swoole-rs/actions/workflows/php-versions.yml?query=branch%3Amain)
[![Debug PHP](https://github.com/appwrite-labs/swoole-rs/actions/workflows/debug.yml/badge.svg?branch=main)](https://github.com/appwrite-labs/swoole-rs/actions/workflows/debug.yml?query=branch%3Amain)

[Swoole](https://github.com/swoole/swoole-src)'s PHP extension, rewritten in Rust.
`swoole.so` is built by Cargo from the crates in `crates/`. It presents the same PHP
classes, functions and constants as upstream Swoole v6.2.6, and upstream is built only
to compare against.

The numbers on this page were measured on 2026-11-16: the code counts at `d577a1d`, the
test and speed results in the merge-tier run on `94a6fef`. The badges above are live.

## Status

Experimental. It passes upstream's own tests, [v0.1.0](#install) is its first
pre-release, and nobody runs it in production.

What is done:

- **No upstream source is compiled.** Nothing from swoole-src's `src/`, `ext-src/` or
  `include/` is in the build, and the build needs no swoole-src checkout. 67 crates hold
  about 209,000 lines of Rust.
- **Some C remains, on purpose.** 42,000 lines of C and C++ that swoole-src itself bundles
  from elsewhere (PHP's PDO drivers, curl and `proc_open`, and PECL ssh2, adapted to
  coroutine hooks) live in `native/swoole-src/thirdparty`. Our own C is about 4,100 lines
  of shims under `crates/`, nearly all in `crates/zend-sys` over the parts of PHP's C API
  that Rust can't call ([what stays C](docs/zend.md#what-stays-c)), and 225 lines in
  `native/`. TLS is OpenSSL, as it is upstream.
- **The v6.2.6 test inventory contains 1,897 original PHPTs.** CI runs the same
  assertions against both builds. The remaining compatibility fixes are tracked in
  [the v6.2.6 migration](https://github.com/appwrite-labs/swoole-rs/issues/4926).
  The last v6.2.5 comparison is shown [below](#compared-with-upstream-swoole).
- **On v6.2.5, the Rust build used fewer instructions on all 34 benchmark workloads.**
  See [the comparison](#compared-with-upstream-swoole).

What is not:

- **No production use.** Appwrite's end-to-end suite runs against it in CI
  (`appwrite-e2e.yml`). On 2026-10-27 an earlier build, still part C++, gave the same
  results as stock Swoole (7,260 tests,
  [#90](https://github.com/appwrite-labs/swoole-rs/issues/90)). Of the three weekly runs
  since, two failed and the latest, on 2026-11-16, passed. No Appwrite deployment uses it.
- **Memory safety is not demonstrated.** The code has 476 `unsafe` blocks, fns and
  impls, down from 5,699 on 2026-11-11, and 19 of 67 crates forbid `unsafe`
  ([per crate](https://github.com/appwrite-labs/swoole-rs/blob/badges/unsafe.md)). Half of
  what is left sits in five crates: the layer over PHP's C API, the server, the core
  state, coroutines and the runtime hooks. The count says how much the compiler can't
  check, not that the rest is right: the nightly AddressSanitizer run failed on three of the five nights to 2026-11-16, and
  ThreadSanitizer on one; its thread-mode reports
  ([#3342](https://github.com/appwrite-labs/swoole-rs/issues/3342),
  [#3343](https://github.com/appwrite-labs/swoole-rs/issues/3343)) were fixed on
  2026-11-12. Miri, the API fuzzer and the soak run passed all five.
- **The tests reach 76% of the Rust.** That is line coverage of `crates/` under the PHPTs,
  the corpus apps and swoole/library's suite. The coverage build has no thread support, so
  `Swoole\Thread`'s code counts as unreached; its own PHPTs run in a separate job
  ([per crate](https://github.com/appwrite-labs/swoole-rs/blob/badges/coverage.md)).
- **One upstream version.** It tracks v6.2.6 (pinned at `13d2f336`) and reports itself as
  that version.
- **Linux is what CI tests.** Every merge runs Ubuntu 24.04 on x86-64 with PHP 8.3, plus
  Alpine (musl) and a thread-safe PHP. PHP 8.2, 8.4 and 8.5 run nightly, as does
  arm64 with PHP 8.3. The build scripts handle macOS, which CI doesn't run.
- **Pre-releases only, and no packages.** [Releases](#install) are prebuilt binaries for
  Linux. There is no PECL or PIE package and no Docker image, and nothing for macOS: build
  from source there.
- **Coding agents wrote it.** About 3,000 commits since 2026-10-25 came from AI coding
  agents working through GitHub issues under one maintainer's direction
  ([AGENTS.md](AGENTS.md)). CI is the reviewer: a change lands when its gates pass,
  without a person reading each line.

## Goals

- **A drop-in replacement.** A PHP application written for Swoole v6.2.6 runs unchanged.
  Upstream's tests, run with their original assertions, are the definition of the same
  behaviour.
- **Faster than upstream on every workload**
  ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)). CI measures each workload against its own limit.
- **Memory-safe under load, with evidence**
  ([#1893](https://github.com/appwrite-labs/swoole-rs/issues/1893)): clean sanitizer,
  fuzz, soak and Miri runs, and an `unsafe` count that only falls. Replacing the C ABI
  between crates with typed Rust handles
  ([#2212](https://github.com/appwrite-labs/swoole-rs/issues/2212)) is most of that work.
- **Differential fuzzing of every network-facing codec** against upstream
  ([#100](https://github.com/appwrite-labs/swoole-rs/issues/100)).

Not goals: rewriting mature third-party libraries (TLS, compression, database clients),
or putting an async runtime under Swoole's reactor (`deny.toml` bans them). The
[migration approach](docs/migration.md) has the rules the work follows.

## Features

The target is the full PHP API of Swoole v6.2.6. The upstream tests measure compatibility.

- **Servers.** `Swoole\Server` over TCP, UDP and Unix sockets in base, process and thread
  mode, `Swoole\Http\Server`, `Swoole\WebSocket\Server`, HTTP/2, task workers, static
  files, TLS and DTLS, and the coroutine `Swoole\Coroutine\Http\Server`.
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
| `--stdext` | upstream's experimental stdext |
| `--fix-halfclose` | see below |

## Compared with upstream Swoole

| | Upstream Swoole | swoole-rs |
| --- | --- | --- |
| Language | C and C++ | Rust, plus the 42,000 lines of C and C++ that upstream bundles |
| Build | `phpize`, `configure`, `make` | Cargo, through `tools/build_ext.py` |
| PHP API | the reference | the same, generated from upstream's stubs (`tools/arginfo.py`) |
| v6.2.5 PHPTs passing, of 1,811 | 1,742 | 1,746 |
| Instructions per request (v6.2.5) | 1.00× | 0.85× geometric mean over 34 workloads |
| Releases and packages | yes | prebuilt Linux binaries, as pre-releases; no packages |
| Production use | years | none |

**Speed.** The merge tier counts instructions per request under Callgrind on 34 workloads
and fails a change that raises one more than 2% above its record
([results](https://github.com/appwrite-labs/swoole-rs/blob/badges/speed.md)). The largest
gains are uploads (0.31×), EOF-framed TCP (0.50×) and 64 KiB WebSocket frames (0.56×). Plain
HTTP is 0.90×. The closest are gzip, compressed WebSocket and DNS at 0.98×, and HTTPS at
0.97×: they spend their time in zlib, the resolver and OpenSSL, which both builds share.
These are instruction counts, not requests per second. The one wall-clock comparison so
far, Appwrite's endpoints in October on an earlier build, was within 1% of upstream.

**Behaviour.** No difference is intended. One is offered: upstream truncates a queued
response when the client half-closes, and a build with `--fix-halfclose` repairs that for
synchronous, non-TLS responses in process mode. Responses held by a suspended coroutine
still truncate ([open problem](docs/http-response-lifetime.md)).

**How it is checked.** Every merge runs upstream's PHPTs on both builds, twelve
[corpus applications](corpus/README.md) including an unchanged Hyperf skeleton, h2spec,
Autobahn and swoole/library's suite. Nightly runs add Miri, the sanitizers, fuzzing, a
soak run, other PHP versions, arm64 and a debug PHP. See the
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
file=swoole-rs-v0.1.1-php8.3-nts-linux-gnu-x86_64.so
url=https://github.com/appwrite-labs/swoole-rs/releases/download/v0.1.1
curl -fsSL -O "$url/$file" -O "$url/SHA256SUMS"
grep " $file\$" SHA256SUMS | sha256sum -c -
cp "$file" "$(php -r 'echo ini_get("extension_dir");')/swoole.so"
echo extension=swoole > "$(php --ini | sed -n 's/^Scan for additional .ini files in: //p')/swoole.ini"
php --ri swoole
```

For the v0.1.1 binary above, the last command prints `Version => 6.2.5`, the upstream version it stands in
for, and `swoole-rs => 0.1.1 (<commit>)`. `gh attestation verify "$file" --repo
appwrite-labs/swoole-rs` checks that the file was built by this repository's release
workflow.

A release binary is the default build: TLS, HTTP/2, Brotli and zstd are in, `zts` files
add `Swoole\Thread`, and the switches in the [table above](#features) are not. Build from
source for those.

Versions are swoole-rs's own (`0.1.0`), separate from the upstream version it reports. A
`0.x` release is a pre-release: the PHP API is upstream's and stays, but which targets and
switches a release has, and how its files are named, can change between any two of them.

## Build

You need:

- Rust through `rustup`. `rust-toolchain.toml` pins 1.100.0 and `rustup` installs it on
  first use.
- PHP 8.2 to 8.5 with its development headers (`php-config`).
- A C and C++ compiler, libclang, and the OpenSSL and PCRE2 development files.
  [`.devcontainer/install.sh`](.devcontainer/install.sh) is the full package list for
  Ubuntu 24.04, and the one CI installs.

```sh
python3 tools/build_ext.py
php -n -d extension="$PWD/target/ext/swoole.so" -r 'echo SWOOLE_VERSION, "\n";'
```

That prints `6.2.6`. Add switches from the table above, for example
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
the gates and PHPTs of the modules a change touches, and instruction counts) and
[Merge](.github/workflows/merge.yml), which runs everything in `main`'s merge queue
before a change lands.

## Layout

- `crates/`: the Rust. `crates/ext` builds `swoole.so`, and `crates/zend` is the layer
  over PHP's C API.
- `native/`: the bundled C and the headers the Rust exports.
- [`migration/modules.json`](migration/modules.json): the 30 modules the migration was
  done in, with their paths and tests ([fields](docs/migration-modules.md)).
- [`corpus/`](corpus/README.md): the applications and the
  [upstream PHPT selection](corpus/upstream/README.md).
- [`benchmarks/`](benchmarks/README.md): the Callgrind workloads and their limits.

## Licence

[Apache-2.0](LICENSE), as upstream Swoole. Bundled PHP code retains the
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

The second row is the latest completed run of each nightly workflow on `main`.
