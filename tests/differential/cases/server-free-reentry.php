<?php
// A Server whose last reference goes releases an addProcess() process whose
// destructor calls the server's methods through a Connection\Iterator it kept.
set_error_handler(null);
class Child extends Swoole\Process
{
    public $connections;

    public function __destruct()
    {
        echo "  destruct\n";
        s('  offsetExists', fn() => isset($this->connections[1]));
        s('  count', fn() => count($this->connections));
    }
}
$serv = s('server.new', fn() => new Swoole\Server('127.0.0.1', 0, SWOOLE_BASE));
$child = new Child(function () {});
$child->connections = $serv->connections;
s('addProcess', fn() => $serv->addProcess($child));
$child = null;
$serv = null;
echo "after\n";
