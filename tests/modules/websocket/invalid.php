<?php
// Raw-policy compatibility gate; all socket ownership remains in Swoole.
$server = new Swoole\WebSocket\Server('127.0.0.1', (int) getenv('TEST_PORT'), SWOOLE_BASE);
$server->set(['worker_num' => 1, 'package_max_length' => 65536]);
$server->on('open', static function ($server, $request): void {});
$server->on('message', static function ($server, $frame): void {
    if (!$server->push($frame->fd, $frame->data)) {
        throw new RuntimeException('echo push failed');
    }
});
$server->on('request', static fn($request, $response) => $response->end('healthy'));
$server->start();
