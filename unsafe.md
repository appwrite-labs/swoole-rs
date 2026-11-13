# Unsafe code by crate

`unsafe` blocks, fns and impls in non-test code of each crate under `crates/`, counted by `tools/budget.py` on `af96a17`. ✅ marks a crate whose `lib.rs` has `#![forbid(unsafe_code)]`. The PR tier fails a pull request that raises a crate's count or drops its `forbid`.

| crate | unsafe | forbids |
| --- | ---: | :---: |
| `server` | 548 |  |
| `zend` | 388 |  |
| `tls` | 252 |  |
| `hooks` | 238 |  |
| `corebase` | 158 |  |
| `reactor` | 149 |  |
| `phpcoroutines` | 146 |  |
| `process` | 132 |  |
| `phpstdext` | 125 |  |
| `phpclients` | 105 |  |
| `phpruntime` | 105 |  |
| `phpswoole` | 105 |  |
| `ports` | 102 |  |
| `clients` | 88 |  |
| `coroutine` | 87 |  |
| `phphttp` | 78 |  |
| `buffer` | 71 |  |
| `curl` | 70 |  |
| `memory` | 70 |  |
| `http-ffi` | 54 |  |
| `protocols-ffi` | 51 |  |
| `locks` | 50 |  |
| `ossupport` | 46 |  |
| `phpprocess` | 46 |  |
| `phpthread` | 43 |  |
| `odbc` | 41 |  |
| `ssh2` | 41 |  |
| `phpserver` | 35 |  |
| `pgsql` | 32 |  |
| `oci` | 29 |  |
| `timer` | 28 |  |
| `bus` | 26 |  |
| `utilities` | 26 |  |
| `websocket-ffi` | 23 |  |
| `sqlite` | 21 |  |
| `zlib` | 15 |  |
| `firebird` | 14 |  |
| `oracle` | 14 |  |
| `error-ffi` | 12 |  |
| `brotli` | 9 |  |
| `locale` | 7 |  |
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

Total 3,688. Goal: the hardened runtime ([#1893](https://github.com/appwrite-labs/swoole-rs/issues/1893), [#1902](https://github.com/appwrite-labs/swoole-rs/issues/1902)).
