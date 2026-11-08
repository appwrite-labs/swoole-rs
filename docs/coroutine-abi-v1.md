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
- `cancel` copies the cancel function before clearing it, sets `RC_CANCELED`
  before invoking it, and does not touch the coroutine afterwards.
- `yield_ex` deletes its timer only after a wake that was not the timeout, then
  reports cancellation.
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

The context is Rust (`src/coroutine/context.cc` and `swoole_asm_context.h` left
the build): stack allocation, the guard page, marker initialization and
stack-usage sampling (original marker bytes and placement; `<=` on init versus
`<` on query), boost.context's `swoole_make_fcontext`/`swoole_jump_fcontext`, and
the ASan fiber annotations, which run whenever `dlsym` finds the sanitizer
runtime. `swoole::coroutine::Context` is an inline view that keeps the
`CoroutineFunc` and calls it from Rust's entry. ucontext and valgrind stack
registration are gone: config.m4 rejects a CPU without boost asm unless
`--enable-thread-context` is given.

`src/coroutine/base.cc` left the build: Rust defines `Coroutine::yield_ex`,
`cancel`, `print_list`, `print_socket_bound_error`, `coroutine::run` and the
`swoole_coroutine_*` C API under their upstream symbols; native/coroutine keeps
the thread-local scheduler, the facade's construction and delete, and the
`std::function` arguments.

Execution-time accounting (`get_execute_usec`, `calc_execute_usec`) is in Rust;
C++ reads the clocks and passes them in.

Not validated: `SW_USE_THREAD_CONTEXT` (its cross-owner `std::mutex` hand-off is
not a valid Rust ownership model). Original inventory: 130
coroutine, 44 channel and 27 scheduler PHPTs.
