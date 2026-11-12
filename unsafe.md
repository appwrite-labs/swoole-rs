# Unsafe code by crate

`unsafe` blocks, fns and impls in non-test code of each crate under `crates/`, counted by `tools/budget.py` on `2e5bba2`. ✅ marks a crate whose `lib.rs` has `#![forbid(unsafe_code)]`. The PR tier fails a pull request that raises a crate's count or drops its `forbid`.

| crate | unsafe | forbids |
| --- | ---: | :---: |
| `server` | 584 |  |
| `zend` | 486 |  |
| `tls` | 328 |  |
| `hooks` | 252 |  |
| `reactor` | 195 |  |
| `corebase` | 194 |  |
| `phpcoroutines` | 169 |  |
| `phpstdext` | 157 |  |
| `process` | 137 |  |
| `phpserver` | 123 |  |
| `phpruntime` | 120 |  |
| `phpclients` | 119 |  |
| `ports` | 119 |  |
| `phpswoole` | 115 |  |
| `clients` | 101 |  |
| `coroutine` | 87 |  |
| `phphttp` | 84 |  |
| `curl` | 79 |  |
| `memory` | 73 |  |
| `buffer` | 72 |  |
| `http-ffi` | 58 |  |
| `phpprocess` | 58 |  |
| `ossupport` | 53 |  |
| `protocols-ffi` | 52 |  |
| `locks` | 50 |  |
| `phpthread` | 50 |  |
| `pgsql` | 46 |  |
| `odbc` | 41 |  |
| `ssh2` | 41 |  |
| `timer` | 32 |  |
| `oci` | 29 |  |
| `bus` | 26 |  |
| `utilities` | 26 |  |
| `websocket-ffi` | 23 |  |
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
| `phphttp2` | 0 | ✅ |
| `phplocks` | 0 | ✅ |
| `phpredis` | 0 | ✅ |
| `phpstubs` | 0 | ✅ |
| `phptimer` | 0 | ✅ |
| `protocols` | 0 | ✅ |
| `websocket` | 0 | ✅ |
| `zend-sys` | 0 |  |

Total 4,280. Goal: the hardened runtime ([#1893](https://github.com/appwrite-labs/swoole-rs/issues/1893), [#1902](https://github.com/appwrite-labs/swoole-rs/issues/1902)).
