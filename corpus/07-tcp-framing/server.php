<?php
$server = new Swoole\Server('127.0.0.1', (int) getenv('TEST_PORT'), SWOOLE_BASE);
$server->set([
    'worker_num' => 1,
    'open_length_check' => true,
    'package_length_type' => 'N',
    'package_length_offset' => 0,
    'package_body_offset' => 4,
    'package_max_length' => 131072,
]);
$server->on('receive', static function ($server, $fd, $reactorId, $packet): void {
    // Reply with the exact framed binary message received by the callback.
    $server->send($fd, $packet);
});
$server->start();
