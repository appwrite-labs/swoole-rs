# Reactor gates

`python3 tools/dev.py` registers most of these as phases when the `reactor`
module is selected. Scripts can also be run directly from the repo root.

## Source-body facades

Most gates take the pinned upstream source and a Rust FFI archive:

    python3 tests/modules/reactor/NAME.py --source target/upstream/src \
        --archive PATH/libswoole_rs_ffi.a --sanitize --report OUT.json

`NAME` is one of `forwarding`, `wait`, `callbacks`, `callback_queues`,
`initialize`, `builtins`, `backend_storage`, `flush` and `poll_snapshot`.
Each extracts the original function bodies from the pin and the patched ones
from the archive's hybrid tree (`hybrid_tree`, #796), compiles them against
native facades and the archive, and checks that both variants behave the same.
The 53 ordinary queued-output compositions formerly in `output.cc` now call the
shipped Rust exports in `crates/reactor/src/output.rs`'s
`ordinary_data_positive_matrix` and `ordinary_file_positive_matrix` tests. `reactor-unit` runs them; the redundant
`reactor-output` phase is removed. `forwarding.py` retains the native forwarding
cases and all native/corrected/hybrid Socket syntax and disabled-feature checks.
`clean` is covered by the Rust socketpair test and is not extracted by name.
A PR moving the remaining forwarding decisions ports their cases into Rust and
removes the corresponding facade, as `iov_owned` became `iov_vector_*` tests.

`reactor-native-originals` (`tools/core_tests.py`) runs upstream's own
`network/socket.cpp` and `network/stream.cpp` GoogleTests on both engines.

Compile and declaration gates:

- `compile_gate.py --source S --archive A --report R`: original and hybrid backend translation units compile as C++14.
- `buffer_typed_compile.py --source S --archive A --report R`: the hybrid tree's typed Buffer callers compile.
- `declarations.py --archive A --report R`: every `swr_v1_reactor_*`/`swr_v1_stream_*` declaration in the header is defined in the archive.


## Production consumer

`production_supported.py` loads the real Native and selected `swoole.so` builds,
compiles a small PHP addon (`production_supported.cc`) against each build's
reconstructed headers, and runs the matching `production_*.php` fixture under
both engines:

    python3 tests/modules/reactor/production_supported.py --source target/upstream/src \
        --source-ref HEAD --engine hybrid=SELECTED/swoole.so --engine upstream=NATIVE/swoole.so \
        --build-manifest SELECTED/build-manifest.json --archive SELECTED/libswoole_rs_ffi.a \
        --selected-export SELECTED/export.json --native-export NATIVE/export.json \
        --require-batch --output OUT.json

Profiles: default (readiness over AF_UNIX socketpairs), `--callbacks`,
`--builtins`, `--socket-output`, `--blocking`. `--require-owned-defer` (with `--builtins`)
checks the Rust-owned storage imports and origins against the Native control; `--require-typed-buffer`
refuses a selected build whose source predates the required Rust code.

Known failure: the original broad production profile failed on Native kqueue (a
missing WRITE filter deletion warned, and its assertions assumed poll ordering
and ONCE across separate kqueue records). The supported profile covers
single-filter kqueue records only.

## Unit tests

    cd tests/modules/reactor && python3 -m unittest test_*.py

`test_buffer_typed_identity.py` and `test_sockets_source.py` read historical
commits with `git show` and fail in checkouts that lack them.
