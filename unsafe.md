# Unsafe code by crate

`unsafe` blocks, fns and impls in non-test code of each crate under `crates/`, counted by `tools/budget.py` on `36da0a3`. ✅ marks a crate whose `lib.rs` has `#![forbid(unsafe_code)]`. The PR tier fails a pull request that raises a crate's count or drops its `forbid`.

| crate | unsafe | forbids |
| --- | ---: | :---: |
| `server` | 297 |  |
| `zend` | 264 |  |
| `hooks` | 163 |  |
| `tls` | 131 |  |
| `phpcoroutines` | 102 |  |
| `reactor` | 98 |  |
| `corebase` | 97 |  |
| `phpstdext` | 81 |  |
| `ports` | 80 |  |
| `process` | 78 |  |
| `coroutine` | 75 |  |
| `phpruntime` | 75 |  |
| `phpswoole` | 73 |  |
| `phpclients` | 70 |  |
| `phphttp` | 58 |  |
| `memory` | 48 |  |
| `clients` | 42 |  |
| `curl` | 42 |  |
| `buffer` | 31 |  |
| `phpthread` | 30 |  |
| `odbc` | 26 |  |
| `ssh2` | 26 |  |
| `timer` | 22 |  |
| `phpserver` | 21 |  |
| `bus` | 20 |  |
| `phpprocess` | 20 |  |
| `pgsql` | 16 |  |
| `sqlite` | 15 |  |
| `utilities` | 14 |  |
| `ossupport` | 13 |  |
| `oci` | 12 |  |
| `oracle` | 12 |  |
| `firebird` | 8 |  |
| `zlib` | 8 |  |
| `http-ffi` | 7 |  |
| `brotli` | 4 |  |
| `error-ffi` | 4 |  |
| `locale` | 3 |  |
| `locks` | 3 |  |
| `protocols-ffi` | 3 |  |
| `cares-sys` | 2 |  |
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

Total 2,199. Goal: the hardened runtime ([#1893](https://github.com/appwrite-labs/swoole-rs/issues/1893), [#1902](https://github.com/appwrite-labs/swoole-rs/issues/1902)).
