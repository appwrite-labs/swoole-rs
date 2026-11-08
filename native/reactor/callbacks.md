# Reactor callbacks and builtin scheduling

## Ordered end/exit callbacks

The selected Reactor replaces both private ordered callback maps with one lazy
Rust owner holding two sorted vectors of integer keys and opaque Native
`std::function` payload tokens. Kind 0 is end, kind 1 is exit. There is no
Native map mirror. Public methods and the fallback stay intact; the private
class layout changes, so old Native binary layout compatibility is not claimed.

Entry points: lookup, fallible insertion, detach, the resume index after a
changed walk, drain and empty-owner release, plus two orchestration operations:

- `callbacks_ensure`: lookup, empty-payload allocation request, insertion,
  commit and discard/requery. Native allocates the typed payload, passes its
  identity on the next call, and transfers ownership only on commit. Empty
  publication precedes Native function copy assignment (the `map[]` prefix). A
  discarded candidate is destroyed after Rust returns and lookup runs again.
- `callbacks_clear_next`: chooses each detached payload's type; Native deletes
  it. At the end Rust releases the empty metadata and publishes the owner slot
  as null.

Traversal: the owner starts with one view per kind (entries pointer and
length), which Rust republishes after every change, so Native walks the sorted
entries without a call per callback (#680). After each callback Native re-reads
the view: when the entry it ran is still at its index the walk steps on,
otherwise `callbacks_after` gives the first index past its key. A later-key
insertion is seen, a future erasure disappears and an earlier-key insertion is
not revisited. Exit returning false stops traversal. The same event-count
reference survives the exit traversal. Copying walks the source's end view then
its exit view, assigning each typed copy; the copy constructor's
catch/clear/rethrow and copy/swap assignment stay Native, and copy assignment
uses a temporary metadata owner rather than map node reuse. Erase detaches
metadata before Native destroys the payload. Capacity refusal returns a status;
Native throws `bad_alloc` after return. Member cleanup order stays sockets,
exit callbacks, end callbacks.

Internally the container makes one opaque-owner allocation and two empty `Vec`
seeds, reserves fallibly before sorted insertion, and publishes a new owner only
after successful insertion. Internal operations do not re-enter the exported C
ABI.

## Handlers

`set_handler`, `isset_handler`, `get_handler` and `get_error_handler` are Rust
tables (`swr_v1_reactor_handlers_*`, `crates/reactor/src/handlers.rs`).

## Builtin begin/idle/exit scheduling

Rust owns the policy of `execute_begin_callback`, the idle end callback,
try-exit, and the deferred/default exit predicates, through one scalar
transition ABI. The reply is an 8-byte C value (phase at offset 0, action at
4). Roles: idle/future, try-exit, deferred exit, default exit. Actions: task
invocation, exit query, stopped publication, Boolean result. Invalid replies
abort in the Native adapter. Try-exit observes `wait_exit` once before
`if_exit` and does not re-read it. Trim/time and signal callbacks stay Native.

## Deferred queues

`defer_tasks` and `destroy_callbacks` are fields of the Rust reactor state:
FIFO queues of C `(call, data, dispose)` entries (`crates/reactor/src/deferred.rs`),
created on first push. Rust callers (`Socket::free`) push a function pointer
directly. `Reactor::defer` and `add_destroy_callback` box the `std::function`
with its data and push a trampoline that runs and frees it; `dispose` frees a box
whose task never ran.

- The `PRIORITY_DEFER_TASK` end callback detaches `defer_tasks`, runs it and
  frees it, so a task that defers again lands on the next end pass. A non-null
  `defer_tasks` makes the kernel wait non-blocking.
- The Reactor destructor runs `destroy_callbacks` until empty (callbacks it
  adds run too); the state frees the queue.
