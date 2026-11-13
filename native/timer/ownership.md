# M03 timer ownership

Frozen queue ABI: `docs/timer-abi-v1.md`.

## Queue and timer

`crates/timer/src/lib.rs` is the only timer heap/index and scheduling state
machine. Its `Timer` methods, and the frozen exports over them, own IDs, deadline ordering, update/repair,
captured dispatch time, selection rounds, active-ID completion, missed-repeat
coalescing, cancellation, closing/draining and allocation-failure rollback.

`crates/timer/src/owner.rs` is `swoole::Timer`: a Rust-owned record (backend,
reactor, clock base, armed wakeup, abandoned-selection marker) and its
operations, `init`/`reinit`, `add`, `remove`, `update`, `delay`, `select` and the
destructor's close/complete/end/drain. It holds the queue in a `RefCell` that no
borrow holds across a callback or destructor, so a bailout never skips a release. It allocates and frees every
`TimerNode`, whose callback and destructor are plain C function pointers;
`swr_v1_timer_add_plain` adds one whose callback is a function of its data alone,
for Rust callers, which call it by path. With a reactor it hooks
`Reactor::timeout_msec`, the timer-priority end callback, the exit condition and a
destroy callback that frees this thread's timer. No C++ reaches into the timer or calls the
`swoole_timer_*` API; `swoole.h` declares `TimerNode` for the pointers it passes.
`crates/timer/src/wrapper.rs` is that API, under its C++ names, with `SwooleTG.timer` through `swr_v1_core_timer_slot`.

A Zend bailout longjmps out of a callback or destructor over `select`'s Rust
frames, which hold nothing to drop. A C++ exception a callback or destructor
throws aborts at the first Rust frame (`panic = "abort"`), as an uncaught one
would.

`next_msec` is the armed-wakeup cache used by `reinit` and thread-manager
waits, not a second queue. Rust `end` computes the next delay.

Zend bailout: a fatal longjmp from a PHP callback used to leave selection active
and abort at shutdown. The PHP callback now marks the selection as abandoned
before propagating bailout; the owner destructor then uses the normal exact-ID
close/complete/end/drain path. Active self-deletion is still rejected. Tested by
`tests/modules/timer/test_bailout.py`.

## Clock and backend helpers

Stateless helpers in `crates/timer/src/runtime.rs`, called from Rust only (`owner.rs`).

| Function | Caller |
| --- | --- |
| `swr_v1_timer_backend` (manual/reactor/system) | `Timer::init`, from copied flags |
| `swr_v1_timer_system_set` (`setitimer`, positive interval or disarm) | `Timer::init`'s system-timer `set`/`close` |
| `swr_v1_timer_relative_msec` | `Timer::get_relative_msec` |
| `swr_v1_timer_rearm_after_add` | `Timer::add`; Native set still precedes the cached timeout write |
| `swr_v1_timer_delay_deadline` | `Timer::delay`, before the queue update; `None` on rejection |

`timer_uses_reactor` in `crates/phpprocess/src/host.rs`: user workers and
coroutine-enabled task workers use the reactor; other server roles and a
process-pool master use the system timer.

## Known failures

The unchanged 200-trial signal test fails on both engines: upstream 94 PASS /
4 WARNED / 2 FAILED, Rust 94 PASS / 6 WARNED. See `tests/modules/timer/README.md`.
Combined macOS Native/Rust ASan needs a compatible compiler/runtime pair.
