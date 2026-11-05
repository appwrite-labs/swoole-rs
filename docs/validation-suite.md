# Validation

These checks run in addition to the corpus applications and original upstream
PHPTs (see [hybrid build](hybrid-build.md)). `EXT` is a path to a built
`swoole.so`. `tools/dev.py integrate` runs the registered ones (see
[development](development.md)).

| Check | Command |
| --- | --- |
| Seeded buffer queue model | `python3 -m pytest tests/modules/buffer/test_model.py --sanitize --manifest MANIFEST` ([details](../tests/modules/buffer/model.md)) |
| Buffer backpressure/lifecycle | `python3 tests/modules/buffer/lifecycle.py --engine upstream=EXT --engine hybrid=EXT`, or corpus project `11-buffer-lifecycle` |
| Native buffer benchmark | `python3 tests/modules/buffer/benchmark.py --build-manifest target/hybrid/rust/build-manifest.json --iterations 50000 --repeat 5 --output target/reports/buffer-benchmark.json` (`--peek-front` reads each front before pop; `--previous-build-manifest` adds an older build to the same trial series) |
| Original upstream PHPTs | `python3 corpus/upstream/run.py --engine NAME=EXT --require-all` |
| WebSocket length prefixes vs pinned C++ | `python3 -m pytest tests/modules/websocket/test_prefix.py --sanitize --manifest MANIFEST` |
| WebSocket raw transport | `python3 -m pytest tests/modules/websocket/test_close.py --engine upstream=EXT --engine hybrid=EXT` |
| WebSocket invalid-frame policy | `python3 -m pytest tests/modules/websocket/test_invalid.py --engine upstream=EXT --engine hybrid=EXT` |
| Autobahn RFC suite | `python3 tests/modules/websocket/autobahn.py --engine upstream=EXT --engine hybrid=EXT` |
| Timer kernel / adapter / fork / bailout | `python3 -m pytest tests/modules/timer --sanitize --manifest MANIFEST --engine upstream=EXT --engine timer=EXT` |
| Pinned Hyperf skeleton | `python3 corpus/thirdparty/hyperf.py --prepare`, then corpus project `12-hyperf-skeleton` ([scope](../corpus/thirdparty/README.md)) |

Tests run in isolated process groups on ephemeral ports. Fixed-port PHPTs and
the Hyperf app run serially. Application tests do not fetch dependencies;
`--prepare` does that explicitly. C++ ASan/UBSan (`--sanitize`) instruments the
native callers, not Rust or PHP.

## Known failures

These fail on both upstream and hybrid unless noted:

- `tests/swoole_websocket_server/send_close_frame.phpt`: racy on upstream, so
  `run.py` records it unsupported (`RACY`, #111).
- An original close-frame PHPT and the randomized HTTP cookie parser
  (`create_request` fails on 67 of 501 split points).
- `memory/top` PHPT is skipped.
- Buffer lifecycle scenario 4 (client half-close after a complete request): the
  4 MiB response is truncated (about 48 KiB received).
- Timer: on Linux PHP 8.2, an uncleared repeating non-coroutine timer under OOM
  times out.

## Buffer lifecycle scenarios

1. A slow reader drains an ordered 4 MiB response in 16 KiB reads; every byte,
   the HTTP framing and EOF are checked, and queued bytes must be positive.
2. Client reads a 1 KiB prefix of that stream, then disconnects.
3. Client disconnects after a 1 KiB prefix of a sendfile response (offset 13,
   length 4,194,277). `send_queued_bytes` cannot show a sendfile task: pinned
   `Socket::sendfile_async` queues a `TYPE_SENDFILE` chunk with length 0.
4. Client half-closes after sending a complete request; expects the full
   response and EOF.
5. 32 connect/request/disconnect cycles. All 36 tagged connections must see
   exactly one request and one `onClose`. Server FD growth must be at most 4 and
   PHP heap growth at most 4 MiB.
6. Shutdown with an unread queued response: clean exit within 5 s, exactly one
   WorkerStop, at most one close for the session.

HTTP health is checked after the first four scenarios.

## Autobahn

The suite is pinned by digest:
`crossbario/autobahn-testsuite@sha256:519915fb568b04c9383f70a1c405ae3ff44ab9e35835b085239c258b6fac3074`
(suite 25.10.1, Autobahn 0.10.9; Linux amd64). `docker pull` it first.

The default profile selects cases `1.*`–`7.*`, `10.*`, `12.1.1` and `13.1.1`
(249 of 517). `--cases` replaces the selection, `--full` runs all 517, `--host`
overrides `host.docker.internal`. The fixture enables permessage-deflate and
allows 64 MiB messages. A case is a strict pass only if both `behavior` and
`behaviorClose` are `OK`. Missing, unexpected or timed-out cases fail the run.
In the recorded run, upstream and hybrid gave identical category counts
(139/249 strict passes).
The full suite's compression stress cases produce `zlib.error: invalid stored
block lengths` in the legacy client and run for minutes. The manual GitHub
workflow takes `full_suite=true` with a 30-minute bound.
