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

A bailout abandons suspended coroutine stacks permanently. Only an entry that
returns reaches the core's destruction hook; dropping the live registry does not
free its non-owning handles. Thread contexts likewise stay parked: deactivation
clears shared runtime state, and only a completed context is joined. Yielding is
safe because those frames and their values remain allocated.

The main stack can be reused after a bailout. Raw `coroutine::resume` and `cancel`
therefore require their callers to keep no
pending drops between the call and its bailout trap. PHP handlers release their
owners before resuming; deferred join callbacks release their `Rc` before the
switch, and the reactor unboxes deferred values before calling them.
`Waiter::wake`, `Waiter::cancel` and `channel::Queue::close` instead contain the resumed coroutine's
bailout and return `Result`: their callers may own values. HTTP/2 stops frame
processing and defers the error until the receiving frame releases its stream
and response owners. Channel close returns its error to the PHP handler's
outer bailout boundary. These traps enclose only the resume/close operations,
whose main-stack frames have no pending drops; they do not unwind or reclaim
abandoned coroutine stacks.

The `phpcoroutines-bailout` gate compares channel-close resumption on the main
stack and inside a coroutine with the pinned engine, both on normal return and
on a fatal error in the resumed waiter. It requires shutdown to run and rejects
execution past a fatal close. The HTTP/2 frame gate covers its wake consumers.

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
