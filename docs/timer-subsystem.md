# Timer subsystem (M03)

Contract: [timer-abi-v1.md](timer-abi-v1.md). Upstream targets:
`src/core/timer.cc`, `include/swoole_timer.h`, `ext-src/swoole_timer.cc`,
`src/wrapper/timer.cc`, `src/os/timer.cc`.

## Upstream structure

- `src/core/timer.cc` holds scheduling. `src/os/timer.cc` holds SIGALRM/
  setitimer integration.
- `Timer` owns a min-heap, an ID→`TimerNode*` unordered map, a selection round,
  IDs, a monotonic base and the next wakeup delay. Each `TimerNode` has a
  separately allocated `HeapNode`. Nothing else in the tree uses `Heap`.
- `ext-src/swoole_timer.cc` `clearAll()` and `list()` iterate `Timer::get_map()`
  directly; `info()` reads node fields; `stats()` exposes count and round.
- Coroutine, channel, socket and server code hold native node pointers and
  cancel by pointer. The server uses `delay()` to refresh send timers.
- `reinit()` changes wakeup integration and rearms the next delay, keeping nodes,
  IDs, clock base and rounds.
- `swoole_fork()` (`src/core/base.cc`) frees inherited timers in the child before
  resetting the reactor; `tests/swoole_timer/after_fork.phpt` checks that.

Observable behavior the Rust queue keeps: one captured time per selection;
repeats advance from the old deadline until after that time (missed intervals
fire once); `exec_count` increments after the callback; a head timer from the
current round stops selection; removing the active timer marks it removed but
keeps it visible to `info()` until the callback returns.

## Implementation

The overlay deletes the C++ heap/map/dispatch code; omitting the module keeps
upstream. The adapter removes by ID directly (the `TimerNode` overload only
extracts the ID).

- IDs are generated internally and monotonic, so the map uses a small integer
  hasher instead of SipHash. It is not a general-purpose hash.
- Heap sifting moves a hole and places the candidate once, keeping strict
  comparisons, equal-child choice, swap-last removal and birth-round blocking.
- Update/remove/dispatch use `get_mut` on existing entries, so they never
  allocate (`HashMap::insert` on a full table can grow it).

## Commands

```sh
cargo test --locked -p swoole-rs-timer
cargo clippy --locked -p swoole-rs-timer --all-targets -- -D warnings
python3 tools/build_hybrid.py
python3 -m pytest tests/modules/timer --sanitize \
  --manifest target/hybrid/rust/build-manifest.json \
  --engine upstream=target/upstream/src/modules/swoole.so \
  --engine timer=target/hybrid/rust/modules/swoole.so
```

The gates are pytest files (`pip install pytest`); options and fixtures are in
`tests/conftest.py`, and the first `--engine` is the control. Tests that need an
engine or `--manifest` are skipped without one. The 35 original timer PHPTs run in the `phpt` phase (`corpus/upstream/run.py --module timers`); they are listed in
[UPSTREAM.md](../tests/modules/timer/UPSTREAM.md); four need external DNS/network
and are opt-in. `tests/modules/timer/FORK.md` describes the fork cases.

## Gaps

- Linux PHP-loaded adapter ASan is not set up.
- On macOS, dispatch and repeat throughput were about 1% below upstream and
  cancel about 5% above; Linux is unmeasured.
