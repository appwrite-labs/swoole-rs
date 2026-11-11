# Unsafe code by crate

`unsafe` blocks, fns and impls in non-test code of each crate under `crates/`, counted by `tools/budget.py` on `40c49ab`. ✅ marks a crate whose `lib.rs` has `#![forbid(unsafe_code)]`. The PR tier fails a pull request that raises a crate's count or drops its `forbid`.

| crate | unsafe | forbids |
| --- | ---: | :---: |
| `server` | 789 |  |
| `zend` | 498 |  |
| `tls` | 432 |  |
| `hooks` | 354 |  |
| `reactor` | 260 |  |
| `phpserver` | 232 |  |
| `phpcoroutines` | 229 |  |
| `process` | 228 |  |
| `phpclients` | 225 |  |
| `corebase` | 202 |  |
| `phpruntime` | 199 |  |
| `phpstdext` | 186 |  |
| `phphttp` | 177 |  |
| `coroutine` | 162 |  |
| `ports` | 162 |  |
| `clients` | 159 |  |
| `phpswoole` | 157 |  |
| `curl` | 105 |  |
| `memory` | 103 |  |
| `buffer` | 85 |  |
| `phpprocess` | 84 |  |
| `phpthread` | 68 |  |
| `ossupport` | 62 |  |
| `protocols-ffi` | 62 |  |
| `http-ffi` | 58 |  |
| `locks` | 53 |  |
| `phphttp2` | 51 |  |
| `timer` | 50 |  |
| `pgsql` | 47 |  |
| `ssh2` | 47 |  |
| `odbc` | 41 |  |
| `utilities` | 33 |  |
| `bus` | 31 |  |
| `oci` | 31 |  |
| `websocket-ffi` | 28 |  |
| `sqlite` | 22 |  |
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
| `phplocks` | 0 | ✅ |
| `phpredis` | 0 | ✅ |
| `phpstubs` | 0 | ✅ |
| `phptimer` | 0 | ✅ |
| `protocols` | 0 | ✅ |
| `uring-sys` | 0 |  |
| `websocket` | 0 | ✅ |
| `zend-sys` | 0 |  |

Total 5,792. Goal: the hardened runtime ([#1893](https://github.com/appwrite-labs/swoole-rs/issues/1893), [#1902](https://github.com/appwrite-labs/swoole-rs/issues/1902)).
