# Timer queue ABI v1

Declarations: `native/include/swoole_rs_timer.h` (15 exports). Feature value 4.
Except `create`, `release` and `ids`, each export wraps a `Timer` method: the
same name, but `select_next` for `next` and `stats` for `stats_get`. Callers:
`tests/modules/timer/kernel.cc` and `native/timer/swoole_timer.h`'s reads;
`swoole::Timer` (`crates/timer/src/owner.rs`, see
[ownership.md](../native/timer/ownership.md)) calls the methods.

## Ownership

Rust owns one indexed min-heap with ID lookup, scheduling fields, next ID,
selection round, captured time, active ID and closing state. `TimerNode`
objects are `swoole::Timer`'s (`crates/timer/src/owner.rs`); their callbacks and
destructors are plain C function pointers. The queue sees nodes only as tokens.
Time is injected as signed 64-bit relative monotonic milliseconds; Rust creates no
clock, runtime or thread.

- One owner thread per handle; no concurrent calls.
- Handles and outputs must be valid and aligned; outputs must not alias the
  queue or each other. Null handles/outputs return `INVALID`.
- The opaque non-null token is never dereferenced or freed by Rust. A successful
  add takes custody of one native node; only `RELEASE` or `RELEASE_OVERFLOW`
  hands it back. Get/next views are copied metadata.
- No export invokes a callback, destructor, Zend function or exception. No Rust
  panic or C++ exception crosses the boundary. Allocation is fallible and
  arithmetic is checked.

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

All view/stats outputs are zeroed on entry, including on failure, except where a
status returns valid metadata. Reserved fields are always zero. `next_delay`
defaults to -1. ID 0 means no active node; real IDs start at 1.

- **create/release**: create returns null on failure. Release accepts only an
  empty, non-selecting queue, otherwise `PENDING` and the queue stays usable.
  Release never disposes tokens.
- **add**: `now >= 0`, `delay > 0`, `persistent` 0 or 1, non-null token.
  Deadline is checked `now + delay`; interval = delay for repeats, 0 for
  one-shots. New entries have `exec_count` 0, the current birth round, removed 0.
  IDs span 1..`INT64_MAX`; exhaustion is `OVERFLOW`. Capacity is reserved before
  mutation, so `NOMEM` changes nothing logical. `CLOSING` rejects adds.
- **get**: absent ID is `NOT_FOUND`; IDs <= 0 are `INVALID`. An active removed
  entry stays visible and counted until complete.
- **update**: sets a nonnegative absolute deadline on a retained, non-removed
  ID, including the active one. Removed is `NOT_FOUND`, closing is `CLOSING`.
  No allocation. Birth round and interval are unchanged.
- **remove**: absent or already removed is `NOT_FOUND`. Inactive entries are
  detached with `RELEASE`. The active entry is marked removed and returns
  `DEFERRED`; its token is held until complete.
- **begin**: requires time >= 0, an open queue and no selection (nested begin is
  `BUSY`). Time may not go backwards relative to the last accepted add/begin
  (`INVALID`). Captures `now` once for the selection. Round `UINT64_MAX` is
  `OVERFLOW`.
- **next**: requires a selection and no active ID (`INVALID`/`BUSY`). A closing
  queue returns `CLOSING`. Returns `EMPTY` if the heap is empty, the head
  deadline is after the captured time, or the head was born in the current
  round (which blocks everything behind it, as upstream). Otherwise marks the
  entry active and returns its view. Equal deadlines fire in upstream's order:
  the heap ranks by deadline alone and sifts as `swoole::Heap` does, stopping on
  a tie, so three equal timers run A/C/B. Insertion order (A/B/C) woke
  coroutines in orders upstream never produces (api-fuzz, #2382; #2409).
- **complete**: requires the exact active ID, otherwise `INVALID` with no change.
  A removed entry, closing queue or one-shot detaches that ID (even if the
  callback moved it off the head) with `RELEASE`. A retained repeat advances
  from its current deadline until strictly after the captured time, coalescing
  missed intervals with quotient arithmetic, and increments `exec_count` once.
  If deadline or count would overflow, it is detached with `RELEASE_OVERFLOW`.
- **end**: requires a selection and no active ID. Clears the selection,
  increments the round, returns -1 if empty or closing, otherwise
  `max(1, head_deadline - captured_now)`.
- **stats**: count (including active removed), round, active, selecting, closing.
- **ids**: sets count to the retained count. Insufficient capacity returns
  `CAPACITY` without writing IDs; `NULL`/0 is a sizing query. IDs come in
  unspecified order. Callers snapshot IDs before running callbacks and
  resolve each again.
- **close**: idempotent; marks the active node removed; rejects add/update/begin
  but allows get/remove/stats/ids/complete/end/drain.
- **drain**: requires closing. Detaches one inactive entry (`RELEASE`), returns
  `EMPTY` when none remain, or `DEFERRED` if only the active entry remains.

## Differences from upstream

This contract completes by active ID, accepts active updates, rejects nested selection
and defers release on close. Deadline, ID and count arithmetic is checked.
