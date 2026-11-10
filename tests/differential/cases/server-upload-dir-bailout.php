<?php
// An error handler that ends the request while Server::set converts upload_tmp_dir:
// upstream stops at the conversion, so no directory is created or checked after it.
set_error_handler(static function (int $severity, string $message): bool {
    echo "  warning($severity): $message\n";
    trigger_error('stop', E_USER_ERROR);
});
$serv = s('server.new', fn() => new Swoole\Http\Server('127.0.0.1', 0, SWOOLE_BASE));
s('server.set', fn() => $serv->set(['upload_tmp_dir' => []]));
