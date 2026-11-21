<?php
// Coroutine\Socket::setProtocol replaces a package_length_func whose destructor ends
// the request: upstream stops there, so setProtocol returns nothing.
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
$socket = new Swoole\Coroutine\Socket(AF_INET, SOCK_STREAM, 0);
$stop = new Stop();
s('setProtocol', fn() => $socket->setProtocol(['open_length_check' => true, 'package_length_func' => function () use ($stop) {}]));
$stop = null;
s('setProtocol again', fn() => $socket->setProtocol(['package_length_func' => fn() => 0]));
echo "after\n";
