<?php
Co\run(static function (): void {
    $client = new Swoole\Coroutine\Http2\Client('127.0.0.1', (int) getenv('TEST_PORT'));
    if (!$client->connect()) {
        throw new RuntimeException('connect failed');
    }
    $header = new class($client) {
        public int $streamId;

        public function __construct(private Swoole\Coroutine\Http2\Client $client) {}

        public function __toString(): string
        {
            $inner = new Swoole\Http2\Request();
            $inner->path = '/echo';
            $inner->headers = ['x-inner' => 'inner'];
            $this->streamId = $this->client->send($inner);
            return 'outer';
        }
    };
    $outer = new Swoole\Http2\Request();
    $outer->path = '/echo';
    $outer->headers = ['x-outer' => $header];
    $outerId = $client->send($outer);
    $received = [];
    for ($i = 0; $i < 2; ++$i) {
        $response = $client->recv(5);
        if (!$response) {
            throw new RuntimeException('missing response');
        }
        $received[$response->streamId] = json_decode($response->data, true, flags: JSON_THROW_ON_ERROR)['header'];
    }
    echo json_encode([[$header->streamId, $outerId], $received], JSON_THROW_ON_ERROR), "\n";
    $client->close();
});
