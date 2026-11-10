<?php
// A Swoole\Async\Client is freed while its callback, holding the closure's last
// reference, has a destructor that ends the request: upstream stops there, so the
// code after the free never runs.
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
$client = new Swoole\Async\Client(SWOOLE_SOCK_TCP);
$stop = new Stop();
s('on', fn() => $client->on('connect', function () use ($stop) {}));
$stop = null;
// The private onConnect property held the closure too: the callable now holds the last reference.
(new ReflectionProperty($client, 'onConnect'))->setValue($client, null);
s('free', function () use (&$client) {
    $client = null;
    return 'freed';
});
echo "after\n";
