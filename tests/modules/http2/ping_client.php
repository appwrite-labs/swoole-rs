<?php
// Swoole\Coroutine\Http2\Client against test_client.py's frame server: recv() answers
// its PING and returns false at its GOAWAY.
Co\run(static function (): void {
    $client = new Swoole\Coroutine\Http2\Client('127.0.0.1', (int) getenv('TEST_PORT'));
    if (!$client->connect()) {
        throw new RuntimeException('connect failed');
    }
    var_dump($client->recv(5));
});
