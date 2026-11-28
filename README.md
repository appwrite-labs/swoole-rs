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
classes, functions and constants as upstream Swoole v6.2.5, and upstream is built only
to compare against.

The numbers on this page were measured on 2026-11-11 at `9f077d8`. The badges above are live.

## Status

Experimental. It passes upstream's own tests and nobody runs it in production.

What is done:

- **No upstream source is compiled.** Nothing from swoole-src's `src/`, `ext-src/` or
  `include/` is in the build, and the build needs no swoole-src checkout. 67 crates hold
  about 209,000 lines of Rust.
- **Some C remains, on purpose.** 38,000 lines of C that swoole-src itself bundles from
  elsewhere (PHP's PDO drivers and PECL ssh2, adapted to coroutine hooks) live in
  `native/swoole-src/thirdparty`, plus 216 lines of our own glue. TLS is OpenSSL, as it is
  upstream.
- **Upstream's PHP tests pass wherever upstream passes them.** Of 1,866 pinned PHPTs, 55
  can't run here: upstream skips them unconditionally, or they need an older PHP or an
  outside network ([the list, with reasons](corpus/upstream/not_applicable.json)). Of the
  other 1,811 this build passes 1,745 and upstream passes 1,742 in the same CI runs. No
  test passes on upstream and fails here. The 66 that neither build passes are 38 the
  runner sets aside because something outside the extension decides them (a race in the
  test, the network, a missing tool), and 28 that fail or skip on both.
- **It is faster than upstream on 31 of 34 benchmark workloads**, by instruction count.
  See [the comparison](#compared-with-upstream-swoole).

What is not:

- **No production use.** Appwrite's end-to-end suite runs against it in CI
  (`appwrite-e2e.yml`). On 2026-10-27 an earlier build, still part C++, gave the same
  results as stock Swoole (7,260 tests,
  [#90](https://github.com/appwrite-labs/swoole-rs/issues/90)). The two scheduled runs
  since failed, and a run by hand on 2026-11-11 passed. No Appwrite deployment uses it.
- **Memory safety is not demonstrated.** The code has 5,699 `unsafe` blocks, fns and
  impls, and 17 of 67 crates forbid `unsafe`
  ([per crate](https://github.com/appwrite-labs/swoole-rs/blob/badges/unsafe.md)). Much of
  it is a close port of C++ that still passes raw pointers between crates. The
  AddressSanitizer run fails on `main`, and ThreadSanitizer has open reports in thread
  mode ([#3342](https://github.com/appwrite-labs/swoole-rs/issues/3342),
  [#3343](https://github.com/appwrite-labs/swoole-rs/issues/3343)). Miri, the fuzzers
  and the soak run pass.
- **The tests reach 74% of the Rust.** That is line coverage of `crates/` under the PHPTs,
  the corpus apps and swoole/library's suite. The coverage build has no thread support, so
  `Swoole\Thread`'s code counts as unreached; its own PHPTs run in a separate job
  ([per crate](https://github.com/appwrite-labs/swoole-rs/blob/badges/coverage.md)).
- **One upstream version.** It tracks v6.2.5 (pinned at `76c657d8`) and reports itself as
  that version.
- **Linux is what CI tests.** Every merge runs Ubuntu 24.04 on x86-64 with PHP 8.3, plus
  Alpine (musl) and a thread-safe PHP. PHP 8.2, 8.4 and 8.5 run nightly. The build
  scripts handle macOS and arm64 Linux, and CI runs neither.
- **No release, no packages, no licence file.** There is no tag and no PECL or PIE
  package; you build it from source. The repository root has no `LICENSE` yet. Upstream
  Swoole is Apache-2.0 and the bundled PHP code is under the PHP License 3.01.
- **Coding agents wrote it.** About 2,100 commits since 2026-10-25 came from AI coding
  agents working through GitHub issues under one maintainer's direction
  ([AGENTS.md](AGENTS.md)). CI is the reviewer: a change lands when its gates pass,
  without a person reading each line.

## Goals

- **A drop-in replacement.** A PHP application written for Swoole v6.2.5 runs unchanged.
  Upstream's tests, run with their original assertions, are the definition of the same
  behaviour.
- **Faster than upstream on every workload**
  ([#1894](https://github.com/appwrite-labs/swoole-rs/issues/1894)). Three of 34 are not
  there yet.
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

The target is all of upstream's PHP API at v6.2.5, and nothing is knowingly missing. How
well each part works is what the tests above show.

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

| | Upstream Swoole v6.2.5 | swoole-rs |
| --- | --- | --- |
| Language | C and C++ | Rust, plus the 38,000 lines of C that upstream bundles |
| Build | `phpize`, `configure`, `make` | Cargo, through `tools/build_ext.py` |
| PHP API | the reference | the same, generated from upstream's stubs (`tools/arginfo.py`) |
| Upstream PHPTs passing, of 1,811 | 1,742 | 1,745 |
| Instructions per request | 1.00× | 0.90× geometric mean over 34 workloads |
| Releases and packages | yes | none |
| Production use | years | none |

**Speed.** The merge tier counts instructions per request under Callgrind on 34 workloads
and fails a change that raises one more than 2% above its record
([results](https://github.com/appwrite-labs/swoole-rs/blob/badges/speed.md)). The largest
gains are uploads (0.41×), EOF-framed TCP (0.54×) and timers (0.64×). Plain HTTP is 0.92×.
Three workloads are not faster: length-framed TCP (1.04×), HTTPS (1.0005×) and UDP
(1.0003×). Workloads that spend their time in OpenSSL or zlib sit at 1.00×, as expected.
These are instruction counts, not requests per second. The one wall-clock comparison so
far, Appwrite's endpoints in October on an earlier build, was within 1% of upstream.

**Behaviour.** No difference is intended. One is offered: upstream truncates a queued
response when the client half-closes, and a build with `--fix-halfclose` repairs that for
synchronous, non-TLS responses in process mode. Responses held by a suspended coroutine
still truncate ([open problem](docs/http-response-lifetime.md)).

**How it is checked.** Every merge runs upstream's PHPTs on both builds, twelve
[corpus applications](corpus/README.md) including an unchanged Hyperf skeleton, h2spec,
Autobahn and swoole/library's suite. Nightly runs add Miri, the sanitizers, fuzzing, a
soak run, other PHP versions and a debug PHP. See the
[validation layers](docs/validation-suite.md).

## Build

You need:

- Rust through `rustup`. `rust-toolchain.toml` pins 1.99.0 and `rustup` installs it on
  first use.
- PHP 8.2 to 8.5 with its development headers (`php-config`).
- A C and C++ compiler, libclang, and the OpenSSL and PCRE2 development files.
  [`.devcontainer/install.sh`](.devcontainer/install.sh) is the full package list for
  Ubuntu 24.04, and the one CI installs.

```sh
python3 tools/build_ext.py
php -n -d extension="$PWD/target/ext/swoole.so" -r 'echo SWOOLE_VERSION, "\n";'
```

That prints `6.2.5`. Add switches from the table above, for example
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

## Badges

`tools/badges.py` writes the first row after each merge-tier run on `main`.

- **Merge tier**: the newest run on `main` that finished.
- **Upstream PHPTs passing**: tests this build passes in any of a run's PHPT jobs, over
  those that can apply, with the commit measured. A run in which a job produced no result
  is skipped.
- **Speed vs upstream**: how many workloads beat upstream, and the slowest.
- **Unsafe blocks**: the count in non-test crate code and how many crates forbid `unsafe`.
  `tools/budget.py` fails a PR that raises a crate's count or drops its `forbid`.
- **Coverage**: lines of `crates/` the tests ran, measured nightly.

The second row is the latest completed run of each nightly workflow on `main`.
