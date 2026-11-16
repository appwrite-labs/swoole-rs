# Unsafe code by crate

`unsafe` blocks, fns and impls in non-test code of each crate under `crates/`, counted by `tools/budget.py` on `0c9c8f5`. ✅ marks a crate whose `lib.rs` has `#![forbid(unsafe_code)]`. The PR tier fails a pull request that raises a crate's count or drops its `forbid`.

| crate | unsafe | forbids |
| --- | ---: | :---: |
| `zend` | 132 |  |
| `server` | 124 |  |
| `hooks` | 91 |  |
| `corebase` | 68 |  |
| `tls` | 67 |  |
| `ports` | 60 |  |
| `reactor` | 59 |  |
| `phpcoroutines` | 57 |  |
| `coroutine` | 49 |  |
| `phpruntime` | 45 |  |
| `phpstdext` | 42 |  |
| `phphttp` | 39 |  |
| `process` | 38 |  |
| `phpswoole` | 34 |  |
| `memory` | 31 |  |
| `clients` | 28 |  |
| `curl` | 25 |  |
| `phpclients` | 23 |  |
| `ssh2` | 18 |  |
| `timer` | 17 |  |
| `buffer` | 12 |  |
| `bus` | 12 |  |
| `oracle` | 12 |  |
| `phpprocess` | 11 |  |
| `utilities` | 9 |  |
| `phpserver` | 8 |  |
| `firebird` | 7 |  |
| `http-ffi` | 7 |  |
| `pgsql` | 7 |  |
| `ossupport` | 6 |  |
| `phpthread` | 5 |  |
| `zlib` | 5 |  |
| `brotli` | 4 |  |
| `locks` | 3 |  |
| `cares-sys` | 2 |  |
| `locale` | 2 |  |
| `protocols-ffi` | 2 |  |
| `sqlite` | 2 |  |
| `ssh2-sys` | 2 |  |
| `curl-sys` | 1 |  |
| `firebird-sys` | 1 |  |
| `oci` | 1 |  |
| `odbc` | 1 |  |
| `websocket-ffi` | 1 |  |
| `base64` | 0 | ✅ |
| `crc32` | 0 | ✅ |
| `deflate` | 0 | ✅ |
| `error` | 0 | ✅ |
| `error-ffi` | 0 |  |
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

Total 1,170. Goal: the hardened runtime ([#1893](https://github.com/appwrite-labs/swoole-rs/issues/1893), [#1902](https://github.com/appwrite-labs/swoole-rs/issues/1902)).
