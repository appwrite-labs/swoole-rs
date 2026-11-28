# Validation

These checks run in addition to the corpus applications and original upstream
PHPTs (see [corpus](../corpus/README.md)). `EXT` is a path to a built
`swoole.so`. `tools/dev.py integrate` runs the registered ones (see
[development](development.md)).

| Check | Command |
| --- | --- |
| Buffer backpressure/lifecycle | `python3 tests/modules/buffer/lifecycle.py --engine upstream=EXT --engine cargo=EXT`, or corpus project `11-buffer-lifecycle` |
| Native buffer benchmark | `python3 tests/modules/buffer/benchmark.py --archive target/ext/sdk/lib/libswoole_rs_ffi.a --iterations 50000 --repeat 5 --output target/reports/buffer-benchmark.json` (`--peek-front` reads each front before pop; `--previous-archive` adds an older build to the same trial series) |
| Original upstream PHPTs | `python3 corpus/upstream/run.py --engine NAME=EXT --require-all` |
| HTTP/1 framing ambiguities (smuggling corpus) | `python3 -m pytest tests/modules/http/test_smuggling.py --engine upstream=EXT --engine cargo=EXT` |
| PHP-API differential fuzzing | `python3 tests/differential/fuzz.py --engine upstream=EXT --engine cargo=EXT --seed S --programs N --cases tests/differential/cases --keep DIR` |
| WebSocket raw transport | `python3 -m pytest tests/modules/websocket/test_close.py --engine upstream=EXT --engine cargo=EXT` |
| WebSocket invalid-frame policy | `python3 -m pytest tests/modules/websocket/test_invalid.py --engine upstream=EXT --engine cargo=EXT` |
| Autobahn RFC suite | `python3 tests/modules/websocket/autobahn.py --engine upstream=EXT --engine cargo=EXT` |
| Timer fork / bailout | `python3 -m pytest tests/modules/timer --engine upstream=EXT --engine cargo=EXT` |
| Pinned Hyperf skeleton | `python3 corpus/thirdparty/hyperf.py --prepare`, then corpus project `12-hyperf-skeleton` ([scope](../corpus/thirdparty/README.md)) |

Tests run in isolated process groups on ephemeral ports. Fixed-port PHPTs and
the Hyperf app run serially. Application tests do not fetch dependencies;
`--prepare` does that explicitly.

## Autobahn

The suite is pinned by image digest in `autobahn.py`, which pulls it. The
default profile is a bounded subset; `--cases` replaces it, `--full` runs all
517 cases and `--host` overrides `host.docker.internal`. The first engine is the
control: the run fails when a case does worse on another engine in `behavior`
or `behaviorClose` (`OK`, then `NON-STRICT`, then `INFORMATIONAL`, then anything
else), or when results are missing. Strict passes (both `OK`) are counted. The
merge tier's `autobahn` job runs the default profile on upstream and the Cargo
build; `conformance.yml` runs the full suite on them nightly.
