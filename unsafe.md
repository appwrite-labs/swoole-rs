# Unsafe code by crate

`unsafe` blocks, fns and impls in non-test code of each crate under `crates/`, counted by `tools/budget.py` on `e183bc6`. ✅ marks a crate whose `lib.rs` has `#![forbid(unsafe_code)]`. The PR tier fails a pull request that raises a crate's count or drops its `forbid`.

| crate | unsafe | forbids |
| --- | ---: | :---: |
| `server` | 122 |  |
| `zend` | 117 |  |
| `hooks` | 86 |  |
| `corebase` | 68 |  |
| `tls` | 62 |  |
| `ports` | 60 |  |
| `reactor` | 59 |  |
| `phpcoroutines` | 54 |  |
| `phpruntime` | 45 |  |
| `coroutine` | 42 |  |
| `phphttp` | 36 |  |
| `process` | 36 |  |
| `phpstdext` | 35 |  |
| `memory` | 30 |  |
| `phpswoole` | 30 |  |
| `clients` | 23 |  |
| `phpclients` | 22 |  |
| `curl` | 19 |  |
| `timer` | 13 |  |
| `buffer` | 12 |  |
| `bus` | 11 |  |
| `phpprocess` | 11 |  |
| `oracle` | 8 |  |
| `http-ffi` | 7 |  |
| `ossupport` | 6 |  |
| `phpserver` | 6 |  |
| `utilities` | 6 |  |
| `phpthread` | 5 |  |
| `ssh2` | 4 |  |
| `zlib` | 4 |  |
| `brotli` | 3 |  |
| `firebird` | 3 |  |
| `pgsql` | 3 |  |
| `cares-sys` | 2 |  |
| `locale` | 2 |  |
| `protocols-ffi` | 2 |  |
| `sqlite` | 2 |  |
| `ssh2-sys` | 2 |  |
| `curl-sys` | 1 |  |
| `firebird-sys` | 1 |  |
| `locks` | 1 |  |
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

Total 1,064. Goal: the hardened runtime ([#1893](https://github.com/appwrite-labs/swoole-rs/issues/1893), [#1902](https://github.com/appwrite-labs/swoole-rs/issues/1902)).
