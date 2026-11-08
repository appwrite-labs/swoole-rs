# Coroutine scheduler and channel boundary (M06)

Feature value 32768. Upstream targets:
`src/coroutine/{base,context,thread_context,channel}.cc` and their headers.
Follows the [policy/effect split](abi-v1.md#policyeffect-split).

Rust owns the scheduler (current coroutine, ids, peak, stack size, swap
callbacks, activation) and each coroutine's state (id, state, resume code, task,
cancel function, origin tree, timing) as `repr(C)` records that C++ stores in
upstream's scopes and reads inline; `swoole::Coroutine` is a facade whose writes
are Rust calls. The coroutine map (`Coroutine::coroutines`, whose iteration
order `Coroutine::list()` exposes) stays native.
Context switches, timer operations, callbacks, allocation, diagnostics and
unwinding run natively between Rust calls; Rust has returned before
`swap_in`/`swap_out`, bailout, `on_resume`/`on_yield`/`on_close` and waking
channel waiters.

Sequence points kept exactly:

- `run` saves the CID before possible synchronous deletion.
- `resume` runs its callback before changing current/origin.
- `yield` publishes `WAITING`/`RC_OK` before its callback.
- `cancel` copies the native function before clearing it, sets `RC_CANCELED`
  before invoking it, and does not touch the coroutine afterwards.
- `close` calls `on_close` before registry removal, current restore and delete.
- `SW_CORO_BAILOUT_EXIT_CODE` is used by name.

Channel:

- Fairness includes older producers and consumers, not just occupancy.
- A waiter is removed before it is resumed. Close publishes closed, wakes
  producers then consumers, rereading queues after each resume.
- Timeout callbacks set the stack message error, clear the timer and remove all
  matching entries before resume. After a wait, the timer is deleted before
  checking cancellation, timeout and close; cancellation wins.
- Pop may drain buffered data after close.
- The channel is all Rust (`src/coroutine/channel.cc` and its header left the
  build); pop/push/close run the host's coroutines through an `Effects` trait,
  which crates/phpcollections implements for `Swoole\Coroutine\Channel`. Rust
  never dereferences or frees payloads.

`context.cc`: stack allocation, the guard page, marker initialization and
stack-usage sampling are in Rust (original marker bytes and placement; `<=` on
init versus `<` on query). Valgrind registration, the ASan fiber annotations and
the context-switch ABI (boost.context asm or ucontext) are native.

Execution-time accounting (`get_execute_usec`, `calc_execute_usec`) is in Rust;
C++ reads the clocks and passes them in.

Not validated: `SW_USE_THREAD_CONTEXT` (its cross-owner `std::mutex` hand-off is
not a valid Rust ownership model) and macOS ucontext. Original inventory: 130
coroutine, 44 channel and 27 scheduler PHPTs.
