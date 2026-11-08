# M03 timer ownership

Frozen queue ABI: `docs/timer-abi-v1.md`. Module-local clock/backend helpers:
`swoole_rs_timer_runtime.h`.

| Patch | Upstream file |
| --- | --- |
| 20261108T052712-timer-swoole-timer-is-rust-… | deletes `include/swoole_timer.h` and `src/core/timer.cc` |
| 0003-timer-php | `ext-src/swoole_timer.cc` |
| 20261107T031810-timer-src-wrapper-… | deletes `src/wrapper/timer.cc` |
| 0005-timer-clock | `src/os/timer.cc` |

## Queue and timer

`crates/timer/src/lib.rs` is the only timer heap/index and scheduling state
machine. Its 14 frozen exports own IDs, deadline ordering, update/repair,
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
| `swr_cpp_timer_reactor_timeout`, `swr_cpp_timer_attach` | `Reactor::timeout_msec`, and the timer-priority select, exit condition and destroy callback, which the reactor takes as `std::function`. |

A Zend bailout longjmps out of a callback over `select`'s Rust frames, which hold
nothing to drop.

`next_msec` is the armed-wakeup cache used by `reinit` and thread-manager
waits, not a second queue. Rust `end` computes the next delay.

Zend bailout: a fatal longjmp from a PHP callback used to leave selection active
and abort at shutdown. The PHP callback now marks the selection as abandoned
before propagating bailout; the owner destructor then uses the normal exact-ID
close/complete/end/drain path. Active self-deletion is still rejected. Tested by
`tests/modules/timer/test_bailout.py`.

## Clock and backend helpers

Seven stateless helpers; none allocates, keeps pointers, reads clocks, changes
errno or invokes callbacks.

| Function | Caller |
| --- | --- |
| `swr_v1_timer_backend` (manual/reactor/system) | `Timer::init`, from copied flags |
| `swr_v1_timer_system_set` (`setitimer`, positive interval or disarm) | `Timer::init`'s system-timer `set`/`close`; upstream `src/os/timer.cc`, deleted |
| `swr_v1_timer_relative_msec` | `Timer::get_relative_msec` |
| `swr_v1_timer_rearm_after_add` | `Timer::add`; Native set still precedes the cached timeout write |
| `swr_v1_timer_delay_deadline` | `Timer::delay`, before the queue update; no output write on rejection |

## Still Native

- `src/os/timer.cc`: SIGALRM handler install, `timeval` packing and one
  `setitimer` call, `system_clock` read into `timespec`.
- Reactor callback binding and destruction notification (needs the M05
  lifecycle).
- `timer_uses_reactor` in `native/timer/phptimer.cc`: user workers and
  coroutine-enabled task workers use the reactor; other server roles and a
  process-pool master use the system timer. Belongs with M08.

External callers any replacement must preserve:

- `src/reactor/base.cc` cancels a socket send timer after draining output.
- `src/network/socket.cc` cancels receive/send timers before deregistration and
  defers socket destruction until callbacks unwind.
- `src/core/base.cc` frees the inherited timer before resetting the child event
  loop in `swoole_fork`; the parent's active callback does not resume in the
  child.
- `src/server/reactor_process.cc` and `src/server/master.cc` rebind a system timer
  to a new reactor with `reinit`.
- `src/server/thread.cc` uses a manually triggered manager timer and its next
  delay for condition-variable waits.
- `src/coroutine/thread_context.cc` can share the timer across threads under the
  thread-context lock. The frozen ABI says one owner thread; serial cross-thread
  transfer is unresolved and `--enable-thread-context` is not validated.

## Known failures

The unchanged 200-trial signal test fails on both engines: upstream 94 PASS /
4 WARNED / 2 FAILED, Rust 94 PASS / 6 WARNED. See `tests/modules/timer/README.md`.
Combined macOS Native/Rust ASan needs a compatible compiler/runtime pair.
