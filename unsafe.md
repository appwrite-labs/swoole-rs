# Unsafe code by crate

`unsafe` blocks, fns and impls in non-test code of each crate under `crates/`, counted by `tools/budget.py` on `6614c9a`. ✅ marks a crate whose `lib.rs` has `#![forbid(unsafe_code)]`. The PR tier fails a pull request that raises a crate's count or drops its `forbid`.

| crate | unsafe | forbids |
| --- | ---: | :---: |
| `server` | 652 |  |
| `zend` | 491 |  |
| `tls` | 377 |  |
| `hooks` | 282 |  |
| `corebase` | 213 |  |
| `reactor` | 211 |  |
| `phpserver` | 189 |  |
| `phpruntime` | 184 |  |
| `phpcoroutines` | 180 |  |
| `phpclients` | 174 |  |
| `phpstdext` | 170 |  |
| `process` | 160 |  |
| `ports` | 139 |  |
| `phphttp` | 132 |  |
| `clients` | 131 |  |
| `phpswoole` | 120 |  |
| `coroutine` | 104 |  |
| `curl` | 103 |  |
| `buffer` | 83 |  |
| `memory` | 73 |  |
| `phpprocess` | 72 |  |
| `phpthread` | 60 |  |
| `http-ffi` | 58 |  |
| `protocols-ffi` | 58 |  |
| `ossupport` | 53 |  |
| `locks` | 50 |  |
| `pgsql` | 46 |  |
| `odbc` | 41 |  |
| `ssh2` | 41 |  |
| `phphttp2` | 38 |  |
| `timer` | 30 |  |
| `utilities` | 30 |  |
| `bus` | 29 |  |
| `oci` | 29 |  |
| `websocket-ffi` | 26 |  |
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
| `phplocks` | 0 | ✅ |
| `phpredis` | 0 | ✅ |
| `phpstubs` | 0 | ✅ |
| `phptimer` | 0 | ✅ |
| `protocols` | 0 | ✅ |
| `websocket` | 0 | ✅ |
| `zend-sys` | 0 |  |

Total 4,930. Goal: the hardened runtime ([#1893](https://github.com/appwrite-labs/swoole-rs/issues/1893), [#1902](https://github.com/appwrite-labs/swoole-rs/issues/1902)).
