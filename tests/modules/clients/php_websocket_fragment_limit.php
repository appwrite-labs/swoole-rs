<?php
use Swoole\Coroutine;
use Swoole\Coroutine\Http\Client;
use Swoole\Coroutine\Http\Server;

Coroutine\run(static function (): void {
    $server = new Server('127.0.0.1', 0);
    $server->handle('/http', static fn ($request, $response) => $response->end('ok'));
    $server->handle('/', static function ($request, $response): void {
        $response->upgrade();
        foreach ([[1, 32, 0], [0, 32, 1], [1, 32, 0], [0, 33, 0], [0, 1, 1], [1, 2, 1]] as [$opcode, $size, $flags]) {
            $response->push(str_repeat('x', $size), $opcode, $flags);
        }
    });
    Coroutine::create(static fn () => $server->start());
    $client = new Client('127.0.0.1', $server->port);
    $client->set(['package_max_length' => 64]);
    try {
        // An HTTP client has no frames: push() fails with the handshake error and a warning, ping() with none.
        // errCode is the warning's own last error (php_swoole_fatal_error), as upstream's.
        $warnings = [];
        set_error_handler(static function (int $severity, string $message) use (&$warnings): bool {
            $warnings[] = [$severity, $message];
            return true;
        });
        $refused = $client->get('/http') && $client->push('x') === false;
        $refused = $refused && $client->errCode === SWOOLE_ERROR_PHP_FATAL_ERROR
            && $client->errMsg === 'websocket handshake failed, cannot push data'
            && $client->statusCode === SWOOLE_HTTP_CLIENT_ESTATUS_CONNECT_FAILED && $client->ping() === false;
        restore_error_handler();
        if (!$refused || $warnings !== [[E_WARNING, 'Swoole\\Coroutine\\Http\\Client::push(): websocket handshake failed, cannot push data']]) {
            throw new RuntimeException('push before upgrade: ' . json_encode($warnings));
        }
        if (!$client->upgrade('/') || $client->recv(2)->data !== str_repeat('x', 64)) {
            throw new RuntimeException('exact limit');
        }
        if ($client->recv(2) !== false || swoole_last_error() !== SWOOLE_ERROR_PACKAGE_LENGTH_TOO_LARGE) {
            throw new RuntimeException('non-final continuation over limit');
        }
        if ($client->recv(2)->data !== 'xx') {
            throw new RuntimeException('next complete message');
        }
        echo "passed\n";
    } finally {
        $client->close();
        $server->shutdown();
    }
});
