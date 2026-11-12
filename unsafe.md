# Unsafe code by crate

`unsafe` blocks, fns and impls in non-test code of each crate under `crates/`, counted by `tools/budget.py` on `e931b8a`. ✅ marks a crate whose `lib.rs` has `#![forbid(unsafe_code)]`. The PR tier fails a pull request that raises a crate's count or drops its `forbid`.

| crate | unsafe | forbids |
| --- | ---: | :---: |
| `server` | 596 |  |
| `zend` | 492 |  |
| `tls` | 327 |  |
| `hooks` | 253 |  |
| `reactor` | 195 |  |
| `corebase` | 194 |  |
| `phpcoroutines` | 172 |  |
| `phpstdext` | 157 |  |
| `phpserver` | 144 |  |
| `process` | 136 |  |
| `phpclients` | 127 |  |
| `phpruntime` | 127 |  |
| `ports` | 119 |  |
| `phpswoole` | 115 |  |
| `clients` | 101 |  |
| `phphttp` | 89 |  |
| `coroutine` | 87 |  |
| `curl` | 79 |  |
| `memory` | 73 |  |
| `buffer` | 72 |  |
| `phpprocess` | 59 |  |
| `http-ffi` | 58 |  |
| `ossupport` | 53 |  |
| `protocols-ffi` | 52 |  |
| `locks` | 50 |  |
| `phpthread` | 50 |  |
| `pgsql` | 46 |  |
| `odbc` | 41 |  |
| `ssh2` | 41 |  |
| `timer` | 32 |  |
| `oci` | 29 |  |
| `utilities` | 26 |  |
| `bus` | 25 |  |
| `websocket-ffi` | 24 |  |
| `sqlite` | 21 |  |
| `zlib` | 15 |  |
| `firebird` | 14 |  |
| `oracle` | 14 |  |
| `error-ffi` | 12 |  |
| `brotli` | 9 |  |
| `locale` | 8 |  |
| `curl-sys` | 3 |  |
| `cares-sys` | 2 |  |
| `ssh2-sys` | 2 |  |
| `firebird-sys` | 1 |  |
| `base64` | 0 | ✅ |
| `crc32` | 0 | ✅ |
| `deflate` | 0 | ✅ |
| `error` | 0 | ✅ |
| `ext` | 0 |  |
| `ffi` | 0 |  |
| `framing` | 0 | ✅ |
| `http` | 0 | ✅ |
| `http2` | 0 | ✅ |
| `httpbindings` | 0 | ✅ |
| `inflate` | 0 | ✅ |
| `odbc-sys` | 0 |  |
| `phpcollections` | 0 | ✅ |
| `phpevents` | 0 | ✅ |
| `phphttp2` | 0 | ✅ |
| `phplocks` | 0 | ✅ |
| `phpredis` | 0 | ✅ |
| `phpstubs` | 0 | ✅ |
| `phptimer` | 0 | ✅ |
| `protocols` | 0 | ✅ |
| `websocket` | 0 | ✅ |
| `zend-sys` | 0 |  |

Total 4,342. Goal: the hardened runtime ([#1893](https://github.com/appwrite-labs/swoole-rs/issues/1893), [#1902](https://github.com/appwrite-labs/swoole-rs/issues/1902)).
