# M06 coroutine core

Implements `docs/coroutine-abi-v1.md`. Selected with `SWOOLE_RS_COROUTINE`
(`--enable-swoole-rs-coroutine`); disabled selection keeps the original source
exactly. Header: `swoole_rs_coroutine.h`.

| Patch | Upstream files |
| --- | --- |
| 0001-channel | `include/swoole_coroutine_channel.h`, `src/coroutine/channel.cc` |
| 0003-context-stack-markers | `src/coroutine/context.cc` |
| 0004-coroutine-core | `include/swoole_coroutine.h`, `src/coroutine/base.cc` |
| 0005-context-asan-fibers | `include/swoole_coroutine_context.h` |

Rust frames that span a swap callback, context switch, timer or destructor
own nothing with a destructor, so a stack the bailout abandons, or PHP's longjmp
out of a callback, skips no cleanup.

## Scheduler and coroutine state

`crates/coroutine/src/coroutine.rs` owns two `repr(C)` records:
`swr_v1_coroutine_scheduler` (current coroutine, last and socket-bound ids, peak
count, stack size, the three swap callbacks, the C++ hooks, the live map, gdb's
cursor, activation and the bailout mark) and `swr_v1_coroutine` (id, state,
resume code, task, cancel function, the origin tree and timing). C++ stores the
scheduler in upstream's `SW_THREAD_LOCAL` scope (a plain global in NTS builds,
so `get_current()` stays one load) and embeds one record in each
`swoole::Coroutine`, whose `native` field points back at it. C++ getters read
fields inline; every write is a Rust call.

The live coroutines (`live.rs`, upstream's `Coroutine::coroutines`) are a map by
id that the scheduler points to, allocated with the first coroutine and freed
on deactivation once empty; no lookup goes through a thread-local. Lists run
newest first.

`swoole::Coroutine` is a facade: run, resume, yield (with or without a cancel
function) and bailout are whole Rust operations, in upstream's order (the
resume callback still sees the old current; yield clears the resume code before
its callback). They call back through `swr_v1_coroutine_hooks`, which C++ sets in
the scheduler's initializer: `swap_in` (returns whether the coroutine ended),
`swap_out`, `destroy` (the stack-usage trace and `delete`), `bailout` (calls
`on_bailout`) and, under `SW_CORO_TIME`, `clock`. After a switch in, an ended
coroutine closes: the close callback, then `release` restores current, orphans
the coroutines this one resumed, unlinks it from its origin and drops it from
the live map, then `destroy`. Otherwise a set bailout mark runs the bailout
hook. The bailout callback, `on_bailout`, stays a C++ `std::function`; the
scheduler's `bailout` flag mirrors whether it is set. Cancel copies the
`std::function` before `swr_v1_coroutine_cancel` clears it; `yield_ex`'s timer
stays C++.

## Context stack

`swr_v1_context_stack_alloc(size, flags)` allocates a coroutine stack with
`malloc`, or with `SWR_CTX_STACK_PROTECT` (`SW_CONTEXT_PROTECT_STACK_PAGE`) as an
anonymous mapping (plus `MAP_STACK` on OpenBSD) whose lowest page is made
inaccessible; it returns null on failure, which C++ reports as upstream does.
`SWR_CTX_STACK_MARK` (`SW_CONTEXT_DETECT_STACK_USAGE`) writes upstream's magic
markers before the context is made: at `size - offset + 31` for each doubling
`offset` from 64 KiB while `offset <= size`. `swr_v1_context_stack_usage`
samples them while `offset < size` and reports twice the last changed offset,
or 64 KiB. `swr_v1_context_stack_free` takes the same size and flags. The
Context fields, the switch (boost.context asm or ucontext), valgrind
registration and the ASan fiber annotations stay C++. The thread backend is
upstream's, unchanged.

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
