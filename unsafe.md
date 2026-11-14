# Unsafe code by crate

`unsafe` blocks, fns and impls in non-test code of each crate under `crates/`, counted by `tools/budget.py` on `304fa86`. ✅ marks a crate whose `lib.rs` has `#![forbid(unsafe_code)]`. The PR tier fails a pull request that raises a crate's count or drops its `forbid`.

| crate | unsafe | forbids |
| --- | ---: | :---: |
| `server` | 269 |  |
| `zend` | 264 |  |
| `hooks` | 160 |  |
| `tls` | 111 |  |
| `phpcoroutines` | 102 |  |
| `reactor` | 93 |  |
| `corebase` | 89 |  |
| `ports` | 80 |  |
| `process` | 78 |  |
| `phpruntime` | 75 |  |
| `coroutine` | 74 |  |
| `phpstdext` | 72 |  |
| `phpswoole` | 72 |  |
| `phpclients` | 69 |  |
| `phphttp` | 58 |  |
| `memory` | 48 |  |
| `curl` | 42 |  |
| `clients` | 40 |  |
| `buffer` | 31 |  |
| `phpthread` | 30 |  |
| `odbc` | 26 |  |
| `timer` | 22 |  |
| `bus` | 19 |  |
| `phpprocess` | 19 |  |
| `ssh2` | 18 |  |
| `phpserver` | 15 |  |
| `utilities` | 14 |  |
| `sqlite` | 13 |  |
| `oracle` | 12 |  |
| `pgsql` | 12 |  |
| `oci` | 10 |  |
| `firebird` | 8 |  |
| `http-ffi` | 7 |  |
| `ossupport` | 6 |  |
| `zlib` | 6 |  |
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

Total 2,088. Goal: the hardened runtime ([#1893](https://github.com/appwrite-labs/swoole-rs/issues/1893), [#1902](https://github.com/appwrite-labs/swoole-rs/issues/1902)).
