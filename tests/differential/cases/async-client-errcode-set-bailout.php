<?php
// A Swoole\Async\Client subclass hooks errCode (PHP 8.4), so a failed connect's write of
// the error runs the hook, and the hook ends the request: upstream stops there, so the
// onError callback after the write never runs. On PHP 8.3 the hook is a parse error on every engine.
set_error_handler(null);
class Client extends Swoole\Async\Client
{
    public $errCode {
        set($value) {
            echo "  set errCode\n";
            trigger_error('stop', E_USER_ERROR);
        }
    }
}
register_shutdown_function(static fn() => print("shutdown\n"));
$client = new Client(SWOOLE_SOCK_TCP);
foreach (['connect', 'receive', 'close'] as $event) {
    $client->on($event, static fn() => print("  on$event\n"));
}
$client->on('error', static fn() => print("  onError\n"));
s('connect', fn() => $client->connect('127.0.0.1', 1, 0.5));
Swoole\Event::wait();
echo "after\n";
