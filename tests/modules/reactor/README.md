# Reactor gates

`python3 tools/dev.py` registers most of these as phases when the `reactor`
module is selected. Scripts can also be run directly from the repo root.

## Former facades

The `callbacks`, `initialize` and `builtins` gates sliced function bodies out of
the former hybrid `src/wrapper/event.cc`. Their cases are `crates/reactor` tests named
`former_callbacks_gate_*`, `former_initialize_gate_*` and
`former_builtins_gate_*` (`callbacks.rs`, `initialize.rs`, `builtins.rs`).
The 53 ordinary queued-output compositions formerly in `output.cc` now call the
shipped Rust exports in `crates/reactor/src/output.rs`'s
`ordinary_data_positive_matrix` and `ordinary_file_positive_matrix` tests. `reactor-unit` runs them; the redundant
`reactor-output` phase is removed. The wait frame formerly in `wait.cc` is
`wait_loop.rs`'s `whole_wait_runs_upstreams_frame_on_each_backend`. The backends' add, set,
del and constructors are `swr_v1_reactor_change` and `swr_v1_reactor_state_open`; `change.rs` and
`reactor_state.rs` test them over real pipes, so the `backend_storage` and `poll_snapshot` facades are gone. `Socket::sendfile` and `send_async` are Rust (`crates/tls/src/upstream.rs`), whose tests
cover them, so the `forwarding.py` facade is gone.

`reactor-native-originals` (`tools/core_tests.py`) runs upstream's own
`network/socket.cpp` and `network/stream.cpp` GoogleTests on both engines.

Compile and declaration gates:

- `compile_gate.py --source S --archive A --report R`: original and hybrid backend translation units compile as C++14.
- `declarations.py --archive A --report R`: every `swr_v1_reactor_*`/`swr_v1_stream_*` declaration in the header is defined in the archive.


## Unit tests

    cd tests/modules/reactor && python3 -m unittest test_*.py

`test_sockets_source.py` reads historical
commits with `git show` and fails in checkouts that lack them.
