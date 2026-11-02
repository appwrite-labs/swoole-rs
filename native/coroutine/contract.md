# M06 coroutine core

Implements `docs/coroutine-abi-v1.md`. Selected with `SWOOLE_RS_COROUTINE`
(`--enable-swoole-rs-coroutine`); disabled selection keeps the original source
exactly. Header: `swoole_rs_coroutine.h`.

| Patch | Upstream files |
| --- | --- |
| 0001-channel | `include/swoole_coroutine_channel.h`, `src/coroutine/channel.cc` |
| 0003-context-stack-markers | `src/coroutine/context.cc` |
| 0004-coroutine-core | `include/swoole_coroutine.h`, `src/coroutine/base.cc` |
| 0005-context-bookkeeping | `include/swoole_coroutine_context.h`, `src/coroutine/thread_context.cc` |

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

`swr_v1_channel` is an owned Rust handle (capacity, closed flag, last error,
the data FIFO and both waiter queues of opaque pointers); `swoole::coroutine::Channel`
holds it and is not copyable. pop/push/close are whole Rust operations that call
`swr_v1_channel_effects` for the timeout timer, the yield (C++ builds the cancel
function and returns the canceled/timed-out bits), waking a waiter and the
trace points. Upstream's order holds: a waiter is queued before it yields and
removed (every entry, by the timer callback or cancel function) before it
resumes; after a wait the timer is deleted, then cancellation wins over timeout
over close; pop drains data after close; close wakes producers, then consumers,
re-reading each queue after every resume. A push/pop wake ends the operation:
the woken coroutine may delete the channel. Enqueue picks the producer queue
only for PRODUCER, removal the consumer queue only for CONSUMER (upstream's
asymmetric else branches). Allocation failure returns -5 and C++ throws
`std::bad_alloc`.

## Coroutine map

`Coroutine::coroutines` is upstream's `std::unordered_map<long, Coroutine *>`.
Its iteration order is unspecified upstream and differs between standard
libraries, so no Rust replica of one library's order is kept (#357).
