# PHP Table and coroutine Channel boundary (M27)

Feature value 33554432. Upstream targets:
`ext-src/swoole_table.cc`, `ext-src/swoole_channel_coro.cc`; both have left
the build. `Swoole\Table` is all Rust (`php_swoole_table_minit`, over the
memory crate's table and Zend's string hash); so is `Swoole\Coroutine\Channel`
(`php_swoole_channel_coro_minit`, over crates/coroutine's channel, with PHP's
coroutines as its effects). Public headers,
stubs/arginfo, Table/Channel/row/column layouts, M12 storage, M06 channel
lifetime and M16 locking are unchanged. Follows the
[policy/effect split](abi-v1.md#policyeffect-split).

Rust owns the callback-free validation, type/value/return decisions, iteration
and operation sequencing in both files. Native code does every Zend conversion,
allocation, class/object/property registration, refcount/COW/GC/destructor,
exception/bailout and row lock/unlock.

- Constructor defaults, type and narrowing checks, binary key/field lengths,
  column conversions, return/error/property publication and row-unlock/temporary
  cleanup order match upstream. Undefined overflow and casts are outside the
  compared domain.
- Table iterators and borrowed row values stay inside their original locked scope.
- Channel zval custody transfers only at the original successful push/pop point.
  Suspension goes through phpcoroutines' cancelable yield and timer; the
  destructor drains the queued zvals, then warns about discarded waiters as
  `Channel::~Channel` did.
