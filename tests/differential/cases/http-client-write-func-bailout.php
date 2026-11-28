<?php
// Coroutine\Http\Client::set replaces a write_func whose destructor ends the request:
// upstream stops there, so set() returns nothing.
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
$client = new Swoole\Coroutine\Http\Client('127.0.0.1', 1);
$stop = new Stop();
s('set', fn() => $client->set(['write_func' => function () use ($stop) {}]));
$stop = null;
s('set again', fn() => $client->set(['write_func' => function () {}]));
echo "after\n";
