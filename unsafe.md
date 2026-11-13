# Unsafe code by crate

`unsafe` blocks, fns and impls in non-test code of each crate under `crates/`, counted by `tools/budget.py` on `c0f0972`. ✅ marks a crate whose `lib.rs` has `#![forbid(unsafe_code)]`. The PR tier fails a pull request that raises a crate's count or drops its `forbid`.

| crate | unsafe | forbids |
| --- | ---: | :---: |
| `server` | 487 |  |
| `zend` | 342 |  |
| `tls` | 244 |  |
| `hooks` | 220 |  |
| `corebase` | 157 |  |
| `phpcoroutines` | 146 |  |
| `reactor` | 142 |  |
| `process` | 125 |  |
| `phpstdext` | 113 |  |
| `phpruntime` | 103 |  |
| `ports` | 101 |  |
| `phpclients` | 98 |  |
| `phpswoole` | 95 |  |
| `coroutine` | 87 |  |
| `clients` | 80 |  |
| `buffer` | 71 |  |
| `phphttp` | 71 |  |
| `memory` | 70 |  |
| `curl` | 69 |  |
| `protocols-ffi` | 51 |  |
| `locks` | 50 |  |
| `ossupport` | 44 |  |
| `http-ffi` | 43 |  |
| `phpprocess` | 43 |  |
| `phpthread` | 43 |  |
| `odbc` | 41 |  |
| `ssh2` | 41 |  |
| `pgsql` | 32 |  |
| `oci` | 29 |  |
| `timer` | 28 |  |
| `bus` | 26 |  |
| `utilities` | 26 |  |
| `phpserver` | 24 |  |
| `websocket-ffi` | 23 |  |
| `sqlite` | 21 |  |
| `zlib` | 15 |  |
| `firebird` | 14 |  |
| `oracle` | 14 |  |
| `error-ffi` | 12 |  |
| `brotli` | 9 |  |
| `locale` | 7 |  |
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

Total 3,465. Goal: the hardened runtime ([#1893](https://github.com/appwrite-labs/swoole-rs/issues/1893), [#1902](https://github.com/appwrite-labs/swoole-rs/issues/1902)).
