# Unsafe code by crate

`unsafe` blocks, fns and impls in non-test code of each crate under `crates/`, counted by `tools/budget.py` on `c28455f`. ✅ marks a crate whose `lib.rs` has `#![forbid(unsafe_code)]`. The PR tier fails a pull request that raises a crate's count or drops its `forbid`.

| crate | unsafe | forbids |
| --- | ---: | :---: |
| `corebase` | 92 |  |
| `zend` | 76 |  |
| `server` | 52 |  |
| `hooks` | 29 |  |
| `memory` | 15 |  |
| `process` | 15 |  |
| `curl` | 14 |  |
| `phpcoroutines` | 13 |  |
| `phphttp` | 12 |  |
| `phpstdext` | 12 |  |
| `reactor` | 12 |  |
| `phpswoole` | 11 |  |
| `timer` | 11 |  |
| `phpruntime` | 10 |  |
| `oracle` | 8 |  |
| `ports` | 7 |  |
| `bus` | 6 |  |
| `clients` | 6 |  |
| `phpclients` | 6 |  |
| `phpprocess` | 5 |  |
| `tls` | 5 |  |
| `zlib` | 4 |  |
| `brotli` | 3 |  |
| `firebird` | 3 |  |
| `pgsql` | 3 |  |
| `cares-sys` | 2 |  |
| `sqlite` | 2 |  |
| `ssh2` | 2 |  |
| `ssh2-sys` | 2 |  |
| `curl-sys` | 1 |  |
| `firebird-sys` | 1 |  |
| `locale` | 1 |  |
| `oci` | 1 |  |
| `odbc` | 1 |  |
| `utilities` | 1 |  |
| `base64` | 0 | ✅ |
| `crc32` | 0 | ✅ |
| `deflate` | 0 | ✅ |
| `error` | 0 | ✅ |
| `error-ffi` | 0 |  |
| `ext` | 0 |  |
| `ffi` | 0 |  |
| `framing` | 0 | ✅ |
| `http` | 0 | ✅ |
| `http-ffi` | 0 |  |
| `http2` | 0 | ✅ |
| `httpbindings` | 0 | ✅ |
| `inflate` | 0 | ✅ |
| `odbc-sys` | 0 |  |
| `phpcollections` | 0 | ✅ |
| `phpevents` | 0 | ✅ |
| `phphttp2` | 0 | ✅ |
| `phplocks` | 0 | ✅ |
| `phpredis` | 0 | ✅ |
| `phpserver` | 0 | ✅ |
| `phpstubs` | 0 | ✅ |
| `phpthread` | 0 | ✅ |
| `phptimer` | 0 | ✅ |
| `protocols` | 0 | ✅ |
| `websocket` | 0 | ✅ |
| `zend-sys` | 0 |  |

Total 444. Goal: the hardened runtime ([#1893](https://github.com/appwrite-labs/swoole-rs/issues/1893), [#1902](https://github.com/appwrite-labs/swoole-rs/issues/1902)).
