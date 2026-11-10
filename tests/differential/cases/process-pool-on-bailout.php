<?php
// Process\Pool::on replaces a callable whose destructor ends the request: upstream
// stops there, so the second callable is never installed and on() returns nothing.
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
$pool = new Swoole\Process\Pool(1);
$stop = new Stop();
s('on', fn() => $pool->on('workerStart', function () use ($stop) {}));
$stop = null;
s('on again', fn() => $pool->on('workerStart', fn() => null));
echo "after\n";
