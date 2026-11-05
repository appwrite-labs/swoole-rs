# Workers, processes and IPC boundary (M08)

Feature value 524288 (`--module process --module locks --module phpcoroutines`):
the Thread resource tables share the lock/atomic/barrier owner. PHP coroutine
entrypoints relay a pending whole-thread exit after returning through their Rust
owners; process selects that implementation explicitly. Every Thread-enabled
hybrid build requires `--module phpcoroutines`, including a build using the native
Process family. Native control builds retain their original module selection. Upstream targets:
`src/server/{worker,task_worker,process,manager,reactor_process}.cc` and
`src/os/{process_pool,pipe,signal,wait}.cc`. Production Server/Factory/Worker/ProcessPool methods and PHP bindings are preserved.
Worker/Pool and TLS WorkerGlobal fields are native views of canonical Rust records;
Server private storage and its command container implementation are Rust-owned.
Follows the [policy/effect split](abi-v1.md#policyeffect-split).

Rust implements state transitions, worker/task selection, progress/retry/error,
reload and exit policy. Native code executes the remaining syscalls, queue transport, native object
construction, timer/reactor operations, context switches, callbacks, exceptions
and Zend escapes. Rust owns shared Server/Port counter records and operations,
the TLS worker identity/request/stop state, and ThreadFactory slot custody. The
thread table releases its lock before native join callbacks; native thread body
and Zend bailout traps remain in their original native frames. Signedness, width, casts, wrapping, errno and last_error match
upstream. Exceptions: Rust makes the syscalls of worker CPU pinning, of reading a task's temporary file and of the signal table's sigaction and signalfd paths.

`ProcessPool`'s IPC is Rust (`crates/process/src/pool.rs`): the stream
listener's address, its accepted connection, the reply and packet buffers live
behind `ProcessPool::ipc_`, and the three worker loops (task, stream, message
protocols), round-robin scheduling and dispatch to a worker's pipe or queue run
in Rust over native hooks (`swr_v1_pool_hooks`) for the queue, the sockets, the
message bus, timers and callbacks. Hooks that can reach PHP run inside the
extension's `zend_try` trap; a bailout ends the loop, and its native caller
re-raises it once no Rust frame is left (`swr_v1_process_bailout`).

Rust owns the worker/PID registry, pool records and reload/IPC state. Shared
worker, connection/session and counter storage remains in its original shared
arena, with canonical Rust layouts and checked native views. Thread-local worker
state has no TLS destructor; worker stop explicitly releases a detached copy.
PID, worker ID, session ID and fd remain distinct identities. Worker direct
public fields and `Worker::*` member-pointer types remain source-compatible
with upstream; the native declarations expand canonical Rust record fields in
the Worker, Pool and WorkerGlobal classes themselves.

Server command names, ids, accepted process types, handler slots and outstanding
reply callbacks live in Rust tables. Native code constructs, invokes and destroys
`std::function` payloads after the registry/core orchestration calls return; no
registry borrow or lock spans callable work. Existing PHP callers retain their
caught bailout boundaries. The native member guard drains the
callable slots before the Rust base owner dies; no registry lock spans callbacks.
The production `add_command`, `command` and handler/reply methods remain public;
the three implementation `unordered_map` fields are replaced by this owner.

Static locations, ordered unique index names and MIME configuration also live in
Rust. The original public collection operations used by production and core gates
remain borrowed native views. Index order, binary names, optional empty versus
absent configuration and case-sensitive membership are preserved. Direct public
index-vector assignment/appends retain duplicates; the named index setter keeps
its original uniqueness policy. Native HTTP contexts retain strong Rust MIME
table references, so mutations remain visible, collection replacement preserves
the old context identity, and the table can outlive the Server facade.

- Callbacks may change settings or maps, close connections, dispatch again or
  destroy an event/coroutine; facts are refreshed at each original observation
  point.
- `fork` and `fork_exec` run natively with no Rust frame across the parent/child
  split. `fork_exec` failure returns pid 0 as upstream. Registry and configuration
  operations release their Rust locks before native fork paths run.
- Signal handlers stay async-signal-safe: no Rust allocation, I/O, formatting,
  mutex or unwinding in a signal frame. The one callback there is upstream's:
  the immediate handler runs `swoole_signal_callback` when the thread has no
  reactor. Masking and reentrant delivery order are as upstream.

Ordering kept from upstream:

- ProcessPool create/start partial resource publication; `onStart` before spawn;
  old PID erased before fork.
- Manager handles messages before timers; ProcessPool handles timers before
  messages.
- Reload snapshots with old/new worker overlap; abnormal-exit callback before
  respawn; timer kill before clear.
- Upstream's final wait loop breaks unconditionally on its first iteration, so
  it does not guarantee all children are reaped.

Original inventory: 287 Process/ProcessPool/Server PHPTs; corpus
`10-task-workers`. Tests must use isolated process groups, private files, IPC and
ports, and must not kill by name.

The pending user-worker sequence lives in the Server's existing Rust worker
registry. Its native view borrows the ordered pointers only until the next
configuration append; startup publishes IDs over the canonical worker records.
Resource construction, fork hooks and callback entrypoints remain native.

`SocketPairRecord` defines the inline pipe-pair storage. Generated native fields
and constructor assertions check every offset, size and alignment. Rust creates
the descriptors and configures the fixed nonlogging buffer/timeout leaves;
blocking changes, diagnostics, waits and socket destruction keep native frames.
