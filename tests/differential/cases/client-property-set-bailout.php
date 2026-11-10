<?php
// A Swoole\Client subclass hooks errCode (PHP 8.4), so a failed connect's write of the
// error runs the hook, and the hook ends the request: upstream stops there, so the
// connect warning after the write is never raised. On PHP 8.3 the hook is a parse error
// on every engine.
class Client extends Swoole\Client
{
    public $errCode {
        set($value) {
            echo '  set errCode = ' . r($value) . "\n";
            trigger_error('hook', E_USER_WARNING);
        }
    }
}
set_error_handler(static function (int $severity, string $message): bool {
    echo "  warning($severity): $message\n";
    trigger_error('stop', E_USER_ERROR);
});
$client = new Client(SWOOLE_SOCK_TCP);
register_shutdown_function(static fn() => print("shutdown\n"));
s('client.connect', fn() => $client->connect('127.0.0.1', 1, 0.5));
