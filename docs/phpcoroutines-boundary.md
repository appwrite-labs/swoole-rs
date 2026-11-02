# PHP coroutine frontend boundary (M29)

Feature value 134217728 (`--module phpcoroutines`, `SWOOLE_RS_PHPCOROUTINES`).
Follows [docs/zend.md](zend.md).

`crates/phpcoroutines` is upstream's `PHPCoroutine` and `swoole_coroutine.cc`:
each coroutine's PHP context and its VM swap (executor, output and serializer
globals, through `crates/zend-sys/src/shim_coroutine.c`), activation and
configuration, the deadlock check, the preemptive scheduler's interrupt thread,
`Swoole\Coroutine`'s methods, `go()`/`defer()`, `Swoole\ExitException`, the
exit and `@` opcode handlers and the coroutine-aware autoloader.

C++ keeps the arginfo method tables, the coroutine core's operations (lent as
`swr_v1_phpcoroutines_ops`) and the coroutine's entry function, which runs the
user callable and its defer tasks inside `zend_first_try` so a bailout never
crosses a Rust frame. `PHPCoroutine` remains as the entry points other C++
calls (`create`, hook flags, the scheduler's configuration).

`swoole_coroutine_scheduler.cc` and `swoole_coroutine_system.cc` are upstream's.
