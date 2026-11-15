# Unsafe code by crate

`unsafe` blocks, fns and impls in non-test code of each crate under `crates/`, counted by `tools/budget.py` on `dde469d`. ✅ marks a crate whose `lib.rs` has `#![forbid(unsafe_code)]`. The PR tier fails a pull request that raises a crate's count or drops its `forbid`.

| crate | unsafe | forbids |
| --- | ---: | :---: |
| `zend` | 196 |  |
| `server` | 162 |  |
| `hooks` | 128 |  |
| `corebase` | 78 |  |
| `tls` | 77 |  |
| `phpcoroutines` | 75 |  |
| `reactor` | 75 |  |
| `process` | 70 |  |
| `phpswoole` | 63 |  |
| `ports` | 61 |  |
| `phpstdext` | 58 |  |
| `coroutine` | 57 |  |
| `phpruntime` | 56 |  |
| `phphttp` | 54 |  |
| `phpclients` | 46 |  |
| `clients` | 36 |  |
| `curl` | 35 |  |
| `memory` | 32 |  |
| `phpthread` | 21 |  |
| `odbc` | 20 |  |
| `timer` | 20 |  |
| `phpprocess` | 19 |  |
| `ssh2` | 18 |  |
| `utilities` | 14 |  |
| `bus` | 13 |  |
| `buffer` | 12 |  |
| `oracle` | 12 |  |
| `pgsql` | 11 |  |
| `oci` | 10 |  |
| `phpserver` | 10 |  |
| `sqlite` | 9 |  |
| `firebird` | 8 |  |
| `http-ffi` | 7 |  |
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

Total 1,598. Goal: the hardened runtime ([#1893](https://github.com/appwrite-labs/swoole-rs/issues/1893), [#1902](https://github.com/appwrite-labs/swoole-rs/issues/1902)).
