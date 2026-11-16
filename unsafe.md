# Unsafe code by crate

`unsafe` blocks, fns and impls in non-test code of each crate under `crates/`, counted by `tools/budget.py` on `11244c5`. ✅ marks a crate whose `lib.rs` has `#![forbid(unsafe_code)]`. The PR tier fails a pull request that raises a crate's count or drops its `forbid`.

| crate | unsafe | forbids |
| --- | ---: | :---: |
| `zend` | 103 |  |
| `server` | 84 |  |
| `hooks` | 73 |  |
| `corebase` | 52 |  |
| `reactor` | 46 |  |
| `coroutine` | 45 |  |
| `ports` | 42 |  |
| `phpcoroutines` | 39 |  |
| `tls` | 39 |  |
| `process` | 36 |  |
| `phphttp` | 34 |  |
| `memory` | 30 |  |
| `phpruntime` | 22 |  |
| `phpswoole` | 22 |  |
| `phpclients` | 21 |  |
| `clients` | 20 |  |
| `phpstdext` | 19 |  |
| `curl` | 18 |  |
| `timer` | 13 |  |
| `buffer` | 12 |  |
| `bus` | 10 |  |
| `phpprocess` | 10 |  |
| `oracle` | 8 |  |
| `http-ffi` | 7 |  |
| `phpserver` | 6 |  |
| `utilities` | 6 |  |
| `ossupport` | 4 |  |
| `phpthread` | 4 |  |
| `zlib` | 4 |  |
| `brotli` | 3 |  |
| `firebird` | 3 |  |
| `pgsql` | 3 |  |
| `cares-sys` | 2 |  |
| `locale` | 2 |  |
| `protocols-ffi` | 2 |  |
| `sqlite` | 2 |  |
| `ssh2` | 2 |  |
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

Total 856. Goal: the hardened runtime ([#1893](https://github.com/appwrite-labs/swoole-rs/issues/1893), [#1902](https://github.com/appwrite-labs/swoole-rs/issues/1902)).
