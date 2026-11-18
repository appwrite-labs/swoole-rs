# Process tests

`corebase-unit` runs `crates/corebase/src/process`'s tests and `process-signals` runs `signals.py`.
The process state and operations now belong to corebase; the standalone crate and
internal logger host dispatch are gone. Reactor signal cleanup and TLS listener
callbacks retain their external owners. Corebase's `thread` feature preserves the
worker globals used by the original SW_THREAD server cases, and its shared PHP
configuration detector preserves ZTS builds.

Fork and signal tests share the process signal-state lock in the combined binary.
The logger test that requires no process logger runs in a private child, retaining
its original assertions; Miri cannot spawn that child and reports it ignored.
This keeps the process tests' capture logger from changing its log threshold.
`core-tests/src/os/process_pool.cpp` and `os/pipe.cpp` are Rust cases in
`hooks-system-rust` (tests/modules/hooks/README.md), and the Server worker-retirement
witness is crates/server's `worker_methods::tests::abort_and_reset`.

`native_methods.rs` calls the public pool methods with the existing scripted
`PoolNative`, without linking the server or registering a global host. It covers
protocol/type/request settings, start-id narrowing, allocation refusal and the
missing-listener error. Production callers pass the server's native effects
explicitly; there is no test-only replacement for method routing.

`pool_owners.rs` checks reentrant bus ownership and IPC visibility across processes.
The IPC regression places the pool record in shared memory and uses a pipe to make
a child inspect it while the parent borrows its own IPC state. The child must still
find IPC; borrowing must not temporarily clear the shared pool's pointer.

Pool pipe creation, send-buffer queries, message-bus allocation, pipe registration
and stop-message publication use typed methods across the process/server boundary.
The scripted host retains the original operation logs and integer test inputs;
those five Op variants exist only in tests. Regressions cover send-buffer fallback
and clamping, failed allocation ownership, failed detach preserving running state,
and the async loop's deliberately ignored pipe-registration failure.

`corebase::sys::waitpid` and both Rust wait registries distinguish a reaped PID,
`WNOHANG` with no ready child, and a captured OS error. Corebase regressions use a
pipe-controlled child to check pending/exit/status behavior and error capture.
The original process/reaper assertions retain their integer observations only at
the test or C boundary; manager service/reload ordering and timeout policy stay intact.

`worker_owner.rs` (server crate unit tests) checks that the user-worker registry
retains a Process's record, pipe and private queue until server teardown. It also
checks that a thread's record copy retains those native resources, explicit close
does not close a reused descriptor at drop, and a plain record copy does not adopt
borrowed allocations. The existing PHP Process queue tests retain their inputs
and assertions through the shared native owner.

The task packet and temporary-file GoogleTests from v6.2.6 run as Rust tests in
`task_packets.rs`. Each keeps the upstream test name.

| Upstream suite | Rust tests |
| --- | --- |
| `server/task_packet.cpp` | `inline_lengths_must_fit_storage`, `file_header_lengths_must_match`, `file_paths_must_have_a_terminator`, `valid_file_packets_can_be_peeked_then_consumed` |
| `server/task_tmpfile.cpp` | `failed_writes_leave_no_files`, `successful_inline_and_file_packets_preserve_bytes`, `creation_failure_preserves_the_error_and_leaves_no_files` |
