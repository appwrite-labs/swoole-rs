# native/swoole-src

What is left of swoole-src v6.2.6 (13d2f33) at the paths it has there:

- `php_swoole.h` and `php_swoole_library.h`, which the C in `native/hooks` and `native/phphelpers` includes.
- `thirdparty/LICENSE`: the PHP License 3.01, for what `native/hooks` keeps of PHP's
  `proc_open`, plain-files wrapper and PDO drivers.
- `thirdparty/LICENSE-ZEND`: the Zend License 2.00, copied verbatim from
  [PHP 8.4.0's Zend license](https://github.com/php/php-src/blob/php-8.4.0/Zend/LICENSE),
  for `native/hooks/zend_execute_private.h`, the part of Swoole's
  `thirdparty/php/zend/zend_execute.c` that `crates/phpstdext` still calls.
