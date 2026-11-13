# Unsafe code by crate

`unsafe` blocks, fns and impls in non-test code of each crate under `crates/`, counted by `tools/budget.py` on `3a58c76`. ✅ marks a crate whose `lib.rs` has `#![forbid(unsafe_code)]`. The PR tier fails a pull request that raises a crate's count or drops its `forbid`.

| crate | unsafe | forbids |
| --- | ---: | :---: |
| `server` | 387 |  |
| `zend` | 314 |  |
| `hooks` | 206 |  |
| `tls` | 200 |  |
| `corebase` | 132 |  |
| `reactor` | 125 |  |
| `phpcoroutines` | 121 |  |
| `process` | 118 |  |
| `phpclients` | 96 |  |
| `phpruntime` | 93 |  |
| `phpstdext` | 92 |  |
| `ports` | 90 |  |
| `coroutine` | 87 |  |
| `phpswoole` | 82 |  |
| `memory` | 69 |  |
| `phphttp` | 69 |  |
| `clients` | 58 |  |
| `curl` | 56 |  |
| `buffer` | 41 |  |
| `phpthread` | 40 |  |
| `phpprocess` | 34 |  |
| `odbc` | 31 |  |
| `ossupport` | 27 |  |
| `bus` | 26 |  |
| `pgsql` | 26 |  |
| `ssh2` | 26 |  |
| `http-ffi` | 25 |  |
| `timer` | 24 |  |
| `utilities` | 24 |  |
| `phpserver` | 23 |  |
| `sqlite` | 15 |  |
| `oci` | 13 |  |
| `oracle` | 12 |  |
| `websocket-ffi` | 12 |  |
| `zlib` | 12 |  |
| `brotli` | 9 |  |
| `firebird` | 8 |  |
| `locale` | 7 |  |
| `error-ffi` | 4 |  |
| `locks` | 3 |  |
| `protocols-ffi` | 3 |  |
| `cares-sys` | 2 |  |
| `ssh2-sys` | 2 |  |
| `curl-sys` | 1 |  |
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

Total 2,846. Goal: the hardened runtime ([#1893](https://github.com/appwrite-labs/swoole-rs/issues/1893), [#1902](https://github.com/appwrite-labs/swoole-rs/issues/1902)).
