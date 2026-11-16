# Unsafe code by crate

`unsafe` blocks, fns and impls in non-test code of each crate under `crates/`, counted by `tools/budget.py` on `f37dcf2`. ✅ marks a crate whose `lib.rs` has `#![forbid(unsafe_code)]`. The PR tier fails a pull request that raises a crate's count or drops its `forbid`.

| crate | unsafe | forbids |
| --- | ---: | :---: |
| `zend` | 160 |  |
| `server` | 151 |  |
| `hooks` | 99 |  |
| `reactor` | 74 |  |
| `corebase` | 71 |  |
| `phpcoroutines` | 69 |  |
| `tls` | 68 |  |
| `ports` | 61 |  |
| `coroutine` | 57 |  |
| `phphttp` | 50 |  |
| `phpruntime` | 49 |  |
| `process` | 49 |  |
| `phpstdext` | 48 |  |
| `phpswoole` | 38 |  |
| `phpclients` | 37 |  |
| `curl` | 32 |  |
| `memory` | 32 |  |
| `clients` | 29 |  |
| `timer` | 20 |  |
| `odbc` | 18 |  |
| `ssh2` | 18 |  |
| `bus` | 13 |  |
| `utilities` | 13 |  |
| `buffer` | 12 |  |
| `oracle` | 12 |  |
| `phpprocess` | 12 |  |
| `pgsql` | 11 |  |
| `phpserver` | 9 |  |
| `sqlite` | 9 |  |
| `firebird` | 8 |  |
| `http-ffi` | 7 |  |
| `phpthread` | 7 |  |
| `oci` | 6 |  |
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

Total 1,384. Goal: the hardened runtime ([#1893](https://github.com/appwrite-labs/swoole-rs/issues/1893), [#1902](https://github.com/appwrite-labs/swoole-rs/issues/1902)).
