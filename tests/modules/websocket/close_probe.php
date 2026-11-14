<?php
// Independent diagnostic: retain the client until both server responses arrive.
$server = new Swoole\WebSocket\Server('127.0.0.1', (int) getenv('TEST_PORT'), SWOOLE_BASE);
$server->set(['worker_num' => 1]);
$server->on('open', static function ($server, $request): void {});
$server->on('message', static function ($server, $frame): void {
    if ($frame->data === 'aaa') {
        $reply = new Swoole\WebSocket\CloseFrame();
        $reply->code = 1000;
        $reply->reason = 'diagnostic';
    } else {
        $reply = $frame->data;
    }
    if (!$server->push($frame->fd, $reply)) {
        throw new RuntimeException('push failed');
    }
});
$server->on('request', static fn($request, $response) => $response->end('healthy'));
$server->start();
