# Unsafe code by crate

`unsafe` blocks, fns and impls in non-test code of each crate under `crates/`, counted by `tools/budget.py` on `3b863d2`. ✅ marks a crate whose `lib.rs` has `#![forbid(unsafe_code)]`. The PR tier fails a pull request that raises a crate's count or drops its `forbid`.

| crate | unsafe | forbids |
| --- | ---: | :---: |
| `server` | 409 |  |
| `zend` | 324 |  |
| `hooks` | 206 |  |
| `tls` | 202 |  |
| `corebase` | 134 |  |
| `reactor` | 130 |  |
| `phpcoroutines` | 121 |  |
| `process` | 119 |  |
| `phpclients` | 97 |  |
| `phpruntime` | 93 |  |
| `phpstdext` | 93 |  |
| `ports` | 90 |  |
| `coroutine` | 87 |  |
| `phpswoole` | 84 |  |
| `phphttp` | 70 |  |
| `memory` | 69 |  |
| `clients` | 61 |  |
| `curl` | 56 |  |
| `buffer` | 41 |  |
| `phpthread` | 41 |  |
| `ssh2` | 39 |  |
| `phpprocess` | 37 |  |
| `ossupport` | 34 |  |
| `protocols-ffi` | 33 |  |
| `odbc` | 31 |  |
| `bus` | 26 |  |
| `pgsql` | 26 |  |
| `utilities` | 26 |  |
| `http-ffi` | 25 |  |
| `timer` | 24 |  |
| `phpserver` | 23 |  |
| `locks` | 20 |  |
| `sqlite` | 15 |  |
| `oracle` | 14 |  |
| `websocket-ffi` | 14 |  |
| `oci` | 13 |  |
| `zlib` | 12 |  |
| `brotli` | 9 |  |
| `firebird` | 8 |  |
| `locale` | 7 |  |
| `error-ffi` | 4 |  |
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

Total 2,973. Goal: the hardened runtime ([#1893](https://github.com/appwrite-labs/swoole-rs/issues/1893), [#1902](https://github.com/appwrite-labs/swoole-rs/issues/1902)).
