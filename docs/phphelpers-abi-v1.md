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

For the header classes, public names, signatures, field types and order,
visibility, object layout and macros are identical across native, M26-only and
combined builds; feature selection never changes class layout. Rust plans
lifecycle sequencing and iterator decisions only. Self-assignment,
initialization, copy/move/reset, duplicate-key ownership and failure order match
upstream. Plain getters and forwarders stay native.

The 31 stdext PHPTs need `--stdext` (`SW_STDEXT`), which default builds disable.
