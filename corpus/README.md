# Compatibility corpus

Small, unchanged PHP projects progress from a single callback to application
state, persistence, coroutine coordination, WebSocket lifecycles, framed TCP, timers, shared memory, and task workers. Projects 1–11 are locally authored fixtures. Project 12 is an unchanged pinned
Hyperf skeleton with external HTTP and container probes; its exact scope is
documented in thirdparty/README.md.
No application branches on the extension under test. Python's standard library
runs external clients against real PHP processes.

| Level | Project | E2E assertions |
| --- | --- | --- |
| 1 | Hello | Status, content type, body, reuse of the same keep-alive socket |
| 2 | Router | Routing, 404, nested query arrays, headers, 64 KiB binary echo, concurrent clients |
| 3 | Notes API | Authentication, validation, create/list/read/delete, 405/404, state between requests |
| 4 | Job service | Multi-file app, SQLite, authentication, state transitions, metrics, persistence across full process restart |
| 5 | Coroutines | Four producers, channel capacity two, fan-in, sleep, pop timeout |
| 6 | WebSocket room | Verified upgrade, masked client frames, two-client broadcast, closing handshake, surviving client |
| 7 | Framed TCP | Binary length-prefixed messages, split writes, coalesced frames and concurrent clients |
| 8 | Timers | Repetition, cancellation, existence checks and nested one-shot |
| 9 | Shared Table | Typed rows, forked child updates, atomic increment, deletion and process completion |
| 10 | Task workers | HTTP validation and computation in a separate task worker |
| 11 | Buffer lifecycle | Slow reads, stream/sendfile cancellation, half-close, churn, pending-write shutdown |
| 12 | Pinned Hyperf skeleton | Unchanged app startup/routing/parsing/shutdown; external coroutine I/O and persistence probes |

```sh
# Build the pinned upstream reference without installing it globally:
python3 tools/build_upstream.py
python3 tools/build_ext.py
python3 corpus/run.py \
  --engine cargo=target/ext/swoole.so \
  --engine upstream=target/upstream/src/modules/swoole.so
```

The upstream build needs Git, a C/C++ compiler, make, autoconf, phpize, php-config,
pkg-config, PCRE2 and OpenSSL development files. On Ubuntu:
`sudo apt-get install php-dev php-sqlite3 libpcre2-dev libssl-dev pkg-config`.
On macOS the script discovers Homebrew OpenSSL and PCRE2 headers. Override
`--phpize`, `--php-config`, and `--openssl-dir` for another toolchain. The runner's
`--php` must match the PHP version used to compile both extensions.

The runner uses `php -n` to avoid accidentally loading another Swoole extension.
On Linux distributions with dynamic PDO modules, add
`--php-extension pdo --php-extension pdo_sqlite --php-extension curl
--php-extension sockets --php-extension tokenizer` to corpus, PHPT and benchmark commands. Install the
matching PHP curl and sockets packages first where separate. These explicit modules are loaded identically for every engine and
recorded in report metadata. Homebrew PHP builds them in and needs no extra flags; do not add dynamic
module flags for those built-in modules.

The reference is **v6.2.5**, commit
`76c657d88be39b7af6bf85e46bcbac0da7387760`.
`tools/build_upstream.py` verifies the checkout's commit and writes
`target/upstream/build.log`; it never runs `make install` or changes php.ini.

`manifest.json` names each project's entry point, required capabilities, and E2E
test. Capabilities are probed through PHP's actual classes/functions/extensions.
An absent API is `unsupported`; once its symbols exist, its behavioral test runs
and any regression is `failed`. Presence is not evidence of compatibility.

- Default mode fails on behavioral failures and prints unsupported cases explicitly.
- `--require-all` also fails on unsupported projects. CI uses this for upstream.
- `--project 04-job-service` selects a project; repeat to select several.
- `--output PATH` selects the JSON report, default `target/reports/corpus.json`.

The report includes extension checksums, PHP/Swoole versions, capabilities,
per-project outcomes, reasons, and durations. Each server runs in its own process
group, data directory, and port. Cleanup terminates the full group, including
upstream workers. The restart test explicitly reuses only its database directory.

## Syscalls

`syscalls.py` (the `syscalls` phase on Linux, in the integrate lane and when the reactor
changes) runs projects 1–10 under `strace -f` on both engines, twice each, and compares
what the calls carry: socket options, socket and accept flags, `fcntl` and `ioctl` modes,
epoll registrations, send and receive flags, clone flags, signal dispositions, and each
result's errno. Fds, addresses, ports, lengths and buffers are dropped. A signature every
run of one engine made and no run of the other did is a difference; one only some runs
make is timing, reported as unstable and not gated. Each difference needs a
`syscalls.json` entry (`engine`, `signature` and optional `project` patterns, `reason`),
or a `kind:bug` issue that the entry's reason names. `target/reports/syscalls.json` holds
every difference and each engine's call counts.

## Original Swoole PHPT tests

The [upstream selection](upstream/README.md) runs original Swoole PHPTs through
PHP's official runner, with source checksums and unchanged assertions; the merge
tier runs every pinned PHPT.

## swoole/library's PHPUnit suite

`library/run.py` runs the unit tests of swoole/library, the PHP code the extension
embeds, at the commit the pin embeds, against each engine's embedded copy. Start
the services of the library's `docker-compose.yml` with its
`docker-compose.host.yml` and map their names to 127.0.0.1, as the merge tier's
`library` job does, then:

```sh
python3 corpus/library/run.py --prepare
python3 corpus/library/run.py --engine upstream=target/upstream/src/modules/swoole.so \
  --engine cargo=target/ext/swoole.so
```

A test passing on the first engine and failing on another fails the run.
`composer.json` is the library's `composer-embedded.json` with PHP pinned to the
runners' 8.3, so `composer.lock` resolves the same everywhere.

Prepare the Hyperf application before running project 12:
`python3 corpus/thirdparty/hyperf.py --prepare`. Source and Composer dependency
identities are pinned; runtime tests do not resolve dependencies. Project 11
needs a `--fix-halfclose` build: upstream truncates queued responses after a
client half-close.
