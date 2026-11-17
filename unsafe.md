# Unsafe code by crate

`unsafe` blocks, fns and impls in non-test code of each crate under `crates/`, counted by `tools/budget.py` on `13f8476`. ✅ marks a crate whose `lib.rs` has `#![forbid(unsafe_code)]`. The PR tier fails a pull request that raises a crate's count or drops its `forbid`.

| crate | unsafe | forbids |
| --- | ---: | :---: |
| `corebase` | 132 |  |
| `zend` | 65 |  |
| `server` | 34 |  |
| `hooks` | 25 |  |
| `curl` | 12 |  |
| `phpcoroutines` | 12 |  |
| `phpstdext` | 12 |  |
| `phpswoole` | 10 |  |
| `phpruntime` | 9 |  |
| `oracle` | 8 |  |
| `phphttp` | 7 |  |
| `ports` | 5 |  |
| `clients` | 4 |  |
| `phpclients` | 4 |  |
| `phpprocess` | 4 |  |
| `zlib` | 3 |  |
| `cares-sys` | 2 |  |
| `ssh2` | 2 |  |
| `ssh2-sys` | 2 |  |
| `bus` | 1 |  |
| `curl-sys` | 1 |  |
| `firebird` | 1 |  |
| `oci` | 1 |  |
| `odbc` | 1 |  |
| `pgsql` | 1 |  |
| `sqlite` | 1 |  |
| `base64` | 0 | ✅ |
| `brotli` | 0 |  |
| `crc32` | 0 | ✅ |
| `deflate` | 0 | ✅ |
| `error` | 0 | ✅ |
| `error-ffi` | 0 |  |
| `ext` | 0 |  |
| `ffi` | 0 |  |
| `firebird-sys` | 0 |  |
| `framing` | 0 | ✅ |
| `http` | 0 | ✅ |
| `http2` | 0 | ✅ |
| `httpbindings` | 0 | ✅ |
| `inflate` | 0 | ✅ |
| `locale` | 0 |  |
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

Total 359. Goal: the hardened runtime ([#1893](https://github.com/appwrite-labs/swoole-rs/issues/1893), [#1902](https://github.com/appwrite-labs/swoole-rs/issues/1902)).
