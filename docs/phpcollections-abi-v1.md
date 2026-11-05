# PHP Table and coroutine Channel boundary (M27)

Feature value 33554432. Upstream targets:
`ext-src/swoole_table.cc`, `ext-src/swoole_channel_coro.cc`. Public headers,
stubs/arginfo, Table/Channel/row/column layouts, M12 storage, M06 channel
lifetime and M16 locking are unchanged. Follows the
[policy/effect split](abi-v1.md#policyeffect-split).

Rust owns the callback-free validation, type/value/return decisions, iteration
and operation sequencing in both files. Native code does every Zend conversion,
allocation, class/object/property registration, refcount/COW/GC/destructor,
exception/bailout, row lock/unlock, queue transfer and coroutine suspension.

- Constructor defaults, type and narrowing checks, binary key/field lengths,
  column conversions, return/error/property publication and row-unlock/temporary
  cleanup order match upstream. Undefined overflow and casts are outside the
  compared domain.
- Table iterators and borrowed row values stay inside their original locked scope.
- Channel zval custody transfers only at the original successful push/pop point;
  suspension, cancellation, close and destructor draining stay native.
