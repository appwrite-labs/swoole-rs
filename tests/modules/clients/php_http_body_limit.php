<?php
use Swoole\Coroutine\Http\Client;
use Swoole\Coroutine\Http\Server;

Co\run(static function (): void {
    $server = new Server('127.0.0.1', 0);
    $server->handle('/', static function ($request, $response): void {
        $size = (int) $request->get['size'];
        $response->write(str_repeat('a', 64));
        $response->end(str_repeat('b', $size - 64));
    });
    go(static fn () => $server->start());

    $client = new Client('127.0.0.1', $server->port);
    try {
        foreach ([128, 256] as $limit) {
            $client->set(['package_max_length' => $limit]);
            if (!$client->get("/?size=$limit") || $client->body !== str_repeat('a', 64) . str_repeat('b', $limit - 64)) {
                throw new RuntimeException("response at $limit bytes was not returned intact");
            }
        }
        if ($client->get('/?size=257') || $client->errCode !== SWOOLE_ERROR_PACKAGE_LENGTH_TOO_LARGE || $client->body !== '') {
            throw new RuntimeException('oversized response must fail with the package-length error and an empty body');
        }
    } finally {
        $client->close();
        $server->shutdown();
    }
});
echo "passed\n";
