<?php
Co\run(static function (): void {
    $client = new Swoole\Coroutine\Http2\Client('127.0.0.1', (int) getenv('TEST_PORT'));
    $client->set(['package_max_length' => 128]);
    $client->connect();
    $client->send(new Swoole\Http2\Request());
    $client->send(new Swoole\Http2\Request());

    var_dump($client->recv(5) === false, $client->errCode === SWOOLE_ERROR_PACKAGE_LENGTH_TOO_LARGE);
    $response = $client->recv(5);
    var_dump($response->streamId, $response->data === str_repeat('b', 128));
    $client->close();
});
