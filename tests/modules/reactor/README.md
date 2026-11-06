# Reactor gates

`python3 tools/dev.py` registers most of these as phases when the `reactor`
module is selected. Scripts can also be run directly from the repo root.

## Source-body facades

Most gates take the pinned upstream source and a Rust FFI archive:

    python3 tests/modules/reactor/NAME.py --source target/upstream/src \
        --archive PATH/libswoole_rs_ffi.a --sanitize --report OUT.json

`NAME` is one of `callbacks`, `initialize` and `builtins`.
Each extracts the original function bodies from the pin and the patched ones
from the archive's hybrid tree (`hybrid_tree`, #796), compiles them against
native facades and the archive, and checks that both variants behave the same.
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
