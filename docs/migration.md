# Migration approach

The upstream Swoole extension keeps its entry point, PHP object model, worker
lifecycle, scheduler and hooks. Bounded subsystems are replaced one at a time
by Rust code linked as a static library and called through a small C ABI.

Swoole's internals are C++ classes, not a C API, so the seam uses hand-written
`extern "C"` wrappers rather than bindgen over C++ headers. At every boundary:

- Pass opaque handles, byte pointers with lengths and integer error codes.
- Allocate and release on the same side. Say who owns each buffer and whether a
  callee may retain it.
- No Rust panic or C++ exception crosses the ABI. Zend calls and PHP
  exception/bailout handling stay in the PHP-facing C++ layer.
- PHP values stay on their owning thread. Upstream owns the reactor and
  scheduling; no Tokio runtime drives Swoole descriptors.
- Keep the upstream implementation selectable at build time until the
  replacement passes the corpus, upstream tests and benchmarks, then delete it.

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

A graduated module is listed in `GRADUATED` in `tools/build_hybrid.py`. Every
Rust build applies its patches, sets its feature bit and links its crate; there
is no `--module` or `--enable-swoole-rs-` switch, no `SWOOLE_RS_` guard and no
cargo feature. Reference builds (`tools/build_upstream.py`,
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
`include/`, excluding `thirdparty/`) still compiled into the default build. At
the start of the roadmap (2026-10-25) that was about 77k of 89k. The migration is
done when nothing remains apart from declared third-party code.

1. **Green baseline.** A build with every module passes the merge tier on
   `main`: corpus, selected PHPTs and benchmarks against upstream.
2. **Wider oracle.** Grow the PHPT selection from 69 of 1,719 toward the full
   upstream suite, grouped by module.
3. **Graduate and delete.** Make Rust the only path for leaf modules (base64,
   crc32, error, framing, websocket, timer, buffer, http, protocols). Delete
   their upstream fallbacks and fold or remove the optional ownership switches.
4. **Deepen thin modules.** Process, hooks, coroutine, reactor and clients move
   from Rust policy over C++ mechanisms to Rust mechanisms.
5. **Claim unowned code.** Add modules for the server, coroutine socket,
   process and pool frontends, threads, curl and database drivers, or record
   why each stays native.
6. **Invert the host.** Rust owns module startup, class registration and the
   [embedded PHP library](../migration/embedded-runtime-scope.json), with
   minimal Zend shims. The pinned upstream tree stops being a build input.
   The all-modules hybrid is within 5% of upstream on every callgrind workload.

References: [Rust FFI](https://doc.rust-lang.org/nomicon/ffi.html),
[upstream CMake build](https://github.com/swoole/swoole-src/blob/v6.2.4/CMakeLists.txt),
[upstream config.m4](https://github.com/swoole/swoole-src/blob/v6.2.4/config.m4).
