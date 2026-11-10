<?php
// Compression per request (#2649): POST /gzip echoes its JSON body gzip-encoded (the client
// sends Accept-Encoding: gzip), and a WebSocket message comes back compressed with
// permessage-deflate (the client negotiates it).
$server = new Swoole\WebSocket\Server('127.0.0.1', (int) getenv('TEST_PORT'), SWOOLE_BASE);
$server->set(['worker_num' => 1, 'http_compression' => true, 'websocket_compression' => true]);
$server->on('request', static function ($request, $response): void {
    $response->header('Content-Type', 'application/json');
    $response->end($request->getContent());
});
$server->on('message', static function ($server, $frame): void {
    $server->push($frame->fd, $frame->data, WEBSOCKET_OPCODE_TEXT, SWOOLE_WEBSOCKET_FLAG_FIN | SWOOLE_WEBSOCKET_FLAG_COMPRESS);
});
$server->start();
