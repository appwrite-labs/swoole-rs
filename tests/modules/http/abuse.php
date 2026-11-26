<?php
// One worker serving HTTP/1, h2c and WebSocket at default limits; test_abuse.py measures its RSS.
// send_timeout is the deadline a send waits on a peer's HTTP/2 window (60 s when unset).
$server = new Swoole\WebSocket\Server('127.0.0.1', (int) getenv('TEST_PORT'), SWOOLE_BASE);
$server->set(['worker_num' => 1, 'log_level' => SWOOLE_LOG_NONE, 'websocket_compression' => true,
    'open_http2_protocol' => true, 'send_timeout' => 1]);
$server->on('request', static function ($request, $response): void {
    $response->end(count($request->header ?? []) . ' ' . strlen((string) $request->getContent()));
});
$server->on('message', static function ($server, $frame): void {
    $server->push($frame->fd, (string) strlen($frame->data));
});
$server->start();
