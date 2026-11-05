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

For `php_swoole_cxx`, Rust owns callback-free callable/merge decisions. Zend
values, HashTables, COW, GC, allocators/destructors and the known-string
storage stay native.

String, CharPtr, Variable, inline Callable helpers and callable factories retain
upstream's bodies and Zend-owned state. Their former step-plan loops merely
repeated those native effects over FFI and are removed. KeyValue custody,
ArrayIterator traversal and Array update-result policy remain in Rust; native
storage and effects preserve upstream initialization, copy/move/reset,
duplicate-key ownership and failure order. SDK compilation and loaded helper
fixtures compare behavior without freezing upstream class text or layout.

The 31 stdext PHPTs need `--stdext` (`SW_STDEXT`), which default builds disable.
