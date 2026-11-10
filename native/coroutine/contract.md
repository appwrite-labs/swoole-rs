# M06 coroutine core

Header: `swoole_rs_coroutine.h`.

| Patch | Upstream files |
| --- | --- |
| 0001-channel, …-channel-cc-and-swoole-co | `include/swoole_coroutine_channel.h`, `src/coroutine/channel.cc` (deleted) |
| 0003-context-stack-markers, …-context-is-rust | `src/coroutine/context.cc`, `include/swoole_asm_context.h` (deleted) |
| 0004-coroutine-core, …-coroutine-operations-are-rust | `src/coroutine/base.cc` (deleted) |
| phpcoroutines' …-swoole-coroutine-h-swoole-coroutine-co | `include/swoole_coroutine{,_context,_api,_system}.h` (deleted) |

`swoole_coroutine.h` here is an opaque `Coroutine` with the three inline getters
native/ still calls, and `swoole_coroutine_system.h` and `swoole_coroutine_api.h` the `System` members and
`swoole_coroutine_*` exports, each keeping only what native/, the vendored code
and the compiled core-tests call. The deletion is in phpcoroutines' patch set
because hooks' and phpcoroutines' patches edit the headers first.

Rust frames that span a swap callback, context switch, timer or destructor
own nothing with a destructor, so a stack the bailout abandons, or PHP's longjmp
out of a callback, skips no cleanup.

## Scheduler and coroutine state

`crates/coroutine/src/coroutine.rs` owns two `repr(C)` records:
`swr_v1_coroutine_scheduler` (current coroutine, last and socket-bound ids, peak
count, stack size, the three swap callbacks, the C++ hooks, the live map, gdb's
cursor, activation and the bailout mark) and `swr_v1_coroutine` (id, state,
resume code, task, cancel function, the origin tree and timing). Rust keeps the
scheduler in upstream's `SW_THREAD_LOCAL` scope (`thread_local!` under ZTS, a
plain static otherwise; `swr_v1_coroutine_scheduler_get`) and allocates each
`swoole::Coroutine` as a `swr_v1_coroutine` followed by its `Context`
(`context::Native`), whose `native` field points back at it. C++ getters read
fields inline; every write is a Rust call.

The live coroutines (`live.rs`, upstream's `Coroutine::coroutines`) are a map by
id that the scheduler points to, allocated with the first coroutine and freed
on deactivation once empty; no lookup goes through a thread-local. Lists run
newest first.

Create, run, resume, yield (with or without a cancel function), the timed
`yield_ex`, cancel and bailout are whole Rust operations, in upstream's order (the
resume callback still sees the old current; yield clears the resume code before
its callback). They call back through `swr_v1_coroutine_hooks`: `swap_in` (returns
whether the coroutine ended), `swap_out`, `destroy` (the stack-usage trace, then
the context and record are freed), `bailout` (`on_bailout`, a C function and its
data) and, under `SW_CORO_TIME`, `clock`. After a switch in, an ended
coroutine closes: the close callback, then `release` restores current, orphans
the coroutines this one resumed, unlinks it from its origin and drops it from
the live map, then `destroy`. Otherwise a set bailout mark runs the bailout
hook. `swr_v1_coroutine_bail(callback, data)` is `Coroutine::bailout`: inside a
coroutine it sets the hook, jumps to the main context and exits with
`SW_CORO_BAILOUT_EXIT_CODE` if that returns; outside one it marks the scheduler.

A cancelable yield's `cancel_fn` points at a `swr_v1_coroutine_canceler` (a C
function and its data) on the yielder's stack. Cancel (`Coroutine::cancel`'s
symbol) refuses a coroutine that is not suspended with one, then calls it.
`yield_ex` adds its timeout timer through
`swr_v1_timer_add_plain_sec` (crates/timer, a C callback and data) and deletes it after a
wake that was not the timeout.

`src/coroutine/base.cc` is gone. `crates/coroutine/src/scheduler.rs` keeps the
scheduler, `swr_v1_coroutine_{scheduler_get,bail}` and the operations Rust calls by
path (`current`, `resume`, `yield_with_cancel`), which need no timer or event loop,
so they build in every configuration. `crates/coroutine/src/facade.rs`, which only
the extension builds (`upstream-symbols`), defines the rest under their upstream
symbols (`yield_ex`, `cancel`, `activate`, `deactivate`, `print_list`,
`print_socket_bound_error`), `swr_v1_coroutine_create` and the `swoole_coroutine_*`
C API. The configure switches it reads (`SW_USE_THREAD_CONTEXT`,
`SW_CONTEXT_PROTECT_STACK_PAGE`, `SW_CONTEXT_DETECT_STACK_USAGE`, `SW_CORO_TIME`) are
`swoole_rs_corebase::built_with`'s (native/corebase/switches.cc), so the module has
no native source left.

## Context stack

`swr_v1_context_stack_alloc(size, flags)` allocates a coroutine stack with
`malloc`, or with `STACK_PROTECT` (`SW_CONTEXT_PROTECT_STACK_PAGE`) as an
anonymous mapping (plus `MAP_STACK` on OpenBSD) whose lowest page is made
inaccessible; it returns null on failure, which context init reports as upstream does.
`STACK_MARK` (`SW_CONTEXT_DETECT_STACK_USAGE`) writes upstream's magic
markers before the context is made: at `size - offset + 31` for each doubling
`offset` from 64 KiB while `offset <= size`. `swr_v1_context_stack_usage`
samples them while `offset < size` and reports twice the last changed offset,
or 64 KiB. `swr_v1_context_stack_free` takes the same size and flags.

`crates/coroutine/src/context.rs` owns the `Context` record that follows
each coroutine's `swr_v1_coroutine`: init
allocates the stack and makes the boost.context fcontext (out of memory is
upstream's fatal error and exit 254), swap_in/swap_out jump with
`swoole_jump_fcontext`, and the first frame runs the coroutine's C routine, sets `end` and switches out. ASan's fiber annotations run
when `dlsym` finds `__sanitizer_{start,finish}_switch_fiber`. ucontext and
valgrind stack registration are not kept; config.m4 fails on a CPU without
boost asm, and links it with the thread backend too, since the Rust context
references it either way.

With `THREAD` (`--enable-thread-context`) the record has no stack and
`fctx` owns a thread (`crates/coroutine/src/thread.rs`; `thread_context.cc` is
gone). A switch hands a baton, a binary semaphore, from one thread to the next,
where upstream unlocks `std::mutex`es from threads that didn't lock them. init
returns the errno of a failed spawn, which create reports as a warning and -1. `activate` adds the keep-alive timer and async threads as
upstream did, then `swr_v1_thread_context_init` records the reactor, timer,
buffer stack and async threads that each coroutine thread adopts once it first
holds the baton. The `threadcontext` workflow compares the coroutine PHPT
families against upstream built the same way.

## Channel

`crates/coroutine/src/chan.rs` is the whole Channel; `src/coroutine/channel.cc`
and its header are gone. pop/push/close
run the host's coroutines through the `Effects` trait: crates/phpcollections
implements it over PHP's coroutines (timer, cancelable yield, resume), and the
crate's tests over a thread-per-coroutine model that runs upstream's
`coroutine_channel` GoogleTests (tests/modules/coroutine/channel.rs). Upstream's
order holds: a waiter is queued before it yields and removed (every entry, by
the timer callback or cancel function) before it resumes; after a wait the timer
is deleted, then cancellation wins over timeout over close; pop drains data
after close; close wakes producers, then consumers, re-reading each queue after
every resume. A push/pop wake ends the operation: the woken coroutine may delete
the channel. Enqueue picks the producer queue only for PRODUCER, removal the
consumer queue only for CONSUMER (upstream's asymmetric else branches).
Upstream's `SW_TRACE_CHANNEL` trace logs, compiled only into trace builds, are
not kept.

## Coroutine map

`Coroutine::coroutines` is upstream's `std::unordered_map<long, Coroutine *>`.
Its iteration order is unspecified upstream and differs between standard
libraries, so no Rust replica of one library's order is kept (#357).
