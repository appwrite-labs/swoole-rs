# M26 common PHP helpers and optional stdext

M26 covers `ext-src/php_swoole_cxx.cc`, `ext-src/swoole_stdext.cc` and the
common Zend owning facades in `ext-src/php_swoole_cxx.h` and shared property/call/value operations in `ext-src/php_swoole_private.h`. Callable
owns a Rust handle. C++ keeps the
source-compatible constructors and methods. String and Variable left with their last callers. The contract is
[docs/phphelpers-abi-v1.md](../../../docs/phphelpers-abi-v1.md).

`ext-src/swoole_stdext.cc`.
`ext-src/swoole_stdext.cc` (typed arrays, methods on arrays/strings/streams and
the `swoole_*` stdext functions) is Rust in `crates/phpstdext` over the zend
crates (docs/zend.md); only the `ASSIGN_DIM`/`ASSIGN_DIM_OP` handlers (PHP's
`zend_execute.c` internals) and the PCRE/JSON library calls stay C++.

Rust owns the facade values and their operations through typed Zend layouts.
Version-dependent Zend leaf effects remain C; calls that can execute PHP trap
bailouts there before returning to Rust. Function calls and known-string allocation/publication also execute in Rust.
The C++ known-string global remains a borrowed ABI view.

| Rust ownership / operations | Native leaf effects |
| --- | --- |
| Callable lifetimes and factories | Zend allocators/refcount macros; trapped PHP calls and diagnostics |
| Function calls; 64 known strings and their pointer cache | Trapped Zend invocation/hash effects; persistent allocation/interning |
| `php_swoole_async` admission | `std::function` storage, native invocation/async/yield |

Edit the three overlays with `tools/patches.py`; see
[hybrid build](../../../docs/hybrid-build.md#editing-patches).

## Common-helper qualification

The common owning values, calls and known-cache bodies execute in Rust.
Their old C++-body policy fixture is retired; copying deleted bodies into fake
Zend layouts no longer qualifies production. `phphelpers-sdk` compiles the
actual native and selected headers, and `phphelpers-facade-runtime`,
`phphelpers-entrypoint` and `phphelpers-runtime` execute both engines against
unchanged PHP observations. `php_swoole_async`'s admission and its
dispatch branches are Rust unit tests (`crates/phphelpers/src/asynchronous.rs`).

## SDK compile checks

```sh
python3 tests/modules/phphelpers/sdk.py --source PINNED --sdk-export IMMUTABLE/export.json --output SDK.json
```

`sdk.py` (`phphelpers-sdk`) compiles 16 actual-TU profiles (native/hybrid x
stdext off/on x two `.cc` files x C++14/17), compiles four optimized
`php_swoole_async` caller objects and checks their Rust imports. Native is the pin with every `SWOOLE_RS_*`
switch off; hybrid is the export's own build tree (patched headers,
`rust-prefix/include` and its switches). Missing optional
packages (for example libpq) keep their error but do not drop other flags. Its
report (per-command dependency hashes, `selected_header_sha256`, the native public
header hash) is the input for `entrypoint.py`, `facade_runtime.py`,
`async_optional_sdk.py` and `prepare_php_benchmark.py`.

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
  helpers from `facade_entrypoint.cc`, requires the remaining iterator
  and call-result Rust imports in selected helpers and none in native, rejects
  retired String/Callable step-plan imports, checks the production
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

Loaded runtime for the keyed-merge routing awaits a matching production build. The 48 optional DB
async callers have no runtime qualification (6 SQLite and 11 ODBC compile on
macOS; 31 Oracle/Firebird callers are not compiled). Whole caller/lifetime/fatal/
OOM/DEBUG/ZTS coverage, other architectures, separate production feature builds,
performance and default activation remain open.

## Gate audit (#493)

The live qualification phases are:

- `phphelpers-sdk`: the full-file compiles (16 profiles) and Rust import checks.
  Its report is also the input that
  `phpcoroutines-sdk` and the stdext loaded checks read.
- `phphelpers-async-sqlite-sdk`, `phphelpers-async-odbc-sdk`: the only builds of
  the optional SQLite and ODBC consumers with their feature enabled.

None of these is a PHPT, Rust unit test or fuzz target.


## Owning Zend facades (#1063)

Callable now executes
its ownership and operations in Rust against the actual Zend SDK.
String and Variable, and their fixture cases and request-allocation probes,
left with their last callers. Their former
modeled C++ fixtures (13/9/4681/14 cases) copied private class bodies and supplied
fake Zend layouts; those assumptions are retired. Array/ArrayIterator, and the
keyed merge, left with their last callers. The SDK compile and loaded facade runtime remain mandatory, with the original PHP expectations unchanged:
callback duplication/invocation and destructor timing.
The obsolete call/merge/cache policy phase is retired after those complete operations move to Rust.

