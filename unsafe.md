# Unsafe code by crate

`unsafe` blocks, fns and impls in non-test code of each crate under `crates/`, counted by `tools/budget.py` on `ce2d10f`. ✅ marks a crate whose `lib.rs` has `#![forbid(unsafe_code)]`. The PR tier fails a pull request that raises a crate's count or drops its `forbid`.

| crate | unsafe | forbids |
| --- | ---: | :---: |
| `server` | 743 |  |
| `zend` | 498 |  |
| `tls` | 427 |  |
| `hooks` | 333 |  |
| `reactor` | 260 |  |
| `phpcoroutines` | 229 |  |
| `process` | 228 |  |
| `phpserver` | 225 |  |
| `phpclients` | 222 |  |
| `corebase` | 202 |  |
| `phpruntime` | 199 |  |
| `phpstdext` | 186 |  |
| `phphttp` | 176 |  |
| `coroutine` | 161 |  |
| `ports` | 159 |  |
| `clients` | 151 |  |
| `phpswoole` | 151 |  |
| `curl` | 105 |  |
| `memory` | 96 |  |
| `buffer` | 84 |  |
| `phpprocess` | 83 |  |
| `phpthread` | 68 |  |
| `ossupport` | 62 |  |
| `protocols-ffi` | 61 |  |
| `http-ffi` | 58 |  |
| `locks` | 57 |  |
| `phphttp2` | 49 |  |
| `timer` | 48 |  |
| `pgsql` | 47 |  |
| `ssh2` | 47 |  |
| `odbc` | 41 |  |
| `utilities` | 33 |  |
| `bus` | 31 |  |
| `oci` | 31 |  |
| `websocket-ffi` | 26 |  |
| `sqlite` | 22 |  |
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
| `phplocks` | 0 | ✅ |
| `phpredis` | 0 | ✅ |
| `phpstubs` | 0 | ✅ |
| `phptimer` | 0 | ✅ |
| `protocols` | 0 | ✅ |
| `websocket` | 0 | ✅ |
| `zend-sys` | 0 |  |

Total 5,679. Goal: the hardened runtime ([#1893](https://github.com/appwrite-labs/swoole-rs/issues/1893), [#1902](https://github.com/appwrite-labs/swoole-rs/issues/1902)).
