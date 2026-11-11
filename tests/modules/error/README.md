# Error handling

Reference: Swoole v6.2.5, commit `76c657d88be39b7af6bf85e46bcbac0da7387760`.
The error overlay replaces first-party message lookup,
error classification, unknown-message formatting and ignored-error membership
in `src/core/error.cc`.

The `swr_v1_error_message` ABI in `docs/core-abi-v1.md` and
`native/include/swoole_rs_error.h` is unchanged: 130 exact static C strings,
null on misses, no allocation, callbacks or mutable state. Numeric gaps and
upstream spelling are intentional. Module-local additive APIs and their
lifetime rules are in `native/error/contract.md`.

Rust behaviour:

- `src/core/error.cc` is not built: `swoole_strerror` and
  `swoole_is_ignored_error` are Rust under their upstream symbols. `crates/error` is
  safe Rust; `crates/error-ffi` exports it to C.
- Codes below 500 go to the OS `strerror`; known codes return
  immutable pointers; other codes are formatted without allocation.
- Unknown messages use each thread's 32-byte buffer. Apple colon spelling, NUL,
  trailing bytes and pointer reuse within a thread match native; known lookups
  are thread-safe.
- Ignored errors use a lazy process-local Rust HashSet in a Rust static:
  idempotent insert, fallible allocation and copied-on-fork state match native.
  Concurrent mutation is excluded; initialized read-only access may be
  concurrent. `swoole_ignore_error` aborts on NOMEM, as upstream's uncaught
  `bad_alloc` terminated.
- The last-error buffer (`sw_error`) is a Rust per-thread buffer, and
  `swoole_throw_error` aborts where upstream's uncaught `swoole::Exception`
  terminated. The module has no C or C++ source. No Zend values, PHP callbacks or bailout
enter Rust.

## Correctness

```sh
cargo test -p swoole-rs-error -p swoole-rs-error-ffi
cargo clippy -p swoole-rs-error -p swoole-rs-error-ffi --all-targets -- -D warnings
python3 tests/modules/error/differential.py --source PINNED_REFERENCE --archive HYBRID_ARCHIVE --sanitize --output REPORT
python3 tests/modules/error/differential.py --source PINNED_REFERENCE --archive HYBRID_ARCHIVE --allocator-faults --sanitize --output REPORT
python3 tests/modules/error/php.py --source PINNED_REFERENCE --engine upstream=CONTROL --engine hybrid=CANDIDATE --output REPORT
```

`differential.py` compiles the complete pinned `error.cc` (from Git objects)
against the archive's Rust error facade, with that build's headers (`HYBRID_ARCHIVE` is
`target/ext/sdk/lib/libswoole_rs_ffi.a`, from `tools/build_ext.py --native`);
only dependency declarations and external buffer storage are stubbed. It checks
all 130 messages, 10414 numeric inputs, the original state assertions, the
throw's abort (in a child), the per-thread last-error buffer, concurrent lookups and reads, 20000 state operations, duplicates,
signed values, fork isolation and child destruction, plus full callback and
32-byte unknown-buffer assertions.

`--allocator-faults` links the unchanged Rust module with a test global
allocator: bucket, handle and growth allocation failures must return NOMEM and
preserve membership, `swoole_ignore_error` must abort on one (in a child); duplicates must not allocate and retries must succeed. It links that
fixture in place of the archive and rejects benchmark preparation.

`php.py` compares engines on 31221 `swoole_strerror` results, runs the original
`swoole_strerror.phpt` and `swoole_error_log.phpt` through PHP's official runner,
and drives 8200 ignored-error insertions, signed/narrowed integers, fork
before/after initialization, child isolation and shutdown logging (`state.php`).
It also runs the 43 integers in `php-narrowing-cases.json` through omitted,
literal-zero, SYSTEM and SWOOLE selectors. It requires pcntl and 64-bit PHP, and
requires both native-error and Rust-error feature identities so two native
builds cannot satisfy parity. Compare bytes within one platform: OS messages
differ across platforms.

`--sanitize` instruments C++ only, not the Rust archive.

`error-native` (`differential.py` on the hybrid build's archive) and `error-php`
run these.

## Benchmarks

Prepare (correctness checks, optimized consumers, state `--check` and
`--check-latency` with clocks disabled; the destination must not exist):

```sh
python3 tests/modules/error/differential.py --source PINNED_REFERENCE --archive PUBLISHED_ARCHIVE --prepare-benchmark NEW_DIRECTORY --output REPORT
```

Measure, without concurrent workloads:

```sh
python3 tests/modules/error/benchmark.py --prepared PREPARED --output MESSAGE_REPORT
python3 tests/modules/error/benchmark.py --prepared PREPARED --state --output STATE_REPORT
python3 tests/modules/error/benchmark.py --prepared PREPARED --state --latency --output STATE_LATENCY_REPORT
```

The runner verifies the prepared binary hashes, runs seven alternating pairs
and kills its private process group on timeout. Message workloads: all known
codes, hot code, system errors and unknown errors. State workloads: mixed-sign
hit/miss/duplicate at 8 and 4096 entries, fresh insertion at 4096 and 65536
entries, and four-thread read-only hit/miss at 4096 entries; each sample forks
from the same empty parent. `--latency` reports p50/p95/p99 of batch-mean
nanoseconds per operation (about 128 batches), not per-call tail latency.
