# Half-close correctness checks

The half-close correction is an opt-in fix selected by
`tools/build_ext.py --fix-halfclose` (crates/ffi's `halfclose` cargo feature;
default builds keep upstream's behavior). It drains
queued stream/file output before graceful close, protects explicit application
pause/resume, preserves synchronous PROCESS responses across early FIN, retries
incomplete TLS close_notify, keeps sendfile ownership across EINTR, and ignores
cached read events after close is queued. See the
[native contract](../../../native/halfclose/contract.md).

The merge tier's `halfclose` job runs every phase below:
`python3 tools/dev.py integrate --fix-halfclose --php-extension sockets --phase 'halfclose-*' --with-prerequisites`.

These check the repair's behavior, which upstream lacks.
`halfclose-pause-originals` runs `client_close_in_writable_event.phpt` against the
repaired build. The PHPT gate runs it only on the unrepaired Cargo build.

## PHP server gates

Each takes one or more `--engine NAME=/absolute/modules/swoole.so`. On Linux add
`--php-extension sockets` for original selections; Mac has it built in.

```sh
python3 tests/modules/halfclose/gate.py --engine fixed=/absolute/modules/swoole.so
python3 tests/modules/halfclose/prequeue.py --engine fixed=/absolute/modules/swoole.so
python3 tests/modules/halfclose/prequeue.py --engine fixed=/absolute/modules/swoole.so \
  --workers 2 --keep-alive --max-queued-bytes 1
python3 tests/modules/halfclose/pause.py --engine fixed=/absolute/modules/swoole.so
python3 tests/modules/halfclose/pause.py --engine fixed=/absolute/modules/swoole.so --coroutines
python3 tests/modules/halfclose/pause.py --engine fixed=/absolute/modules/swoole.so \
  --tls-cert /absolute/server.crt --tls-key /absolute/server.key --tls-version 1.2
```

- `gate.py`: queued graceful close for stream/file/end responses, FIN and reset,
  BASE and PROCESS. Every payload byte, one close callback, no duplicate request,
  live server.
- `prequeue.py`: early FIN against synchronous PROCESS responses; the second
  profile adds two workers, keep-alive and output pressure.
- `pause.py`: explicit application pause, early resume and resume after queued
  close, normal or coroutine callbacks. With `--tls-cert/--tls-key` every case
  requires the full payload and a strict close_notify; unexpected EOF fails.
  `--immediate-close-only` runs the empty-application-queue boundary.

## Original PHPTs

Run with the official runner:

```sh
python3 corpus/upstream/run.py --manifest tests/modules/halfclose/pause-originals.json \
  --engine fixed=/absolute/modules/swoole.so --require-all
```

## Rust unit checks

- The read dispatcher with a cached EOF after close queued:
  `cargo test -p swoole-rs-server prequeue`.
- TLS shutdown/close: `cargo test -p swoole-rs-tls shutdown` covers it over real
  OpenSSL memory BIOs and nonblocking socketpairs (ordering, one-way, peer-first,
  quiet shutdown, backpressure retry, peer death) plus five injected error results.

## Known failures

These probes exit nonzero on purpose. Do not count them as passing:

- `gate.py --probe-extra-request`: unread requests after `Connection: close`
  still reset or time out.
- `prequeue.py --probe-coroutines` and `--probe-unordered`: suspended coroutine
  responses and non-ordered dispatch are outside the fix.

Suspended HTTP response ownership is blocked on the worker-generation contract in
[http-response-lifetime.md](../../../docs/http-response-lifetime.md).
TLS, thread and custom dispatch are outside the synchronous prequeue fix. Mac's
original `bad_client` warning discrepancy and the expired-client `verify_02`
FAILED/WARNED result are unresolved; the latter is a terminal EPIPE alert write.
See [expired-peer.md](../../../native/halfclose/expired-peer.md).
