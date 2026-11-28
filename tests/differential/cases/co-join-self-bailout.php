<?php
// An error handler that ends the request on Co::join's "can not join self": upstream
// stops there, so join returns nothing and the coroutine runs no further.
set_error_handler(static function (int $severity, string $message): bool {
    echo "  warning($severity): $message\n";
    trigger_error('stop', E_USER_ERROR);
});
register_shutdown_function(static fn() => print('last error = ' . swoole_last_error() . "\n"));
Swoole\Coroutine\run(static function () {
    s('join', fn() => Swoole\Coroutine::join([Swoole\Coroutine::getCid()]));
    echo "after join\n";
});
