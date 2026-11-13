# Reactor callbacks and deferred queues

## End callbacks and exit conditions

Upstream's `end_callbacks` and `exit_conditions` maps are fixed slot arrays in the
Rust reactor state (`crates/reactor/src/callbacks.rs`), keyed by `EndCallback`
(0 to 7) and `ExitCondition` (0 to 9, 999, 1000) and walked in key order. A slot
holds a plain function (`void (*)(void)`, `bool (*)(size_t *event_num)`) or a
built-in; no C++ caller captures state, so neither takes data. A walk re-reads each
slot when it reaches it: a callback that sets a later key is seen, one it erases is
not, and an earlier key it sets waits for the next walk. `if_exit` starts from the
registered socket count, shares it across the conditions and stops at the first
refusal.

The C++ members (`set_end_callback`, `erase_end_callback`, `set_exit_condition`,
`remove_exit_condition`, `isset_exit_condition`) are inline calls to
`swr_v1_reactor_set_end`, `_set_exit` and `_isset_exit`; Rust callers use the
`swr_v1_event_*` forms over the thread's reactor, by path (they have no symbol).
`Reactor::if_exit` and `Reactor::wait` are Rust under their upstream symbols.

The constructor's built-ins, set after the create hook
(`install_builtins`): the deferred queue's end callback and exit
condition, the idle task, signal dispatch (main thread), try-exit, `malloc_trim`
(main thread, glibc) and the default condition (no sockets registered).

## Handlers

`set_handler`, `isset_handler`, `get_handler` and `get_error_handler` are Rust
tables (`crates/reactor/src/handlers.rs`). `get_handler` and `default_write_handler`
have no C++ caller, so the class no longer has them: Rust reads them as
`reactor_upstream::reactor_get_handler` and `reactor_default_write_handler`.

## Deferred queues

`defer_tasks` and `destroy_callbacks` are fields of the Rust reactor state:
FIFO queues of C `(call, data, dispose)` entries (`crates/reactor/src/deferred.rs`),
created on first push. `swoole_event_defer` takes a plain `void (*)(void *)` and its
data (`Reactor::defer` has no C++ caller left, so the class no longer has it).

- The `PRIORITY_DEFER_TASK` end callback detaches `defer_tasks`, runs it and
  frees it, so a task that defers again lands on the next end pass. A non-null
  `defer_tasks` makes the kernel wait non-blocking.
- The Reactor destructor runs `destroy_callbacks` until empty (callbacks it
  adds run too); the state frees the queue.
