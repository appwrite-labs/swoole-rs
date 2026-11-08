# PHP coroutine frontend boundary (M29)

Feature value 134217728. Follows [docs/zend.md](zend.md).

`crates/phpcoroutines` is upstream's `PHPCoroutine` and `swoole_coroutine.cc`:
each coroutine's PHP context and its VM swap (executor, output and serializer
globals, through `crates/zend-sys/src/shim_coroutine.c`), activation and
configuration, the deadlock check, the preemptive scheduler's interrupt thread,
`Swoole\Coroutine`'s methods, `go()`/`defer()`, `Swoole\ExitException`, the
exit and `@` opcode handlers and the coroutine-aware autoloader.

MINIT, RINIT and RSHUTDOWN are Rust under upstream's `php_swoole_coroutine_*`
symbols, and the coroutine's entry is Rust over `swr_zend_coroutine_run`, the
zend-sys shim that runs the user callable and its defer tasks inside
`zend_first_try`. Rust reads the coroutine core's state (crates/coroutine's
scheduler and each coroutine's record) and calls the module host, the runtime
hooks and the event loop directly. Rust callers start coroutines with
`swr_v1_phpcoroutines_create`. `native/phpcoroutines/coroutine.cc` keeps what
needs C++: a `Coroutine`'s context and `std::function` members (timer, defer,
bailout, the user exit condition), and reads of `Server`, `Reactor` and io_uring.

`Swoole\Coroutine\Scheduler`, the coroutine options and the reactor's user exit
condition are Rust (`scheduler.rs`), under upstream's
`php_swoole_coroutine_scheduler_minit`, `_rshutdown` and
`php_swoole_set_coroutine_option` symbols; the shim keeps only the reactor side
of the exit condition, which calls `swr_v1_phpcoroutines_can_exit`.
`Swoole\Coroutine\System` and `FILE_LOCK` are Rust (`system.rs`), under
upstream's `php_swoole_coroutine_system_minit`. It calls
`swoole::coroutine::System`'s static members by their own symbols, and the
`swr_v1_system_*` functions in `native/hooks/system.cc` where a C++ string or list
holds the result.
