# Unsafe code by crate

`unsafe` blocks, fns and impls in non-test code of each crate under `crates/`, counted by `tools/budget.py` on `fc5fa69`. ✅ marks a crate whose `lib.rs` has `#![forbid(unsafe_code)]`. The PR tier fails a pull request that raises a crate's count or drops its `forbid`.

| crate | unsafe | forbids |
| --- | ---: | :---: |
| `server` | 835 |  |
| `zend` | 501 |  |
| `tls` | 444 |  |
| `hooks` | 362 |  |
| `reactor` | 262 |  |
| `phpserver` | 242 |  |
| `phpcoroutines` | 241 |  |
| `process` | 237 |  |
| `phpclients` | 227 |  |
| `corebase` | 204 |  |
| `phpruntime` | 203 |  |
| `phphttp` | 187 |  |
| `phpstdext` | 187 |  |
| `ports` | 170 |  |
| `coroutine` | 166 |  |
| `phpswoole` | 163 |  |
| `clients` | 161 |  |
| `curl` | 106 |  |
| `memory` | 103 |  |
| `phpprocess` | 98 |  |
| `buffer` | 85 |  |
| `phpthread` | 74 |  |
| `ossupport` | 64 |  |
| `protocols-ffi` | 62 |  |
| `http-ffi` | 58 |  |
| `phphttp2` | 55 |  |
| `locks` | 53 |  |
| `timer` | 50 |  |
| `pgsql` | 48 |  |
| `ssh2` | 47 |  |
| `utilities` | 46 |  |
| `odbc` | 41 |  |
| `bus` | 31 |  |
| `oci` | 31 |  |
| `websocket-ffi` | 28 |  |
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
| `uring-sys` | 0 |  |
| `websocket` | 0 | ✅ |
| `zend-sys` | 0 |  |

Total 5,974. Goal: the hardened runtime ([#1893](https://github.com/appwrite-labs/swoole-rs/issues/1893), [#1902](https://github.com/appwrite-labs/swoole-rs/issues/1902)).
