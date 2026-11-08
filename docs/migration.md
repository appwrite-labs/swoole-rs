# Migration approach

Upstream Swoole is replaced one subsystem at a time by Rust code linked as a
static library into the pinned extension build. PHP classes and module startup
are Rust over `crates/zend`; what is still compiled from upstream is listed
under [Upstream files still in the build](#upstream-files-still-in-the-build).

Where Rust meets the remaining C++, the seam is hand-written `extern "C"`
wrappers rather than bindgen over C++ headers. At every boundary:

- Pass opaque handles, byte pointers with lengths and integer error codes.
- Allocate and release on the same side. Say who owns each buffer and whether a
  callee may retain it.
- No Rust panic or C++ exception crosses the ABI.
- PHP values stay on their owning thread. No async runtime drives Swoole
  descriptors (`deny.toml` bans them).
- Build and test only the all-modules hybrid; the pinned upstream build is the
  oracle. Delete an upstream body once its replacement passes the corpus,
  upstream tests and benchmarks.

Mature third-party libraries (HTTP parsers, TLS, compression, database
clients) are not rewritten unless doing so removes a dependency or fixes a
demonstrated problem.

See [migration modules](migration-modules.md), [ABI v1](abi-v1.md) and
[hybrid build](hybrid-build.md).

## Graduation

A module graduates when:

1. Every merge-tier phase tagged with it passes on two consecutive completed
   merge-tier runs.
2. Its original PHPTs pass wherever they pass on upstream, as far as the PHPT
   manifest covers it.
3. Its callgrind cost on the corpus workloads that exercise it is measured and
   posted, and graduating it keeps the merge tier's ratchet (see Speed). Module
   micro-benchmarks and timings are reported but don't gate.
4. It has no open `p0`/`p1` `kind:bug` issues.

Every Rust build applies every module's patches, sets its feature bit and links
its crate; no module has a configure switch, a `SWOOLE_RS_<MODULE>` guard or a
cargo feature. A graduated module is listed in `GRADUATED` in
`tools/build_hybrid.py`, and `dev.py` runs its gates on every run. Reference builds (`tools/build_upstream.py`,
`build_hybrid.py --backend upstream`) stay pristine: they are the oracle that
differential tests compare against, not a fallback.

Graduated: base64, buffer, corebase, crc32, error, framing, http, protocols, websocket, timer.

## Speed

The merge tier's `callgrind` job is a ratchet, not a target:
[`benchmarks/callgrind_limits.json`](../benchmarks/callgrind_limits.json)
records the all-modules hybrid/upstream instruction ratio per workload, and the
job fails if one rises more than 1% (noise) above its record.

- A PR that makes a workload cheaper lowers its record in the same PR.
- A PR that moves work into Rust may raise a record by up to 3% of its value,
  stating the before/after numbers in the PR. Migration may be slow before it
  is fast; the ratchet keeps that visible and bounded.
- A record may also rise when an A/B of the same base with and without the
  change shows the extra instructions outside the code it changed (code layout,
  allocator): the PR states the A/B (#686).
- Speed work keeps or grows Rust's share. The default fix moves the whole
  operation, decision and effect, into Rust (#366). Handing code back to
  upstream is only for glue in which Rust decided nothing, and the PR says so.
- Within 5% of upstream is the target for Phase 6, when Rust owns the event
  loop and the HTTP path end to end.

## Roadmap

Progress is measured by the upstream first-party lines (`src/`, `ext-src/`,
`include/`, excluding `thirdparty/` and the generated embedded library) that the
all-modules patch set deletes (`tools/badges.py`). A pinned line a patch moves to
another file is still compiled, so it counts as kept where it now lives. The
migration is done when nothing remains apart from declared third-party code.

Phases 1 to 3 and 5 are done: green baseline ([#20](https://github.com/loks0n/swoole-rs/issues/20)),
wider oracle ([#58](https://github.com/loks0n/swoole-rs/issues/58)), graduate and
delete ([#125](https://github.com/loks0n/swoole-rs/issues/125)) and the Zend surface
([#754](https://github.com/loks0n/swoole-rs/issues/754)). Open:

- **Phase 4, deepen thin modules** ([#439](https://github.com/loks0n/swoole-rs/issues/439)).
  Process, hooks, coroutine, reactor and clients move from Rust policy over
  C++ mechanisms to Rust mechanisms.
- **Phase 6, speed** ([#366](https://github.com/loks0n/swoole-rs/issues/366)). The
  all-modules hybrid is within 5% of upstream on every callgrind workload;
  `benchmarks/callgrind_limits.json` records where it stands.

### Upstream files still in the build

None, since #959 (2026-11-08): `tools/badges.py --source target/upstream/src --output
target/badges --kept` lists no upstream first-party file. The last two headers left as owned
copies trimmed to what C++ still uses: `include/swoole_server.h` in #1465
(`native/process/swoole_server.h`) and `include/swoole.h` in #959 (`native/corebase/swoole.h`).

References: [Rust FFI](https://doc.rust-lang.org/nomicon/ffi.html),
[upstream CMake build](https://github.com/swoole/swoole-src/blob/v6.2.5/CMakeLists.txt),
[upstream config.m4](https://github.com/swoole/swoole-src/blob/v6.2.5/config.m4).
