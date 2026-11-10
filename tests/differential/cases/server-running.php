<?php
// One BASE-mode worker, the only process: the calls run on a live server with one
// connection, which the worker opens to itself.
$server = new Swoole\Server('127.0.0.1', 0, SWOOLE_BASE);
$server->set(['worker_num' => 1, 'log_level' => SWOOLE_LOG_NONE, 'heartbeat_idle_time' => 600, 'heartbeat_check_interval' => 60]);
$server->on('receive', function ($server, $fd, $reactor, $data) {
    s('receive', fn() => $data);
    s('getClientList', fn() => $server->getClientList());
    s('getClientList.after', fn() => $server->getClientList($fd));
    s('getClientList.count', fn() => $server->getClientList(0, 0));
    s('getClientList.max', fn() => $server->getClientList(0, 101));
    s('connections.offsetExists', fn() => isset($server->connections[$fd]));
    s('connections.offsetExists.missing', fn() => isset($server->connections[$fd + 100]));
    s('connections.offsetGet', fn() => array_intersect_key($server->connections[$fd], ['socket_type' => 1, 'remote_ip' => 1, 'reactor_id' => 1]));
    s('connections.offsetSet', function () use ($server, $fd) { $server->connections[$fd] = 1; });
    s('connections.offsetUnset', function () use ($server, $fd) { unset($server->connections[$fd]); });
    s('connections.count', fn() => count($server->connections));
    s('bind', fn() => $server->bind($fd, 7));
    s('bind.again', fn() => $server->bind($fd, 8));
    s('bind.missing', fn() => $server->bind($fd + 100, 7));
    s('protect', fn() => $server->protect($fd));
    s('protect.off', fn() => $server->protect($fd, false));
    s('protect.missing', fn() => $server->protect($fd + 100));
    s('heartbeat', fn() => $server->heartbeat(false));
    s('heartbeat.close', fn() => $server->heartbeat(true));
    // Only with ext/sockets.
    if (method_exists($server->ports[0], 'getSocket')) {
        s('port.getSocket', fn() => get_class($server->ports[0]->getSocket()));
    }
    $server->shutdown();
});
$server->on('workerStart', function ($server) {
    s('workerStart.getClientList', fn() => $server->getClientList());
    go(function () use ($server) {
        $client = new Swoole\Coroutine\Client(SWOOLE_SOCK_TCP);
        $client->connect('127.0.0.1', $server->port, 1);
        $client->send('ping');
        Swoole\Coroutine::sleep(0.5);
    });
});
$server->start();
s('stopped', fn() => true);
