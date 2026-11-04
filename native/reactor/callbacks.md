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

`set_handler`, `isset_handler`, `get_handler` and `get_error_handler` are
upstream's code (#336).

## Builtin defer/begin/idle/exit scheduling

Rust owns the policy of `Reactor::defer`, `execute_begin_callback`, the deferred
and idle end callbacks, try-exit, and the deferred/default exit predicates,
through one scalar transition ABI. The reply is an 8-byte C value (phase at
offset 0, action at 4). Roles: enqueue, deferred end, idle/future, try-exit,
deferred exit, default exit. Actions: allocate/publish, append, detach, execute,
delete, task invocation, exit query, stopped publication, Boolean result.
Invalid replies abort in the Native adapter.

Enqueue publishes the new manager before append. Deferred execution captures
the old manager, clears the Reactor field, executes, then deletes it. Callbacks
that defer again therefore schedule a new manager for the next end pass.
Try-exit observes `wait_exit` once before `if_exit` and does not re-read it. The
event-count predicate receives a checked scalar. Trim/time and signal callbacks
stay Native.

## Deferred queue ownership

`defer_tasks` is a `ReactorDeferredQueue` pointer and the embedded
`destroy_callbacks` a typed queue; the public `CallbackManager` class is
unchanged but no longer used by the Reactor. The facade holds one opaque Rust
owner; Rust owns FIFO order and Native Task identities, with no Native list.

- Native allocates/copies the `std::function`/data pair. Rust admission is
  fallible and takes token custody only on success; Native throws `bad_alloc`
  after return. The payload copy may precede an allocation refusal.
- Execution: Rust selects the front token and returns; Native copies it onto
  the stack. A second call detaches the current front; Native then deletes,
  invokes and drops the scoped copy. The next queue observation follows these
  effects. No extra per-invocation heap allocation.
- `add_destroy_callback` and the Reactor destructor's execute call use the same
  FIFO, with unchanged statement text.
- Empty-owner release never drops or invokes a payload in Rust.

Before the registry fold, the captured SDK measured the Reactor at 1008 B with
the owned queue (Native 1064 B). `destroy_callbacks` stays at offset 840 but
shrinks 24→8 B; later public offsets shift by 16 B. The Reactor is not
copy-constructible, copy-assignable or movable.

## Shared owner allocation

Socket registries, snapshots, callback metadata, deferred queues, poll storage
and backend event arrays share one private fallible boxed-owner helper. It
allocates `Layout<T>` with the global allocator before initialization and wraps
it in a `Box`. Vector reservation precedes owner allocation; lazy owners skip
allocation; new owners publish only after their operation succeeds and commits.
All admitted owner types are nonzero-sized and have no Native or custom `Drop`.
An RAII capsule frees the allocation if an initializer panics.
