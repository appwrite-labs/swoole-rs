<?php
// A descriptor warning whose error handler bails out (memory_limit), from inside
// proc_open's spawn: the bailout leaves proc_open as on upstream (#5545).
ini_set('memory_limit', '16M');
Swoole\Runtime::enableCoroutine(SWOOLE_HOOK_PROC);
set_error_handler(static function (int $level, string $message) {
    echo "handler: $message\n";
    $bytes = str_repeat('x', 64 * 1024 * 1024);
});
register_shutdown_function(static fn() => print("SHUTDOWN\n"));
Swoole\Coroutine\run(static function () {
    proc_open(['/bin/echo', 'x'], [1 => ['bogus']], $pipes);
    echo "after\n";
});
