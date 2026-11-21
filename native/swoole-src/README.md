# native/swoole-src

Files from swoole-src v6.2.6 (13d2f33), at the paths they have there. crates/ext's build.rs
compiles these; it reads nothing from the pin.

Some carry changes the deleted patch set made: `php_swoole.h`, the curl hooks, `proc_open`
and the pdo_sqlite driver and statement (`plain_wrapper.c` is now `native/hooks/plain_wrapper.c`, #5519). The rest are byte-for-byte copies of code swoole-src
bundles from elsewhere and that has no Rust replacement:

- `thirdparty/php*/pdo_*`, `thirdparty/pdo_oci`, `thirdparty/pdo_sqlite`: PHP's own PDO
  drivers, adapted by Swoole to its coroutine hooks. PHP License 3.01 (`thirdparty/LICENSE`), as each file's header says.
- `thirdparty/php/ssh2`: PECL ssh2, adapted the same way. PHP License 3.01.
- `thirdparty/LICENSE-ZEND`: the Zend License 2.00, copied verbatim from
  [PHP 8.4.0's Zend license](https://github.com/php/php-src/blob/php-8.4.0/Zend/LICENSE),
  for `native/hooks/zend_execute_private.h`, the part of Swoole's
  `thirdparty/php/zend/zend_execute.c` that `crates/phpstdext` still calls (#5519).
