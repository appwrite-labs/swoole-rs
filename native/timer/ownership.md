# M03 timer ownership

Frozen queue ABI: `docs/timer-abi-v1.md`. Module-local clock/backend helpers:
`swoole_rs_timer_runtime.h`.

| Patch | Upstream file |
| --- | --- |
| 0001-timer-header | `include/swoole_timer.h` |
| 0002-timer-body | `src/core/timer.cc` |
| 0003-timer-php | `ext-src/swoole_timer.cc` |
| 20261107T031810-timer-src-wrapper-… | deletes `src/wrapper/timer.cc` |
| 0005-timer-clock | `src/os/timer.cc` |

## Queue

`crates/timer/src/lib.rs` is the only timer heap/index and scheduling state
machine. Its 14 frozen exports own IDs, deadline ordering, update/repair,
captured dispatch time, selection rounds, active-ID completion, missed-repeat
coalescing, cancellation, closing/draining and allocation-failure rollback.
There is no Native heap or map in the selected `swoole_timer.h`.

What Native keeps:

| Location | Native responsibility |
| --- | --- |
| `timer.cc` `apply` | Copy returned Rust metadata into the Native callback token. |
| constructor, `init`, `reinit` | Create the Rust handle, read the monotonic base, select manual/reactor/system execution, rearm the external timeout. |
| `release_node` | Run the `std::function`/Zend destructor and delete the TimerNode after Rust returns custody; propagate a pending Zend bailout afterwards. |
| `init_with_reactor` | Set `Reactor::timeout_msec`; attach the timer-priority select, exit condition and destroy callback. |
| destructor | Drive close/complete/end/drain/release through Rust; run returned destructors outside Rust. A PID check distinguishes inherited child cleanup from a callback that resumes in the same process. |
| `add` | Reject invalid durations with a Native warning; allocate the callback object; call Rust add; bring an armed external wakeup forward if needed. These checks run before allocation to preserve failure order. |
| `remove` | Act on RELEASE/DEFERRED and run a detached destructor. |
| `select` | Call Rust begin/next/complete/end around callback execution; on a C++ exception, clean up selection while preserving the primary exception. |
| header `update`/`delay` | Read caller-mutated metadata or the clock; refresh the token from Rust. |
| header get/count/round/get_ids | Copy Rust views/stats/IDs; typed get filters by token type. |
| `crates/timer/src/wrapper.rs` | The `swoole_timer_*` API (`src/wrapper/timer.cc` is deleted): `SwooleTG.timer` through `swr_v1_core_timer_slot`, lazy creation, unit conversion, warnings; clear removes by ID. Reads the `Timer`/`TimerNode` fields `timer.cc`'s `apply` asserts. |
| `crates/phptimer` clear-all, list | Snapshot IDs, resolve each again, filter PHP tokens; run destructors or build Zend values after Rust returns. |
| PHP after/tick/info/stats/clear | Zend parsing/values, callable persistence, callback/destructor attachment. |

`next_msec_` is the Native armed-wakeup cache used by `reinit` and thread-manager
waits, not a second queue. Rust `end` computes the next delay.

Zend bailout: a fatal longjmp from a PHP callback used to leave selection active
and abort at shutdown. The PHP callback now marks the Native selection as
abandoned before propagating bailout; the owner destructor then uses the normal
exact-ID close/complete/end/drain path. This is a Native stack marker, not Rust
state. Active self-deletion is still rejected. Details:
`tests/modules/timer/bailout.md`.

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
4 WARNED / 2 FAILED, Rust 94 PASS / 6 WARNED. See `tests/modules/timer/UPSTREAM.md`
and `tests/modules/timer/RUST_ASAN.md` for original-suite and sanitizer runs.
Combined macOS Native/Rust ASan needs a compatible compiler/runtime pair.
