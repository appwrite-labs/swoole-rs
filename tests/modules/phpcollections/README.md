# PHP collection tests

These cover the `Swoole\Table` and coroutine `Channel` PHP classes, which are
Rust (`crates/phpcollections`, over the zend crates; see docs/zend.md). Table
runs over the memory crate's table.

## Loaded PHP runtime (`php_runtime.py`)

```sh
python3 tests/modules/phpcollections/php_runtime.py \
  --engine native=/absolute/reference.so --engine selected=/absolute/swoole.so \
  --build-manifest /absolute/build-manifest.json --archive /absolute/ffi.a \
  --source-ref EXACT_SOURCE_COMMIT --output target/runtime.json
```

Copies the DSOs, requires the phpcollections feature bit on exactly one engine, attests the
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
