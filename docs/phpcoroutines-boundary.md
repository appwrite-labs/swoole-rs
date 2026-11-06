# PHP coroutine frontend boundary (M29)

Feature value 134217728 (`SWOOLE_RS_PHPCOROUTINES`).
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
calls (hook flags, the scheduler's configuration); C++ starts coroutines with
`swr_v1_phpcoroutines_create`.

`swoole_coroutine_scheduler.cc` is out of the build: `Swoole\Coroutine\Scheduler`,
the coroutine options and the reactor's user exit condition are Rust
(`scheduler.rs`), under upstream's `php_swoole_coroutine_scheduler_minit`,
`_rshutdown` and `php_swoole_set_coroutine_option` symbols. C++ keeps only the
reactor side of the exit condition, which calls `swr_v1_phpcoroutines_can_exit`.
`swoole_coroutine_system.cc` is upstream's.
