<?php
// One Swoole\Coroutine\Http2\Client shared by two coroutines, against
// test_concurrent.py's frame server: while one is parked in recv(), the other
// calls stats(), isStreamExist(), send() and ping() on the same client, then
// receives the second response itself.
Co\run(static function (): void {
    $client = new Swoole\Coroutine\Http2\Client('127.0.0.1', (int) getenv('TEST_PORT'));
    if (!$client->connect()) {
        throw new RuntimeException('connect failed');
    }
    $request = new Swoole\Http2\Request();
    $request->path = '/one';
    echo 'send ', $client->send($request), "\n";
    $done = new Swoole\Coroutine\Channel(1);
    go(static function () use ($client, $done): void {
        $response = $client->recv(5);
        echo "recv {$response->streamId} {$response->statusCode} {$response->data}\n";
        $done->push(true);
    });
    echo 'active ', $client->stats('active_stream_num'), "\n";
    echo 'exists ', var_export($client->isStreamExist(1), true), "\n";
    $request->path = '/two';
    // Converting the body runs PHP code mid-send: the stream id is already taken.
    $request->data = new class($client) {
        public function __construct(private Swoole\Coroutine\Http2\Client $client) {}

        public function __toString(): string
        {
            echo 'body, current ', $this->client->stats('current_stream_id'), "\n";
            return 'two';
        }
    };
    $id = $client->send($request);
    echo "send $id\n";
    echo 'current ', $client->stats('current_stream_id'), "\n";
    echo 'ping ', var_export($client->ping(), true), "\n";
    $done->pop(5);
    $response = $client->recv(5);
    echo "recv {$response->streamId} {$response->statusCode} {$response->data}\n";
    echo 'active ', $client->stats('active_stream_num'), "\n";
    echo 'close ', var_export($client->close(), true), "\n";
});
