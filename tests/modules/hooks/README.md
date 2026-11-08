# Hooks

Pin `c7e514509451beafd4d70ce5b78943d51e76381c`.

## Native-facade gates

```sh
python3 tests/modules/hooks/packet_gate.py --source PINNED_SOURCE --archive BUILD/rust-prefix/lib/libswoole_rs_ffi.a --sanitize --report OWN/packet.json
python3 tests/modules/hooks/compile_gate.py --source PINNED_SOURCE --archive BUILD/rust-prefix/lib/libswoole_rs_ffi.a --report OWN/compile.json
```

`packet_gate.py` extracts the pinned function bodies and
the patched ones from the hybrid build `--archive` comes from (`hybrid_tree`), compiles both
against a deterministic native facade (`packet_fixture.h`) and runs identical
scenarios through each, comparing effects and order. Syscalls, contexts, TLS
and kernel IO are faked; these are policy/order checks, not runtime proof.
`--sanitize` adds ASan/UBSan to C++ only, not the Rust archive.

- The ordered gate is gone. `wait_for` lost its last callers (flock and `waitpid_safe` are
  Rust) and left the build; its 22 retry and cancel cases are `ported_wait_for_matrix` in
  `crates/hooks/src/flock.rs`, over the same retry loop. The coroutine Socket's transfers and
  `TimerController` moved to Rust with their cases (`ported_*` tests in
  `crates/hooks/src/transfer.rs` and `co_io.rs`).
- The async gate is gone. Its 426 async event/lambda/sleep/DNS cases (dispatch
  rejection, timeout/cancel completion, late canceled completion and errno
  capture) are `ported_async_gate` in `crates/hooks/src/offload/ported.rs`:
  upstream's `async`, `async(fn)`, `System::sleep`,
  `gethostbyname_impl_with_async` and `System::getaddrinfo` run unchanged in
  `swoole-rs-oracle` (`shim/hooks_async.cc`) over the gate's recording facade,
  and the Rust offload, composed as the hybrid's system.cc composes it, runs over
  the same facade in Rust; every scenario's effects must match.
- `packet_gate.py` (240 cases): length/EOF/raw framing paths. Its 384
  vectored all-transfer cases moved to Rust with `readv_all`/`writev_all`
  (`ported_vector_all_matrix` in `crates/hooks/src/vector.rs`).
- The socket policy gate's bodies (the bound-coroutine checks, `set_err`,
  `check_return_value`, `is_available`, `get_event_str` and
  `TimeoutController::has_timedout`) moved to Rust with their cases (`ported_*`
  tests in `crates/hooks/src/co_io.rs`).
- The lifecycle gate's bodies (`shutdown`, `cancel`, `close`,
  `connect(addr)` and `check_liveness`) moved to Rust with their cases
  (`ported_*` tests in `crates/hooks/src/lifecycle.rs`, each comparing Rust
  with upstream's body transcribed over the same fakes).

`compile_gate.py` compiles all eight original and patched translation units as
C++14: the pinned ones, then the pin with every file the hybrid build patched
(`overlay_hybrid_tree`) under that build's `config.h`. io_uring units are
reported UNSUPPORTED when the config does not enable `SW_USE_IOURING`.

## Original tests

`core-tests/src/coroutine/system.cpp` is Rust: `system/` holds its 20 TESTs, one
function each (`coroutine_system.<name>`, upstream's names, inputs and expected
values), linked against the hybrid tree's `libswoole.so`.
`hooks-system-rust` (`system.py`, Linux, integrate lane) builds that library as
`core_tests.py` builds the hybrid engine and runs each case in its own process,
excluding `getaddrinfo` (external DNS and network) and `getaddrinfo_timeout` (a
lookup that misses the resolver cache), as the GoogleTest run did. Where
upstream's `System` signature carries C++ library types the cases call what the
hybrid's callers call instead: `swr_v1_system_read_file`, `_getaddrinfo` and
`_wait_signal`, and `swr_v1_hook_socket_poll` with `System`'s poll operations.

`core-tests/src/coroutine/hook.cpp` is Rust too: `system/src/hook.rs` holds its 26
TESTs, one function each (`coroutine_hook.<name>`, upstream's names, inputs and
expected values), calling the C ABI `swoole_coroutine_*` exports; the same
`hooks-system-rust` phase runs them. Upstream's test helpers are written out over
libc (`File`, `String`, `create_socket_pair`, `file_get_contents`,
`Address::assign`), `System::gethostbyname` is `swr_v1_system_gethostbyname`,
and `exists` reads the socket's `sock_fd` through the hybrid's record layout.
Before each case the binary writes `TEST_TMP_FILE` as core-tests' run did and
removes the `/tmp/123.txt` that `ftruncate` leaves behind with mode 0.
`sendmsg_and_recvmsg` stays excluded (below).

`core-tests/src/coroutine/socket.cpp` is Rust: `system/src/socket.rs` holds its 54
TESTs, one function each (`coroutine_socket.<name>`, upstream's names, inputs, ports
and expected values), run by `hooks-system-rust`. A `Socket` is made and deleted
through `swr_v1_runtime_socket` (its constructors are inline in the hybrid header);
its out-of-line members are called under their upstream symbols, its inline ones
through the `swr_v1_hook_co_socket_*` exports the header calls, and its fields
through the record layout (`swoole_rs_co_socket.h`). `enable_ssl_encrypt` and the
`set_ssl_*` setters write the `SSLContext` owner (`swr_v1_tls_owner_new`,
`_set_path` and its string slots and fields); `TimeoutSetter` and
`TimeoutController` are `swr_v1_hook_timeout_scope` and
`swr_v1_hook_co_socket_timed_out` (the `rust_socket_scopes.selected_timeout_restore_and_copy`
witness, whose `TimeoutSetter` left the hybrid header, is a Rust case there too); `pop_read_buffer`/`pop_write_buffer` are
`swr_v1_hook_co_socket_pop_buffer`; `ssl_handshake` and `ssl_verify` are exported
under their upstream symbols for this. The `Server` fixture `recv_success` and
`recv_fail` fork is a std echo (or close-on-receive) server: those cases assert only
on the client. Excluded as before: `tcp6` (IPv6) and the four proxy cases (a local
HTTP or SOCKS5 proxy).

`core-tests/src/memory/lru_cache.cpp` no longer builds (`include/swoole_lru_cache.h`
is deleted). Its TESTs are Rust tests over the DNS cache's `Lru` in
`crates/hooks/src/dns_cache.rs`, with the same inputs and expected values:
`lru_cache.basic` is `lru_cache_basic`, `lru_cache.memory_free` is
`lru_cache_memory_free` and `lru_cache.lru_kick` is `lru_cache_lru_kick`.

Upstream PHPTs for this module run in the merge tier's PHPT gate (the `phpt-*` jobs), which runs every pinned PHPT on upstream and the all-modules hybrid. To attribute a gate regression to this module, see *Attributing a regression* in [corpus/upstream/README.md](../../../corpus/upstream/README.md).

## Known failures

- PHPTs (229), PASS/FAIL/SKIP: Linux reference 183/14/32 vs Rust 181/16/32;
  macOS reference 165/19/45 vs Rust 161/23/45. Rust has candidate-only
  failures on both platforms; base/async_file failures reproduce in the native
  control on macOS. Redis, external services and PHP deprecations account for
  further failures and skips.
- `hooks-system-rust`: 18 of system.cpp's 20 cases pass; the two `getaddrinfo`
  network cases are excluded. socket.cpp: 49 of 54 pass (Linux aarch64, docker).
  `coroutine_hook.sendmsg_and_recvmsg` is excluded:
  it segfaulted in ~15% of GoogleTest runs on both engines (its child reads the
  finished parent's stack). hook.cpp's other 25 cases pass (Linux aarch64,
  docker); `gethostbyname`, `getaddrinfo` and `socket` need external DNS and
  network.
- macOS (clang): `packet_gate.py` stops at a narrowing error in the selected
  `recv_packet` initializer. It passes on Linux, which is where CI runs it.
- Not run: enabled io_uring runtime, ZTS/thread mode, the PHP originals after
  the latest wait/poll, retry, datagram and curl/MongoDB policy moves, and
  performance.

## Gate audit (#481)

Both phases are kept. They compile the pinned and patched C++ bodies against
fakes and inject what a running server can't reliably produce: EAGAIN, EINTR,
partial IO, timer and dispatch failures, cancel races and destructor reentry, all
under ASan/UBSan. Neither a PHPT nor the Rust unit tests reach that C++ glue.

- `hooks-packet-policy`: the fault/order matrix above.
- `hooks-compile`: the only C++14 build of the eight patched units.

A body these gates slice may move into Rust: the PR that moves it ports the
gate's cases for it into a `crates/hooks` test and drops the body from the gate.
