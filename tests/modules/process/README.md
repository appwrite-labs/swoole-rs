# Process tests

`process-unit` runs `crates/process`'s tests and `process-signals` runs `signals.py`.
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
