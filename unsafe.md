# Unsafe code by crate

`unsafe` blocks, fns and impls in non-test code of each crate under `crates/`, counted by `tools/budget.py` on `c3592f0`. ✅ marks a crate whose `lib.rs` has `#![forbid(unsafe_code)]`. The PR tier fails a pull request that raises a crate's count or drops its `forbid`.

| crate | unsafe | forbids |
| --- | ---: | :---: |
| `server` | 246 |  |
| `zend` | 240 |  |
| `hooks` | 159 |  |
| `tls` | 109 |  |
| `phpcoroutines` | 101 |  |
| `reactor` | 92 |  |
| `corebase` | 87 |  |
| `phpruntime` | 75 |  |
| `coroutine` | 74 |  |
| `ports` | 74 |  |
| `process` | 73 |  |
| `phpswoole` | 72 |  |
| `phpstdext` | 63 |  |
| `phpclients` | 61 |  |
| `phphttp` | 58 |  |
| `memory` | 46 |  |
| `clients` | 39 |  |
| `curl` | 37 |  |
| `buffer` | 31 |  |
| `odbc` | 26 |  |
| `phpthread` | 25 |  |
| `timer` | 20 |  |
| `phpprocess` | 19 |  |
| `bus` | 18 |  |
| `ssh2` | 18 |  |
| `phpserver` | 14 |  |
| `utilities` | 14 |  |
| `oracle` | 12 |  |
| `pgsql` | 11 |  |
| `oci` | 10 |  |
| `sqlite` | 9 |  |
| `firebird` | 8 |  |
| `http-ffi` | 7 |  |
| `ossupport` | 6 |  |
| `zlib` | 6 |  |
| `brotli` | 4 |  |
| `error-ffi` | 4 |  |
| `locale` | 3 |  |
| `locks` | 3 |  |
| `cares-sys` | 2 |  |
| `protocols-ffi` | 2 |  |
| `ssh2-sys` | 2 |  |
| `curl-sys` | 1 |  |
| `firebird-sys` | 1 |  |
| `websocket-ffi` | 1 |  |
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

Total 1,983. Goal: the hardened runtime ([#1893](https://github.com/appwrite-labs/swoole-rs/issues/1893), [#1902](https://github.com/appwrite-labs/swoole-rs/issues/1902)).
