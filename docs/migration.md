# Migration approach

Upstream Swoole is replaced one subsystem at a time by Rust code; Cargo builds
`swoole.so` (`crates/ext`). PHP classes and module startup
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
- Build and test only the all-modules Cargo build; the pinned upstream build is the
  oracle. Delete an upstream body once its replacement passes the corpus,
  upstream tests and benchmarks.

Mature third-party libraries (HTTP parsers, TLS, compression, database
clients) are not rewritten unless doing so removes a dependency or fixes a
demonstrated problem.

See [migration modules](migration-modules.md) and [ABI v1](abi-v1.md).

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

Every build links every module's crate and owned C and C++ in `native/`;
no module has a configure switch, a `SWOOLE_RS_<MODULE>` guard or a
cargo feature. A graduated module is listed in `GRADUATED` in
`tools/check_migration.py`, and `dev.py` runs its gates on every run. Reference builds (`tools/build_upstream.py`)
stay pristine: they are the oracle that
differential tests compare against, not a fallback.

Graduated: base64, buffer, corebase, crc32, error, framing, http, protocols, websocket, timer.

## Speed

The merge tier's `callgrind` job is a ratchet, not a target:
[`benchmarks/callgrind_limits.json`](../benchmarks/callgrind_limits.json)
records the Cargo build/upstream instruction ratio per workload, and the
job fails if one rises more than 1% (noise) above its record.

- A PR that makes a workload cheaper lowers its record in the same PR. When
  two consecutive merge runs both beat a record rounded up to two decimals,
  the Badges workflow files a ready ticket to lower it
  (`benchmarks/callgrind.py --ratchet`).
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

### Data layout

Fast code is designed to be fast; it doesn't patch a slow design afterwards.

- **Flat arrays, not hash tables.** Key state by a small dense integer (an fd, a
  slot, a worker id) in `swoole_rs_memory::flat::Table`. A reference to another
  record is an index into its table, not a pointer, and the most used facts about
  it can ride in the index's upper bits so the common check never loads the record.
- **No caches.** A cache hides a slow path; make the path fast instead. The ones
  that stay are upstream behaviour PHP can observe (the DNS cache behind
  `swoole_clear_dns_cache()`, the HTTP `Date` header's per-second value).
- **Structure of arrays** when a loop reads one field of many records: one array
  per field, all indexed by the same key, with hot fields apart from cold ones.
- **Inline storage, not small heap lists.** A list with a known small bound lives
  in a fixed array with a length.
- **Bitsets, not arrays of bools** (`swoole_rs_memory::flat::BitSet`).

`clippy.toml`'s `disallowed-types` rejects std's maps, sets, `LinkedList` and
`VecDeque`. Crates that already use them carry one crate-level `expect` until
their count reaches zero, and `tools/budget.py` (PR tier) counts them, arrays of
bool and cache statics per crate, and fails a PR that raises one above `main`'s.
A use that must stay (output order PHP can see, a cold path) keeps the `expect`.

## Roadmap

`swoole.so` compiles no upstream first-party code (`src/`, `ext-src/`, `include/`):
`crates/ext` builds Rust, `native/` and the pin's bundled `thirdparty/` C, with the
thirdparty files the hybrid changed owned under `native/swoole-src/`.

Phases 1 to 3 and 5 are done: green baseline ([#20](https://github.com/appwrite-labs/swoole-rs/issues/20)),
wider oracle ([#58](https://github.com/appwrite-labs/swoole-rs/issues/58)), graduate and
delete ([#125](https://github.com/appwrite-labs/swoole-rs/issues/125)) and the Zend surface
([#754](https://github.com/appwrite-labs/swoole-rs/issues/754)). Open:

- **Phase 4, deepen thin modules** ([#439](https://github.com/appwrite-labs/swoole-rs/issues/439)).
  Process, hooks, coroutine, reactor and clients move from Rust policy over
  C++ mechanisms to Rust mechanisms.
- **Phase 6, speed** ([#366](https://github.com/appwrite-labs/swoole-rs/issues/366)). The
  Cargo build is within 5% of upstream on every callgrind workload;
  `benchmarks/callgrind_limits.json` records where it stands.

### Upstream files still in the build

None, since #959 (2026-11-08). The last two headers left as owned
copies trimmed to what C++ still uses: `include/swoole_server.h` in #1465
(`native/process/swoole_server.h`, deleted once no C++ included it, #2394) and `include/swoole.h`
in #959 (`native/corebase/swoole.h`).

References: [Rust FFI](https://doc.rust-lang.org/nomicon/ffi.html),
[upstream CMake build](https://github.com/swoole/swoole-src/blob/v6.2.5/CMakeLists.txt),
[upstream config.m4](https://github.com/swoole/swoole-src/blob/v6.2.5/config.m4).
