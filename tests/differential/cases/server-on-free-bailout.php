<?php
// Server::on replaces a callable whose destructor ends the request: upstream stops
// there, so on() returns nothing and the second callable is never installed.
set_error_handler(null);
class Stop
{
    public function __destruct()
    {
        echo "  destruct\n";
        trigger_error('stop', E_USER_ERROR);
    }
}
register_shutdown_function(static fn() => print("shutdown\n"));
$serv = s('server.new', fn() => new Swoole\Server('127.0.0.1', 0, SWOOLE_BASE));
$stop = new Stop();
s('on', fn() => $serv->on('receive', function () use ($stop) {}));
$stop = null;
s('on again', fn() => $serv->on('receive', function () {}));
echo "after\n";
