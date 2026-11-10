<?php
// A frame's __toString ends the request while Coroutine\Http\Client::push packs it:
// upstream stops there, so the client records no pack failure.
set_error_handler(null);
Swoole\Coroutine\run(static function () {
    $server = new Swoole\Coroutine\Http\Server('127.0.0.1', 0);
    $server->handle('/', static function ($request, $response) {
        $response->upgrade();
        $response->recv(1);
    });
    go(static fn() => $server->start());
    $client = new Swoole\Coroutine\Http\Client('127.0.0.1', $server->port);
    s('upgrade', fn() => $client->upgrade('/'));
    register_shutdown_function(static fn() => print("errCode = {$client->errCode}, errMsg = " . r($client->errMsg) . "\n"));
    s('push', fn() => $client->push(new class {
        public function __toString(): string
        {
            trigger_error('stop', E_USER_ERROR);
            return '';
        }
    }));
});
