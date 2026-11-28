<?php
// A server that is never started: every call that needs a running server refuses.
$server = s('server.new', fn() => new Swoole\Server('127.0.0.1', 0, SWOOLE_BASE));
s('server.bind', fn() => $server->bind(1, 2));
s('server.getClientList', fn() => $server->getClientList());
s('server.getClientList.args', fn() => $server->getClientList(0, 10));
s('server.getClientList.negative', fn() => $server->getClientList(-1, -1));
s('server.heartbeat', fn() => $server->heartbeat());
s('server.heartbeat.close', fn() => $server->heartbeat(true));
s('server.protect', fn() => $server->protect(1));
s('server.protect.off', fn() => $server->protect(1, false));
s('port.new', fn() => new Swoole\Server\Port());
$connections = $server->connections;
s('connections.class', fn() => get_class($connections));
s('connections.count', fn() => count($connections));
s('connections.offsetExists', fn() => isset($connections[1]));
s('connections.offsetGet', fn() => $connections[1]);
s('connections.offsetSet', fn() => $connections[1] = 2);
s('connections.offsetUnset', function () use ($connections) { unset($connections[1]); });
s('connections.iterate', fn() => iterator_to_array($connections));
s('iterator.new', fn() => new Swoole\Connection\Iterator());
s('pool.new', fn() => new Swoole\Process\Pool(1));
