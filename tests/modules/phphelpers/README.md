# M26 common PHP helpers and optional stdext

M26 covers `ext-src/php_swoole_cxx.cc`, `ext-src/swoole_stdext.cc` and the
String/KeyValue/ArrayIterator/Array/Variable/CharPtr/inline Callable methods and
callable factories in `ext-src/php_swoole_cxx.h`. Public declarations, layout and
macros stay frozen. The contract is
[docs/phphelpers-abi-v1.md](../../../docs/phphelpers-abi-v1.md).

`ext-src/swoole_stdext.cc` (typed arrays, methods on arrays/strings/streams and
the `swoole_*` stdext functions) is Rust in `crates/phpstdext` over the zend
crates (docs/zend.md); only the `ASSIGN_DIM`/`ASSIGN_DIM_OP` handlers (PHP's
`zend_execute.c` internals) and the PCRE/JSON library calls stay C++.

For the rest, Rust owns copied-scalar policy. Native Zend storage,
HashTables/COW/GC, allocators/destructors, callbacks/coroutines and
exceptions/bailouts stay native.

| Rust policy | Native effects and representation |
| --- | --- |
| Callable construction/release, keyed merge (index vs string route), bool/named call lifecycles | Values/refcounts, numeric-key classification, HashTable APIs, lookup, diagnostics and invocation |
| 64 known strings | Persistent/interned pointer table |
| String/CharPtr/KeyValue/Variable/Callable custody and factories; one release decision per String/CharPtr teardown | Allocation/copy/move/refcounts, storage, callbacks, destruction and errors |
| ArrayIterator traversal; Array update status | Bucket bounds/kinds/indirection, cursor/key storage and HashTable updates |
| `php_swoole_async` admission (blocking short-circuit, one nonblocking context query) | `std::function` capture/storage, invocation, async/yield, exceptions/bailout |

Edit the three overlays with `tools/patches.py`; see
[hybrid build](../../../docs/hybrid-build.md#editing-patches).

## Body gate (`gate.py`, `phphelpers-policy`)

```sh
python3 tests/modules/phphelpers/gate.py --source PINNED --sanitize --output BODY.json
```

Runs complete pinned and selected `php_swoole_cxx` bodies against modeled native
storage and effects, one fixture per family (`effects.cc`, `cache.cc`, ...):

| Family | Matched cases per host |
| --- | --- |
| Known cache / Callable cleanup | 266 / 12 |
| String+CharPtr / KeyValue+Variable / inline Callable | 13 / 9 / 14 |
| ArrayIterator / Array updates | 4681 bounded walks / 2 status profiles |

These are modeled counts, not Zend runtime coverage. `--rust-sanitize` builds a
private ASan-instrumented Rust module; `--archive EXACT --sanitize --rust-asan`
instead checks ASan imports in a supplied archive. Rust std, Zend and system
libraries stay uninstrumented, and Rust ASan does not work on macOS.

`async_gate.py --source PINNED --output UNIQUE.json [--archive FFI.a]`
(`phphelpers-async`) runs the original and selected `php_swoole_async` bodies with
inert recorders (8 dispatch cases), the C11 `async_abi.c` consumer, and C++14/17
with and without ASan/UBSan. It checks the native fallback is unchanged.

## SDK compile checks

```sh
python3 tests/modules/phphelpers/sdk.py --source PINNED --sdk-export IMMUTABLE/export.json --output SDK.json
```

`sdk.py` (`phphelpers-sdk`) compiles 16 actual-TU profiles (native/hybrid x
stdext off/on x two `.cc` files x C++14/17), checks seven class
size/alignment witnesses agree, compiles four optimized `php_swoole_async` caller
objects and checks their Rust imports. Native is the pin with every `SWOOLE_RS_*`
switch off; hybrid is the export's own build tree (patched headers,
`rust-prefix/include` and its switches). It also compares declarations outside 68
inline bodies and 10 verbatim macros against the pinned header. Missing optional
packages (for example libpq) keep their error but do not drop other flags. Its
report (per-command dependency hashes, `selected_header_sha256`, the native public
header hash) is the input for `entrypoint.py`, `facade_runtime.py`,
`async_optional_sdk.py`, `prepare_php_benchmark.py` and
`tests/modules/phpcoroutines/sdk.py`.

`async_optional_sdk.py --source PINNED --sdk-report SDK.json --provider sqlite|odbc
--output UNIQUE.json` compiles the unchanged optional SQLite or ODBC consumer TU
with its feature enabled (native/hybrid x C++14/17, `-Werror`) and checks
the defined async wrappers and Rust imports. ODBC uses installed pkg-config flags
and an explicit unixODBC configuration. Nothing is linked or executed.

## Loaded PHP checks

All take `--engine native=DSO --engine selected=DSO`, `--build-manifest`,
`--archive` (`manifest.parent/'src/../rust-prefix/lib/libswoole_rs_ffi.a'`),
`--source-ref` and `--output`; Linux needs `--php-extension curl.so`.

- `runtime.py` (`phphelpers-runtime`): stdext copied arrays, native callbacks,
  autoload and object lifetime (`runtime.php`), plus `bailout.php` children for
  direct/coroutine return/exception.
- `entrypoint.py --sdk-report SDK.json` (`phphelpers-entrypoint`): builds a
  test-only extension from `entrypoint.cc` and checks named C++ entry results and
  child diagnostics (`entrypoint.php`, `entrypoint-child.php`).
- `facade_runtime.py --sdk-report SDK.json [--prepare-only]`
  (`phphelpers-facade-runtime`): builds native and hybrid-header
  helpers from `facade_entrypoint.cc`, requires all seven Rust
  facade imports in selected helpers and none in native, checks the production
  header/local ABI against the SDK, then runs `facade_entrypoint.php`. Header
  variants against one DSO are not separate production feature builds.

The 31 `swoole_stdext` originals run in the merge tier's `phpt-stdext` job on
`--stdext` upstream and hybrid builds, which also runs the three loaded checks
above. Known result: macOS 31 PASS per engine; Linux 30
PASS / 1 FAIL per engine (`mb_ucfirst` missing on PHP 8.2). The stream original
uses `/tmp/test.txt`.

## Benchmark preparation

- `prepare_php_benchmark.py` (loaded PHP arguments plus `--sdk-report`): 16
  `php_benchmark.php` workloads through the producer DSOs with `--check`; requires
  optimized native `CXXFLAGS` and an optimized selected Rust profile.

It reads no clock. Measurement (`--measure`) must run without concurrent
workloads; none has run.

## Unit tests

```sh
for t in tests/modules/phphelpers/test_*.py; do python3 -m unittest "$t"; done
```

## Open gates

Loaded runtime for the keyed-merge routing, String/CharPtr release decision and
String dup/edup routing awaits a matching production build. The 48 optional DB
async callers have no runtime qualification (6 SQLite and 11 ODBC compile on
macOS; 31 Oracle/Firebird callers are not compiled). Whole caller/lifetime/fatal/
OOM/DEBUG/ZTS coverage, other architectures, separate production feature builds,
performance and default activation remain open.

## Gate audit (#493)

All five phases are kept, none deleted:

- `phphelpers-policy`: complete pinned and selected bodies against modeled
  native effects, with about 200,000 matched cases across the families above,
  under ASan/UBSan.
- `phphelpers-async`: the `php_swoole_async` dispatch bodies, the C11 ABI
  consumer, and C++14/17 builds with and without sanitizers.
- `phphelpers-sdk`: the only full-file compiles (24 profiles), the class layout
  witnesses and the Rust import checks. Its report is also the input that
  `phpcoroutines-sdk` and the stdext loaded checks read.
- `phphelpers-async-sqlite-sdk`, `phphelpers-async-odbc-sdk`: the only builds of
  the optional SQLite and ODBC consumers with their feature enabled.

None of these is a PHPT, Rust unit test or fuzz target.

