<?php
// A Coroutine\Http\Client is freed while it holds a write_func whose destructor ends the
// request: upstream stops there, so the code after the free never runs.
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
s('free', function () use (&$client) {
    $client = null;
    return 'freed';
});
echo "after\n";
