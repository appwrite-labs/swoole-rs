# M06 coroutine core

Implements `docs/coroutine-abi-v1.md`. Selected with `SWOOLE_RS_COROUTINE`
(`--enable-swoole-rs-coroutine`); disabled selection keeps the original source
exactly. Headers: `swoole_rs_coroutine.h`, `swoole_rs_channel_tokens.h`.

| Patch | Upstream files |
| --- | --- |
| 0001-channel-policy | `src/coroutine/channel.cc` |
| 0002-channel-owned-tokens | `include/swoole_coroutine_channel.h` |
| 0003-context-stack-markers | `src/coroutine/context.cc` |
| 0004-coroutine-core | `include/swoole_coroutine.h`, `src/coroutine/base.cc` |
| 0005-context-bookkeeping | `include/swoole_coroutine_channel.h`, `include/swoole_coroutine_context.h`, `src/coroutine/channel.cc`, `src/coroutine/thread_context.cc` |

Rust frames never span a swap callback, context switch, timer, allocation,
print or destructor: C++ runs those between calls.

## Scheduler and coroutine state

`crates/coroutine/src/coroutine.rs` owns two `repr(C)` records:
`swr_v1_coroutine_scheduler` (current coroutine, last and socket-bound ids, peak
count, stack size, the three swap callbacks, activation) and `swr_v1_coroutine`
(id, state, resume code, task, cancel function, the origin tree and timing).
C++ stores the scheduler in upstream's `SW_THREAD_LOCAL` scope (a plain global
in NTS builds, so `get_current()` stays one load) and embeds one record in each
`swoole::Coroutine`, whose `native` field points back at it. C++ getters read
fields inline; every write is a Rust call.

`swoole::Coroutine` is a facade: run, resume, yield and close call
`enter`/`wake`/`suspend`/`leave`/`end`/`release` around the callback and the
switch, in upstream's order (the resume callback still sees the old current;
yield clears the resume code before its callback). `release` restores current,
orphans the coroutines this one resumed and unlinks it from its origin. Cancel
copies the `std::function` before `swr_v1_coroutine_cancel` clears it. The
bailout callback, `on_bailout`, stays a C++ `std::function`.

## Context stack

The stack pointer, `uint32` extent and completion byte are inline Rust cells
with the original widths, alignment and offsets. Context is 80 B / align 8
(128 B with the thread backend; Coroutine 192 B there).

- Extent conversion stays at its field initialization after `fn` is moved.
- `swr_v1_context_prepare` raw-writes completion at the first constructor
  statement, then reads the extent for allocation; the thread backend uses a
  null extent. ADMIT writes the pointer only after allocation (original
  null-only check).
- The requested size and the stored `uint32` extent differ: `uc_stack.ss_size`
  and marker initialization use the full argument; allocation, top and query use
  the extent.
- Destruction: observe pointer, Native logging/valgrind, re-read pointer/size for
  Native free, then CLEAR writes null and reads nothing more. The getcontext-error
  free path still has no clear (upstream behavior).
- Function entry is followed by completion publication, then swap/unlock.

Stack markers: `swr_v1_context_stack_mark` writes the original magic bytes;
`swr_v1_context_stack_usage` reads only initialized marker spans, never a slice
over the uninitialized allocation. Initialization includes the sample at
`offset == size`; queries exclude it (original `<=` vs `<`). The initial 64 KiB
estimate and the doubled last changed sample (which can exceed the allocation)
are kept.

The ucontext-derived SDK fails to build under `-Werror` because of an unused
`sp` in unchanged Native code.

## Channel

`swr_v1_channel_step` borrows an aligned caller-local four-byte cursor for one
call (null is invalid; aliasing cannot be checked). Operations: push 0, pop 1,
close 2. Facts are copied fresh on each return. Actions: timer, queue transfer,
yield, wake or closed publication; negative errors match Channel errors. Phase
6 is terminal; invalid operation/phase returns -4. The operation is fixed for a
cursor's lifetime. Rust owns wait/fairness/error precedence and
producer-before-consumer close order. A Native timer delete completes before
cancellation state is re-read; a timed-out yield returns before deleting its
expired timer. Successes leave `error_` untouched.

A push/pop WAKE is terminal: Native returns the captured result right after
resume without touching the Channel, because the resumed waiter may delete it.
Close wakes are nonterminal; the Channel must stay alive through the next queue
check.

Storage: the data FIFO and both waiter lists are three opaque Rust handles, each
one `VecDeque` of pointer identities (null payloads allowed; front status
distinguishes empty). Construction, clone and growth are fallible; Native throws
`std::bad_alloc` after return. Implicit copy construction/assignment copy
identities into independent queues. Capacity/closed/error live in one
initialized POD prefix (16 B, align 8); selected Channel is 40 B with handles at
16/24/32 (Native 112 B). Capacity 0 is not normalized, success keeps sticky
errors, and closing an already-closed channel keeps its previous error.

`swr_v1_channel_owned_step` runs the same phase/fairness kernel directly on the
Rust queues; only fresh timer/cancel/timeout flags cross in. Close returns
PUBLISH_CLOSED first; Native traces before the next call publishes closure.
Rust removes a waiter before returning its identity. Allocation refusal returns
-5 at the original transfer point. `swr_v1_channel_pop_data` removes an optional
token in one call (a stored null still counts as data).

`swr_v1_channel_waiter`: enqueue and pop use the producer queue only for role 1;
removal uses the consumer queue only for role 2 (preserving the original
asymmetric else branches). Removal erases all equal identities. `Channel::yield`
enqueues before the trace and cancel-closure construction; cancellation removes
before resume. The timer callback publishes `msg.error=true`, then
`msg.timer=nullptr`, removes, then re-reads `msg.co` for resume.

## Coroutine map

`Coroutine::coroutines` is upstream's `std::unordered_map<long, Coroutine *>`.
Its iteration order is unspecified upstream and differs between standard
libraries, so no Rust replica of one library's order is kept (#357).
