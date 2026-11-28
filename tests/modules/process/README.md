# Process tests

`process-unit` runs `crates/process`'s tests and `process-signals` runs `signals.py`.
`core-tests/src/os/process_pool.cpp` and `os/pipe.cpp` are Rust cases in
`hooks-system-rust` (tests/modules/hooks/README.md), and the Server worker-retirement
witness is crates/server's `worker_methods::tests::abort_and_reset`.
