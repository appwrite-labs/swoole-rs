# Unsafe code by crate

`unsafe` blocks, fns and impls in non-test code of each crate under `crates/`, counted by `tools/budget.py` on `0c0e15d`. ✅ marks a crate whose `lib.rs` has `#![forbid(unsafe_code)]`. The PR tier fails a pull request that raises a crate's count or drops its `forbid`.

| crate | unsafe | forbids |
| --- | ---: | :---: |
| `server` | 431 |  |
| `zend` | 347 |  |
| `tls` | 211 |  |
| `hooks` | 208 |  |
| `corebase` | 144 |  |
| `reactor` | 139 |  |
| `phpcoroutines` | 134 |  |
| `process` | 120 |  |
| `phpruntime` | 101 |  |
| `phpclients` | 97 |  |
| `phpstdext` | 97 |  |
| `phpswoole` | 92 |  |
| `ports` | 91 |  |
| `coroutine` | 87 |  |
| `clients` | 73 |  |
| `phphttp` | 71 |  |
| `memory` | 69 |  |
| `curl` | 56 |  |
| `locks` | 50 |  |
| `phpprocess` | 45 |  |
| `http-ffi` | 43 |  |
| `buffer` | 41 |  |
| `phpthread` | 41 |  |
| `ssh2` | 39 |  |
| `ossupport` | 34 |  |
| `protocols-ffi` | 33 |  |
| `odbc` | 31 |  |
| `timer` | 27 |  |
| `bus` | 26 |  |
| `pgsql` | 26 |  |
| `utilities` | 26 |  |
| `phpserver` | 23 |  |
| `sqlite` | 15 |  |
| `zlib` | 15 |  |
| `oracle` | 14 |  |
| `websocket-ffi` | 14 |  |
| `oci` | 13 |  |
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

Total 3,158. Goal: the hardened runtime ([#1893](https://github.com/appwrite-labs/swoole-rs/issues/1893), [#1902](https://github.com/appwrite-labs/swoole-rs/issues/1902)).
