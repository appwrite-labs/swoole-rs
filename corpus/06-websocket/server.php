<?php
$server = new Swoole\WebSocket\Server('127.0.0.1', (int) getenv('TEST_PORT'));
$server->set(['worker_num' => 1]);
$clients = [];
$server->on('open', static function ($server, $request) use (&$clients): void {
    $clients[$request->fd] = true;
});
$server->on('message', static function ($server, $frame) use (&$clients): void {
    foreach (array_keys($clients) as $fd) {
        $server->push($fd, $frame->data);
    }
});
$server->on('close', static function ($server, $fd) use (&$clients): void {
    unset($clients[$fd]);
});
$server->on('request', static fn($request, $response) => $response->end('websocket chat'));
$server->start();
