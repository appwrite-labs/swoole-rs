<?php
// A Coroutine\Http\Server whose settings carry package_length_func: each connection's
// Coroutine\Socket frees its own copy of the callable, never the listener's.
Swoole\Coroutine\run(function () {
    $server = new Swoole\Coroutine\Http\Server('127.0.0.1', 0);
    $server->set(['package_length_func' => static fn($data) => -1]);
    $server->handle('/', static fn($request, $response) => $response->end('ok'));
    go(static fn() => $server->start());
    for ($i = 0; $i < 3; $i++) {
        $client = new Swoole\Coroutine\Http\Client('127.0.0.1', $server->port);
        $client->set(['keep_alive' => false]);
        $client->get('/');
        s("get $i", fn() => [$client->statusCode, $client->body]);
        $client->close();
        Swoole\Coroutine::sleep(0.05);
    }
    $server->shutdown();
});
echo "after\n";
