<?php
// An error handler that ends the request while Co::set converts an option: upstream
// stops there, so neither the later options nor the options array are kept.
set_error_handler(static function (int $severity, string $message): bool {
    echo "  warning($severity): $message\n";
    trigger_error('stop', E_USER_ERROR);
});
register_shutdown_function(static fn() => print('options = ' . r(Swoole\Coroutine::getOptions()) . "\n"));
s('co.set', fn() => Swoole\Coroutine::set(['max_coroutine' => new stdClass, 'enable_deadlock_check' => false]));
