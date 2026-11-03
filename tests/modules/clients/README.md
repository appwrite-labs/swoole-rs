# Clients and resolver migration

Partial, opt-in conversion of client and resolver policy into Rust
(`crates/clients`, overlay `migration/patches/clients/`). Native code keeps
ownership of sockets, Zend values, callbacks, allocations and IO; Rust makes
copied-fact decisions and native effects run after Rust returns. No default
activation.

## Actual-body differentials

Every `*_gate.py` and `gate.py` takes the same arguments:

```sh
python3 tests/modules/clients/gate.py --source /path/to/pinned-git \
  --archive /path/to/libswoole_rs_ffi.a --output target/clients.json [--sanitize] [--rust-sanitized]
```

Each gate extracts the original and patched function bodies from the pinned
source and the clients overlay, compiles both into one program with native
effect-recording doubles, and compares return values, state, errors and effect
order. It fails if a selected body is unchanged or does not call Rust. Without
`--archive`, `gate.py` builds `crates/clients/src/lib.rs` with rustc into a
private archive. `--sanitize` adds native ASan/UBSan; `--rust-sanitized` records
that the archive was ASan-built (requires `--archive` and `--sanitize`). None of
these are whole-TU, live-socket or PHP runtime tests.

| Gate | Bodies covered |
|---|---|
| `gate.py` | Address URL/port policy, hosts-file lookup (real file IO) |
| `http_gate.py` | HTTP Set-Cookie parsing |
| `codec_gate.py` | DNS domain encode/decode and whole query/response callers |
| `pool_gate.py` | Rust persistent pool vs native reference, fork isolation; `--faults` adds allocation-failure cases from a cfg(test) archive |
| `resolver_gate.py` | PHP resolver cluster lookup, retry, host/port selection |
| `http_format_gate.py` | Request formatting against the pinned `sw_snprintf` |
| `ip_gate.py` | Numeric Address validation/assignment and loopback against libc |
| `ip_text_gate.py` | DNS numeric IP formatting |

`python3 -m unittest discover -s tests/modules/clients -p 'test_*.py'` checks
that the shared command recorder in `gate.py` keeps non-UTF-8 diagnostics.

## PHP runtime witnesses

`php_<family>.py` runs `php_<family>.php` against native and selected engines
using private peers on ephemeral ports. Families: `lifecycle`, `http`, `dns`,
`pool`, `pool_fork`, `resolver`, `http_connection`, `http_format`, `coro`,
`async`, `coro_io`.

```sh
python3 tests/modules/clients/php_http.py --build-manifest BUILD_MANIFEST \
  --source-ref HEAD --engine native=/path/native.so --engine hybrid=/path/selected.so \
  --php-extension curl --php-extension sockets --output target/clients-php-http.json
```

`php_attestation.py` refuses to run unless the selected extension matches the
build manifest and the frozen patch section for that family's destination matches
the current overlay; the extension hash must also stay unchanged across the run.
`php_lifecycle.py` takes no `--source-ref`. `--native-only` (all except `http`
and `dns`) runs native controls and reports `passed: false`. `php_dns.py`
rejects c-ares builds, which bypass the converted codec. `php_async.py` bailout
mode expects exit 43 on PHP 8.2 and 0 on 8.5; other versions are unsupported.
`php_pool_fork.py` needs pcntl.

## Original PHPTs

Upstream PHPTs for this module run in the merge tier's PHPT gate (the `phpt-*` jobs), which runs every pinned PHPT on upstream and the all-modules hybrid. To attribute a gate regression to this module, see *Attributing a regression* in [corpus/upstream/README.md](../../../corpus/upstream/README.md).

## Known gaps and differences

- macOS Rust ASan archives fail to link (`___asan_version_mismatch_check_v8`);
  Rust ASan runs on Linux only.
- `codec_gate.py` case counts differ by platform because plain `char` is signed
  on macOS and unsigned on Linux.
- Some inputs are outside the compared domain and are not compared; Rust stops at
  bounds on them.
- The Rust pool frees its container buffers at teardown; the original leaks
  residual queues. Neither closes the opaque Client tokens.
- Removing temporary hosts-parser containers and IP-text string appends changes
  allocation-failure behaviour.
- `ip_gate.py`: macOS keeps its decimal leading-zero grammar and native
  `inet_pton` for scoped IPv6; Linux is strict.
- Async gate locale coverage is the C locale only.
- The pinned `SHUT_RDWR` branch sets only `shutdown_read`; this is preserved.
- `network::Client` itself (crates/clients/src/client.rs) is covered by its Rust
  unit tests against a recording double of the C++ half, and by the PHPTs.
- No benchmark has been run.
