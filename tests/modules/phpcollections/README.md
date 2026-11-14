# PHP collection tests

These cover the `Swoole\Table` and coroutine `Channel` PHP classes, which are
Rust (`crates/phpcollections`, over the zend crates; see docs/zend.md). Table
runs over the memory crate's table.

## Loaded PHP runtime (`php_runtime.py`)

```sh
python3 tests/modules/phpcollections/php_runtime.py \
  --engine upstream=/absolute/reference.so --engine cargo=target/ext/swoole.so \
  --build-manifest target/ext/build-manifest.json --output target/runtime.json
```

Copies the DSOs, requires the phpcollections feature bit on exactly one engine, checks the
selected engine against `tools/build_ext.py`'s manifest, then runs `php_collections.php` on both. The `complete` profile
requires exactly 330 checks and identical summaries: binary/null/partial writes,
defined arithmetic, object/resource custody, COW isolation, suspended FIFO
transfer, aliases/Countable/Iterator, a column whose name creates the table, a set() whose conversion bails out (read back at shutdown), constructor refusal, serialization denial,
and column schemas of 1/3/17/65 columns with duplicates. `--profile column-bytes`
runs the separate 16-check `php_column_bytes.php` (NUL/high-byte column names).
Optional PHP dependencies use repeated `--php-extension NAME`. Registered as
`phpcollections-runtime` and `phpcollections-column-bytes`.

## Originals

The PHPT gate (merge tier `phpt-*` jobs) runs every pinned Table, Channel and
`swoole_global` original on upstream and on the Cargo build.

## Unit tests

```sh
for t in tests/modules/phpcollections/test_*.py; do python3 -m unittest "$t"; done
```
