# M03 timer ownership

Frozen queue ABI: `docs/timer-abi-v1.md`.

## Queue and timer

`crates/corebase/src/timer/mod.rs` is the only timer heap/index and scheduling state
machine. Its Rust `Timer` methods own IDs, deadline ordering, update/repair,
captured dispatch time, selection rounds, active-ID completion, missed-repeat
coalescing, cancellation, closing/draining and allocation-failure rollback.

`crates/corebase/src/timer/owner.rs` is `swoole::Timer`: a Rust-owned record (backend,
reactor, clock base, armed wakeup, abandoned-selection marker) and its
operations, `init`/`reinit`, `add`, `remove`, `update`, `delay`, `select` and the
destructor's close/complete/end/drain. It takes the boxed queue out of a `Cell` for each operation and returns it before
a callback or destructor, so a bailout never skips a borrow release. It allocates and frees every
`TimerNode`, which is internal to the crate. `add_fn` holds a closure of at most two
words in its node, owned captures and all, and drops it with the node; `add_owned`
boxes a larger one.
Callers retain generation-tagged `Held` IDs for cancellation and delay. With a
reactor it hooks `Reactor::timeout_msec`, the timer-priority end callback, the exit
condition and a destroy callback that frees this thread's timer. No C++ reaches
into the timer or calls the `swoole_timer_*` API.
`crates/corebase/src/timer/wrapper.rs` provides these operations by Rust path over this
thread's timer slot; the raw callback and node-pointer exports are gone.
Corebase calls timer teardown, selection and coroutine timeout operations directly.

PHP reserves an empty node before retaining its Zend callable. A thread-confined
reservation cancels the node when dropped, and commits only to the timer generation
that issued it, using the full owner identity even when a packed `Held` generation
wraps. The PHP layer owns the callable and implements the safe callback
interface; its two-word inline storage and matching destruction remain private to
corebase. Callback storage is disjoint from metadata changed during reentry.
A removed reservation releases the retained callable without installing it, so no
borrow of a timer crosses a retention operation. A detached node releases its
callable before node deletion, and a deferred bailout runs after deletion.

A Zend bailout longjmps out of a callback or destructor over `select`'s Rust
frames, which hold nothing to drop. A C++ exception a callback or destructor
throws aborts at the first Rust frame (`panic = "abort"`), as an uncaught one
would.

`next_msec` is the armed-wakeup cache used by `reinit` and thread-manager
waits, not a second queue. Rust `end` computes the next delay.

Zend bailout: a fatal longjmp from a PHP callback used to leave selection active
and abort at shutdown. The PHP callback now marks the selection as abandoned
before propagating bailout; the owner destructor then uses the normal exact-ID
close/complete/end/drain path. Destroying an active, unabandoned selection is still rejected. Tested by
`tests/modules/timer/test_bailout.py`.

## Clock and backend helpers

Stateless helpers in `crates/corebase/src/timer/runtime.rs`, called from Rust only (`owner.rs`).

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
