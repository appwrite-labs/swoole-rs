# Unsafe code by crate

`unsafe` blocks, fns and impls in non-test code of each crate under `crates/`, counted by `tools/budget.py` on `da7d23e`. ✅ marks a crate whose `lib.rs` has `#![forbid(unsafe_code)]`. The PR tier fails a pull request that raises a crate's count or drops its `forbid`.

| crate | unsafe | forbids |
| --- | ---: | :---: |
| `server` | 316 |  |
| `zend` | 281 |  |
| `hooks` | 173 |  |
| `tls` | 146 |  |
| `phpcoroutines` | 116 |  |
| `corebase` | 106 |  |
| `reactor` | 106 |  |
| `phpstdext` | 86 |  |
| `phpruntime` | 84 |  |
| `ports` | 82 |  |
| `phpclients` | 81 |  |
| `process` | 80 |  |
| `coroutine` | 78 |  |
| `phpswoole` | 72 |  |
| `phphttp` | 66 |  |
| `clients` | 54 |  |
| `memory` | 54 |  |
| `curl` | 52 |  |
| `phpthread` | 34 |  |
| `buffer` | 31 |  |
| `odbc` | 31 |  |
| `phpprocess` | 29 |  |
| `pgsql` | 26 |  |
| `ssh2` | 26 |  |
| `bus` | 24 |  |
| `timer` | 23 |  |
| `phpserver` | 22 |  |
| `ossupport` | 20 |  |
| `utilities` | 16 |  |
| `sqlite` | 15 |  |
| `oci` | 13 |  |
| `oracle` | 12 |  |
| `zlib` | 12 |  |
| `brotli` | 9 |  |
| `http-ffi` | 9 |  |
| `firebird` | 8 |  |
| `locale` | 7 |  |
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

Total 2,417. Goal: the hardened runtime ([#1893](https://github.com/appwrite-labs/swoole-rs/issues/1893), [#1902](https://github.com/appwrite-labs/swoole-rs/issues/1902)).
