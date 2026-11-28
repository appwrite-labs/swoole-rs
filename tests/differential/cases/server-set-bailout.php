<?php
// An error handler that ends the request while Server::set converts a global option:
// upstream stops there, so the coroutine options after it are never applied.
set_error_handler(null);
register_shutdown_function(static fn() => print('options = ' . r(Swoole\Coroutine::getOptions()) . "\n"));
set_error_handler(static function (int $severity, string $message): bool {
    echo "  warning($severity): $message\n";
    trigger_error('stop', E_USER_ERROR);
});
$serv = s('server.new', fn() => new Swoole\Server('127.0.0.1', 0, SWOOLE_BASE));
s('server.set', fn() => $serv->set(['socket_connect_timeout' => new stdClass, 'enable_deadlock_check' => false]));
