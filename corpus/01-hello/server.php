<?php
require __DIR__ . '/../bootstrap.php';
$server = httpServer();
$server->on('request', static function ($request, $response): void {
    $response->header('Content-Type', 'text/plain');
    $response->end("Hello, Swoole!\n");
});
$server->start();
