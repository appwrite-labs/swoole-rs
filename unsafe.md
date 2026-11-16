# Unsafe code by crate

`unsafe` blocks, fns and impls in non-test code of each crate under `crates/`, counted by `tools/budget.py` on `0a6522b`. ✅ marks a crate whose `lib.rs` has `#![forbid(unsafe_code)]`. The PR tier fails a pull request that raises a crate's count or drops its `forbid`.

| crate | unsafe | forbids |
| --- | ---: | :---: |
| `zend` | 103 |  |
| `server` | 77 |  |
| `hooks` | 67 |  |
| `corebase` | 51 |  |
| `coroutine` | 44 |  |
| `process` | 33 |  |
| `reactor` | 31 |  |
| `phphttp` | 25 |  |
| `ports` | 25 |  |
| `phpcoroutines` | 24 |  |
| `memory` | 23 |  |
| `tls` | 23 |  |
| `curl` | 18 |  |
| `phpclients` | 18 |  |
| `clients` | 16 |  |
| `phpruntime` | 16 |  |
| `phpstdext` | 16 |  |
| `phpswoole` | 16 |  |
| `buffer` | 12 |  |
| `timer` | 10 |  |
| `phpprocess` | 9 |  |
| `bus` | 8 |  |
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
| `websocket-ffi` | 0 |  |
| `zend-sys` | 0 |  |

Total 730. Goal: the hardened runtime ([#1893](https://github.com/appwrite-labs/swoole-rs/issues/1893), [#1902](https://github.com/appwrite-labs/swoole-rs/issues/1902)).
