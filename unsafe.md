# Unsafe code by crate

`unsafe` blocks, fns and impls in non-test code of each crate under `crates/`, counted by `tools/budget.py` on `0f96bd3`. ✅ marks a crate whose `lib.rs` has `#![forbid(unsafe_code)]`. The PR tier fails a pull request that raises a crate's count or drops its `forbid`.

| crate | unsafe | forbids |
| --- | ---: | :---: |
| `server` | 301 |  |
| `zend` | 274 |  |
| `hooks` | 165 |  |
| `tls` | 143 |  |
| `phpcoroutines` | 110 |  |
| `corebase` | 106 |  |
| `reactor` | 105 |  |
| `phpstdext` | 83 |  |
| `ports` | 81 |  |
| `process` | 78 |  |
| `phpclients` | 77 |  |
| `phpruntime` | 76 |  |
| `coroutine` | 75 |  |
| `phpswoole` | 73 |  |
| `phphttp` | 60 |  |
| `memory` | 53 |  |
| `clients` | 52 |  |
| `curl` | 45 |  |
| `phpthread` | 34 |  |
| `buffer` | 31 |  |
| `odbc` | 26 |  |
| `ssh2` | 26 |  |
| `phpprocess` | 23 |  |
| `timer` | 23 |  |
| `phpserver` | 22 |  |
| `bus` | 20 |  |
| `ossupport` | 20 |  |
| `pgsql` | 16 |  |
| `sqlite` | 15 |  |
| `utilities` | 14 |  |
| `oci` | 12 |  |
| `oracle` | 12 |  |
| `http-ffi` | 9 |  |
| `firebird` | 8 |  |
| `zlib` | 8 |  |
| `locale` | 7 |  |
| `brotli` | 4 |  |
| `error-ffi` | 4 |  |
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

Total 2,304. Goal: the hardened runtime ([#1893](https://github.com/appwrite-labs/swoole-rs/issues/1893), [#1902](https://github.com/appwrite-labs/swoole-rs/issues/1902)).
