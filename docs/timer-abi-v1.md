# Timer queue ABI v1

The queue is `crates/timer`'s `Timer`, whose methods `swoole::Timer`
(`crates/timer/src/owner.rs`, see [ownership.md](../native/timer/ownership.md))
calls. Feature value 4. It exports no C symbols. The operations below are the methods, named as before:
`next` is `select_next` and `stats` is `stats`. `crates/timer/src/kernel.rs` holds
what the retired C++ harness (`kernel.cc`) checked, one test per topic.

## Ownership

Rust owns one indexed min-heap with ID lookup, scheduling fields, next ID,
selection round, captured time, active ID and closing state. `TimerNode`
objects are `swoole::Timer`'s (`crates/timer/src/owner.rs`); their callbacks and
destructors are plain C function pointers. The queue sees nodes only as tokens.
Time is injected as signed 64-bit relative monotonic milliseconds; Rust creates no
clock, runtime or thread.

- One owner thread per handle; no concurrent calls.
- The opaque non-null token is never dereferenced or freed by Rust. A successful
  add takes custody of one native node; only `Release` or `ReleaseOverflow`
  hands it back. Get/next views are copied metadata.
- No operation invokes a callback, destructor, Zend function or exception.
  Allocation is fallible and arithmetic is checked.

Native callbacks run only between `next` and `complete`. Native destructors run
only after a Rust call has detached and returned the token; they may remove
other IDs or add timers while the queue is open. IDs are never reused. The
caller must not pass a token twice, delete it while retained, or delete the
Timer during an active callback: teardown during a callback means request
`close`, then delete after the callback completes and selection ends.

Fork: the owner records its creator PID. Destroying a copy in another
process may close, complete the copied active ID, end the copied selection,
dispose detached tokens and drain/release. The parent is unaffected.

Zend bailout: the PHP callback marks the selection abandoned and
propagates. Owner destruction then uses the same close/complete/end/drain/
release sequence, keeping timer metadata visible to PHP shutdown until teardown.
No callback, Zend jump or bailout continuation runs under a Rust frame
(`tests/modules/timer/test_bailout.py`).

## Operations

Each returns a [`Status`](../crates/timer/src/lib.rs). A failing operation
returns the default (zeroed) view; only statuses that carry metadata return a
filled one. Reserved fields are always zero. `end`'s delay defaults to -1. ID 0
means no active node; real IDs start at 1.

- **idle**: an empty, non-selecting queue, the only one its owner frees. Freeing
  never disposes tokens.
- **add**: `now >= 0`, `delay > 0`, `persistent` 0 or 1, non-null token.
  Deadline is checked `now + delay`; interval = delay for repeats, 0 for
  one-shots. New entries have `exec_count` 0, the current birth round, removed 0.
  IDs span 1..`INT64_MAX`; exhaustion is `Overflow`. Capacity is reserved before
  mutation, so `NoMem` changes nothing logical. `Closing` rejects adds.
- **get**: absent ID is `NotFound`; IDs <= 0 are `Invalid`. An active removed
  entry stays visible and counted until complete.
- **update**: sets a nonnegative absolute deadline on a retained, non-removed
  ID, including the active one. Removed is `NotFound`, closing is `Closing`.
  No allocation. Birth round and interval are unchanged.
- **remove**: absent or already removed is `NotFound`. Inactive entries are
  detached with `Release`. The active entry is marked removed and returns
  `Deferred`; its token is held until complete.
- **begin**: requires time >= 0, an open queue and no selection (nested begin is
  `Busy`). Time may not go backwards relative to the last accepted add/begin
  (`Invalid`). Captures `now` once for the selection. Round `UINT64_MAX` is
  `Overflow`.
- **next**: requires a selection and no active ID (`Invalid`/`Busy`). A closing
  queue returns `Closing`. Returns `Empty` if the heap is empty, the head
  deadline is after the captured time, or the head was born in the current
  round (which blocks everything behind it, as upstream). Otherwise marks the
  entry active and returns its view. Equal deadlines fire in upstream's order:
  the heap ranks by deadline alone and sifts as `swoole::Heap` does, stopping on
  a tie, so three equal timers run A/C/B. Insertion order (A/B/C) woke
  coroutines in orders upstream never produces (api-fuzz, #2382; #2409).
- **complete**: requires the exact active ID, otherwise `Invalid` with no change.
  A removed entry, closing queue or one-shot detaches that ID (even if the
  callback moved it off the head) with `Release`. A retained repeat advances
  from its current deadline until strictly after the captured time, coalescing
  missed intervals with quotient arithmetic, and increments `exec_count` once.
  If deadline or count would overflow, it is detached with `ReleaseOverflow`.
- **end**: requires a selection and no active ID. Clears the selection,
  increments the round, returns -1 if empty or closing, otherwise
  `max(1, head_deadline - captured_now)`.
- **stats**: count (including active removed), round, active, selecting, closing.
- **ids** (test-only): returns the retained count. A slice shorter than it returns
  `Capacity` without writing IDs; an empty slice is a sizing query. IDs come in
  unspecified order. Callers snapshot IDs before running callbacks and
  resolve each again.
- **close**: idempotent; marks the active node removed; rejects add/update/begin
  but allows get/remove/stats/ids/complete/end/drain.
- **drain**: requires closing. Detaches one inactive entry (`Release`), returns
  `Empty` when none remain, or `Deferred` if only the active entry remains.

## Differences from upstream

This contract completes by active ID, accepts active updates, rejects nested selection
and defers release on close. Deadline, ID and count arithmetic is checked.
