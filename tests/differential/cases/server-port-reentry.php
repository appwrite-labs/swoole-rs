<?php
// Server\Port methods called on the same port from PHP code its own methods run:
// set()'s option check warns into an error handler that calls set() and on(), and
// on() replaces a callable whose destructor calls on() again.
set_error_handler(null);
class Again
{
    public function __construct(private Swoole\Server\Port $port)
    {
    }

    public function __destruct()
    {
        echo "  destruct\n";
        s('  nested on', fn() => $this->port->on('receive', function () {}));
    }
}
$serv = s('server.new', fn() => new Swoole\Server('127.0.0.1', 0, SWOOLE_BASE));
$port = s('listen', fn() => $serv->listen('127.0.0.1', 0, SWOOLE_SOCK_TCP));
set_error_handler(static function (int $severity, string $message) use ($port): bool {
    echo "  warning($severity): $message\n";
    s('  nested set', fn() => $port->set(['open_eof_check' => false]));
    s('  nested on', fn() => $port->on('close', function () {}));
    return true;
});
s('set', fn() => $port->set(['no_such_option' => 1, 'open_length_check' => false]));
echo 'setting = ' . r($port->setting) . "\n";
$again = new Again($port);
s('on', fn() => $port->on('receive', function () use ($again) {}));
$again = null;
s('on again', fn() => $port->on('receive', function () {}));
s('getCallback', fn() => $port->getCallback('receive') instanceof Closure);
echo "after\n";
