# PHP coroutine frontend boundary (M29)

Feature value 134217728. Follows [docs/zend.md](zend.md).

`crates/phpcoroutines` is upstream's `PHPCoroutine` and `swoole_coroutine.cc`:
each coroutine's PHP context and its VM swap (executor, output and serializer
globals, through `crates/zend-sys/src/shim_coroutine.c`), activation and
configuration, the deadlock check, the preemptive scheduler's interrupt thread,
`Swoole\Coroutine`'s methods, `go()`/`defer()`, `Swoole\ExitException`, the
exit and `@` opcode handlers and the coroutine-aware autoloader.

MINIT, RINIT and RSHUTDOWN are Rust under upstream's `php_swoole_coroutine_*`
names, which crates/ffi hands crates/phpswoole's phases in its `Families`
table (phpswoole sits below this crate), and the coroutine's entry is Rust over `swr_zend_coroutine_run`, the
zend-sys shim that runs the user callable and its defer tasks inside
`zend_first_try`. Rust reads the coroutine core's state (crates/coroutine's
scheduler and each coroutine's record) and calls the module host, the runtime
hooks and the event loop directly. Rust callers start coroutines with
`create_coroutine` (and C++ with `zend::function::call`); a coroutine that ends in an exception or a
bailout stops the loop and bails out through `swr_v1_coroutine_bail`. The server, reactor and io_uring
reads, deferred tasks, timers and the build's switches are Rust (`host.rs`).

`Swoole\Coroutine\Scheduler`, the coroutine options and the reactor's user exit
condition are Rust (`scheduler.rs`), under upstream's
`php_swoole_coroutine_scheduler_minit`, `_rshutdown` and
`php_swoole_set_coroutine_option` names; the reactor keeps the exit condition
as a plain function (`swr_v1_event_set_exit_condition`).
`Swoole\Coroutine\System` and `FILE_LOCK` are Rust (`system.rs`), under
upstream's `php_swoole_coroutine_system_minit` name. It calls
`swoole::coroutine::System`'s static members by their own symbols, and the
`swr_v1_system_*` functions in `crates/hooks/src/resolve.rs` where a C++ string or list
holds the result.
