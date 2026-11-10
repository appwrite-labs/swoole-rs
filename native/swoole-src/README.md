# native/swoole-src

Files from swoole-src v6.2.5 (76c657d), at the paths they have there. crates/ext's build.rs
compiles these, not the pin's.

Some carry the patch set's changes: `php_swoole.h`, the curl hooks, `proc_open`, `plain_wrapper`
and the pdo_sqlite driver and statement. The rest are byte-for-byte copies of code swoole-src
bundles from elsewhere and that has no Rust replacement:

- `thirdparty/php*/pdo_*`, `thirdparty/pdo_oci`, `thirdparty/pdo_sqlite`: PHP's own PDO
  drivers, adapted by Swoole to its coroutine hooks. PHP License 3.01, as each file's header says.
- `thirdparty/php/ssh2`: PECL ssh2, adapted the same way. PHP License 3.01 (`LICENSE`).
- `thirdparty/boost/asm`: boost.context's switch for the architectures build.rs supports. Boost
  Software License 1.0 (`LICENSE`).
