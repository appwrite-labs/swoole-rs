# Common PHP helpers and stdext boundary (M26)

Feature value 16777216. Upstream targets:
`ext-src/php_swoole_cxx.cc` (deleted; `native/phphelpers/module.cc` keeps its C++), `ext-src/swoole_stdext.cc`, and in
`ext-src/php_swoole_cxx.h` only `zend::String`, `Variable`, the inline
`Callable` helpers and `sw_callable_create`. Helpers nothing in the all-modules
build calls (`KeyValue`, `CharPtr`, `ArrayIterator`, `Array`, `get_function`,
`array_add_or_merge`, `sw_callable_create_ex` and the unused
`php_swoole_private.h` macros and inlines) are deleted, with their Rust exports. Follows the
[policy/effect split](abi-v1.md#policyeffect-split).

`ext-src/swoole_stdext.cc` is Rust in `crates/phpstdext` (docs/zend.md): C++
keeps the `ASSIGN_DIM`/`ASSIGN_DIM_OP` handlers, which need PHP's
`zend_execute.c` internals, and lends the PCRE and JSON library calls at MINIT.

For the common owning facades, Rust owns the complete lifetime and operations:
String conversion/copy/move/release/trim/duplication,
Variable copy/move/reference operations, and Callable
resolution/copy/invocation/release and factories. C++ keeps source-compatible
constructors and forwarding methods. Variable's public `value` aliases stable
Rust-owned zval storage; String and Callable hold opaque Rust handles.

Rust calls Zend directly through the typed SDK and existing macro shims. Only
leaf Zend effects which can execute PHP use C `zend_try` shims; no operation
table or native phase loop implements these families. PHP warning reporting
uses the existing C++ fatal-error reporter, trapped before Rust continues.
Rust also owns the known-string pointer cache lifecycle, direct/named/coroutine
call orchestration, heap-zval custody, string recycling, object
creation, property operations and retained fcall arguments/caches. C++ keeps the
known-string global as a borrowed ABI view and the coroutine-core creation leaf.
SDK compilation and loaded helper fixtures compare behavior without freezing
upstream class text or layout; the PHP facade expectations remain unchanged.

The 31 stdext PHPTs need `--stdext` (`SW_STDEXT`), which default builds disable.

C++ facade object layouts change with ownership migration. Consumers must be
recompiled against the headers of the engine they load. The entrypoint and
benchmark-preparation harnesses build separate native and hybrid callers, just
as the facade runtime does; they do not mix a pinned stack layout with a Rust
handle layout. The observed PHP values, refcounts and error expectations stay
identical across those separately compiled consumers.
