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
Zend layouts no longer qualifies production. `phphelpers-runtime` executes
both engines against unchanged PHP observations. `php_swoole_async`'s admission and its
dispatch branches are Rust unit tests (`crates/phphelpers/src/asynchronous.rs`).

The SDK compile checks (`phphelpers-sdk`, the async SQLite/ODBC SDK phases) and the
test-only helper extensions (`phphelpers-entrypoint`, `phphelpers-facade-runtime`,
the benchmark preparation) left with `php_swoole_cxx.h`, `php_swoole_private.h`,
`swoole_sqlite.cc` and `swoole_odbc.cc` (#1708, #1791): they qualified the
public C++ header against the hybrid, which no longer has one. The extension's
own C++ uses `native/phphelpers/swoole_rs_phphost.h`, which the hybrid build
compiles.

## Loaded PHP checks

`runtime.py` (`phphelpers-runtime`) takes `--engine native=DSO --engine selected=DSO`,
`--build-manifest`, `--archive` (`manifest.parent/'src/../rust-prefix/lib/libswoole_rs_ffi.a'`),
`--source-ref` and `--output`: stdext copied arrays, native callbacks,
autoload and object lifetime (`runtime.php`), `bailout.php` children for
direct/coroutine return/exception, and `vm_bailout.php` children that exit from
PCRE/JSON warning handlers and a VM destructor inside stdext calls.

The 31 `swoole_stdext` originals run in the merge tier's `phpt-stdext` job on
`--stdext` upstream and hybrid builds, which also runs `phphelpers-runtime`.
Known result: macOS 31 PASS per engine; Linux 30
PASS / 1 FAIL per engine (`mb_ucfirst` missing on PHP 8.2). The stream original
uses `/tmp/test.txt`.

## Unit tests

```sh
for t in tests/modules/phphelpers/test_*.py; do python3 -m unittest "$t"; done
```

## Open gates

Whole caller/lifetime/fatal/OOM/DEBUG/ZTS coverage, other architectures,
performance and default activation remain open.

## Owning Zend facades (#1063)

Callable now executes
its ownership and operations in Rust against the actual Zend SDK.
String and Variable, and their fixture cases and request-allocation probes,
left with their last callers. Their former
modeled C++ fixtures (13/9/4681/14 cases) copied private class bodies and supplied
fake Zend layouts; those assumptions are retired. Array/ArrayIterator, and the
keyed merge, left with their last callers.
The obsolete call/merge/cache policy phase is retired after those complete operations move to Rust.

