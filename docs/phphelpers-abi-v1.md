# Common PHP helpers and stdext boundary (M26)

Feature value 16777216 (`--module phphelpers`). Upstream targets:
`ext-src/php_swoole_cxx.cc`, `ext-src/swoole_stdext.cc`, and in
`ext-src/php_swoole_cxx.h` only `zend::String`, `KeyValue`, `ArrayIterator`,
`Array`, `Variable`, `CharPtr`, the inline `Callable` helpers and
`sw_callable_create` / `sw_callable_create_ex`. Follows the
[policy/effect split](abi-v1.md#policyeffect-split).

`ext-src/swoole_stdext.cc` is Rust in `crates/phpstdext` (docs/zend.md): C++
keeps the `ASSIGN_DIM`/`ASSIGN_DIM_OP` handlers, which need PHP's
`zend_execute.c` internals, and lends the PCRE and JSON library calls at MINIT.

For the common owning facades, Rust owns the complete lifetime and operations:
String conversion/copy/move/release/trim/duplication, CharPtr request allocation,
Variable copy/move/reference operations, KeyValue custody and insertion,
ArrayIterator sparse/indirect walks, Array updates, and Callable
resolution/copy/invocation/release and factories. C++ keeps source-compatible
constructors and forwarding methods. Variable's public `value` aliases stable
Rust-owned zval storage; String and Callable hold opaque Rust handles. Iterators
are copyable borrowed cursors and never own or modify the array's buckets.

Rust calls Zend directly through the typed SDK and existing macro shims. Only
leaf Zend effects which can execute PHP use C `zend_try` shims; no operation
table or native phase loop implements these families. PHP warning reporting
uses the existing C++ fatal-error reporter, trapped before Rust continues.
The known-string pointer cache and function/merge adapter policy remain native.
SDK compilation and loaded helper fixtures compare behavior without freezing
upstream class text or layout; the PHP facade expectations remain unchanged.

The 31 stdext PHPTs need `--stdext` (`SW_STDEXT`), which default builds disable.
