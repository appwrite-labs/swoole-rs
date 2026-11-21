<?php
// TLS per request (#2648): HTTP/1.1 and a WebSocket echo over TLS, with the pin's test
// certificate (RSA 4096, CN 127.0.0.1) so both engines serve the same key.
$certs = __DIR__ . '/../../target/upstream/src/tests/include/ssl_certs';
$server = new Swoole\WebSocket\Server('127.0.0.1', (int) getenv('TEST_PORT'), SWOOLE_BASE, SWOOLE_SOCK_TCP | SWOOLE_SSL);
$server->set(['worker_num' => 1, 'ssl_cert_file' => "$certs/server.crt", 'ssl_key_file' => "$certs/server.key"]);
$server->on('request', static fn($request, $response) => $response->end("ok\n"));
$server->on('message', static function ($server, $frame): void {
    $server->push($frame->fd, $frame->data);
});
$server->start();
