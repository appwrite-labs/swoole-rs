# PHP collection facades (M27)

M27 covers the `Swoole\Table` and coroutine `Channel` PHP classes. Both are Rust
over the zend crates (docs/zend.md, `crates/phpcollections`): C++ keeps their
arginfo method tables and lends the coroutine module's queue as a table of
operations. Table runs over the memory crate's table, called from Rust; C++
lends its shared allocation, Zend's string hash and PHP's error reports.

Edit both overlays in `migration/patches/phpcollections/` with `tools/patches.py`
(see [hybrid build](../../../docs/hybrid-build.md#editing-patches)).

## Loaded PHP runtime (`php_runtime.py`)

```sh
python3 tests/modules/phpcollections/php_runtime.py \
  --engine native=/absolute/reference.so --engine selected=/absolute/swoole.so \
  --build-manifest /absolute/build-manifest.json --archive /absolute/ffi.a \
  --source-ref EXACT_SOURCE_COMMIT --output target/runtime.json
```

Copies the DSOs, requires M27 bit 33554432 on exactly one engine, attests the
selected build, then runs `php_collections.php` on both. The `complete` profile
requires exactly 324 checks and identical summaries: binary/null/partial writes,
defined arithmetic, object/resource custody, COW isolation, suspended FIFO
transfer, aliases/Countable/Iterator, constructor refusal, serialization denial,
and column schemas of 1/3/17/65 columns with duplicates. `--profile column-bytes`
runs the separate 16-check `php_column_bytes.php` (NUL/high-byte column names).
Optional PHP dependencies use repeated `--php-extension NAME`. Registered as
`phpcollections-runtime` and `phpcollections-column-bytes`.

## Originals

The PHPT gate (merge tier `phpt-*` jobs) runs every pinned Table, Channel and
`swoole_global` original on upstream and on the all-modules hybrid.

## Benchmark preparation (`prepare_benchmark.py`)

Takes the runtime pair arguments plus `--native-build-manifest FILE`. Optional
paired `--selected-export FILE` / `--native-export FILE` recompute the observed
compiler commands and reject unoptimized builds; `--compilation-correction FILE`
must match exactly one export. It runs seven `benchmark.php` workloads with
`--check` and `hrtime`/`microtime` disabled (new/existing writes, full/field
reads, arithmetic, buffered transfer, handoff), makes the prepared files
read-only and reports remaining eligibility blockers. No measurement mode has run.

## Unit tests

```sh
for t in tests/modules/phpcollections/test_*.py; do python3 -m unittest "$t"; done
```

## Gate audit (#490)

Kept (2):

- `phpcollections-runtime` and `phpcollections-column-bytes`: 324 and 16 loaded-PHP
  checks whose summaries must match upstream exactly. They cover COW isolation,
  object and resource custody, suspended FIFO transfer and binary column names,
  which the PHPTs only pass or fail.

Deleted (3):

- `phpcollections-policy` traced the extracted Table bodies against scripted
  effect doubles; the bodies are Rust now and the runtime checks and PHPTs
  cover what PHP sees.

Each a duplicate:

- `phpcollections-global-handlers` reran `phpcollections-runtime`, then three
  `swoole_global` PHPTs that the PHPT gate already runs on both engines.
- `phpcollections-prerequisites` reran the 63 Table/Channel PHPTs that the PHPT
  gate already runs, with Redis services. It ran only under `--redis-extension`,
  which no CI lane passed, so that option and `--redis-image` are gone too.

