# M03 timer ownership

Frozen queue ABI: `docs/timer-abi-v1.md`. Module-local clock/backend helpers:
`swoole_rs_timer_runtime.h`.

## Queue and timer

`crates/timer/src/lib.rs` is the only timer heap/index and scheduling state
machine. Its frozen exports own IDs, deadline ordering, update/repair,
captured dispatch time, selection rounds, active-ID completion, missed-repeat
coalescing, cancellation, closing/draining and allocation-failure rollback.

`crates/timer/src/owner.rs` is `swoole::Timer`: a Rust-owned record (backend,
reactor, clock base, armed wakeup, abandoned-selection marker) and its
operations, `init`/`reinit`, `add`, `remove`, `update`, `delay`, `select` and the
destructor's close/complete/end/drain/release. `native/timer/swoole_timer.h` is
the C++ view: `TimerNode`, and a `Timer` class with no data members whose methods
call `swr_v1_timer_owner_*`. `crates/timer/src/wrapper.rs` is the
`swoole_timer_*` API over it, with `SwooleTG.timer` through
`swr_v1_core_timer_slot`.

What C++ keeps (`native/timer/timer.cc`):

| Shim | Why |
| --- | --- |
| `swr_cpp_timer_node_new`, `_call`, `_release` | A `TimerNode` holds `std::function` callback and destructor. A C++ exception they throw comes back to Rust as an `exception_ptr`; `select` detaches the active node, ends the selection and returns the first one, which the C++ view's `select` or `remove` rethrows with `swr_cpp_timer_rethrow`: an exception can't unwind through Rust frames under `panic = "abort"`. The Rust `swoole_timer_*` exports have no C++ frame to rethrow in, so they abort, as an uncaught exception would. |
| `swr_cpp_timer_add` | A callback that is a plain function of its data, for Rust callers. |
| `swr_cpp_timer_reactor_timeout`, `swr_cpp_timer_attach` | `Reactor::timeout_msec`, and the timer-priority select, exit condition and destroy callback, which the reactor takes as `std::function`. |

A Zend bailout longjmps out of a callback over `select`'s Rust frames, which hold
nothing to drop.

`next_msec` is the armed-wakeup cache used by `reinit` and thread-manager
waits, not a second queue. Rust `end` computes the next delay.

Zend bailout: a fatal longjmp from a PHP callback used to leave selection active
and abort at shutdown. The PHP callback now marks the selection as abandoned
before propagating bailout; the owner destructor then uses the normal exact-ID
close/complete/end/drain path. Active self-deletion is still rejected. Details:
`tests/modules/timer/bailout.md`.

## Clock and backend helpers

Stateless helpers in `crates/timer/src/runtime.rs`.

| Function | Caller |
| --- | --- |
| `swr_v1_timer_backend` (manual/reactor/system) | `Timer::init`, from copied flags |
| `swr_v1_timer_system_set` (`setitimer`, positive interval or disarm) | `Timer::init`'s system-timer `set`/`close` |
| `swr_v1_timer_relative_msec` | `Timer::get_relative_msec` |
| `swr_v1_timer_rearm_after_add` | `Timer::add`; Native set still precedes the cached timeout write |
| `swr_v1_timer_delay_deadline` | `Timer::delay`, before the queue update; no output write on rejection |

`timer_uses_reactor` in `native/timer/phptimer.cc`: user workers and
coroutine-enabled task workers use the reactor; other server roles and a
process-pool master use the system timer.

## Known failures

The unchanged 200-trial signal test fails on both engines: upstream 94 PASS /
4 WARNED / 2 FAILED, Rust 94 PASS / 6 WARNED. See `tests/modules/timer/UPSTREAM.md`
and `tests/modules/timer/RUST_ASAN.md` for original-suite and sanitizer runs.
Combined macOS Native/Rust ASan needs a compatible compiler/runtime pair.
