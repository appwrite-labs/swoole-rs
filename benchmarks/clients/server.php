<?php
// Outbound coroutine clients per request (#2647), each on a connection the worker keeps open:
// a Coroutine\Http\Client GET, and a 1 KiB send/recv round trip through Coroutine\Client and
// through Coroutine\Socket. The peers (PEER_HTTP_PORT answers "pong", PEER_ECHO_PORT echoes)
// run in benchmarks/callgrind.py, outside valgrind, so only the client side is counted.
require __DIR__ . '/../../corpus/bootstrap.php';

const SIZE = 1024;

$server = httpServer();
$clients = [];
$server->on('request', static function ($request, $response) use (&$clients): void {
    $payload = str_repeat('x', SIZE);
    switch ($request->server['request_uri']) {
        case '/http':
            $http = $clients['http'] ??= new Swoole\Coroutine\Http\Client('127.0.0.1', (int) getenv('PEER_HTTP_PORT'));
            $http->get('/');
            $ok = $http->statusCode === 200 && $http->body === 'pong';
            break;
        case '/client':
            if (!isset($clients['client'])) {
                $clients['client'] = new Swoole\Coroutine\Client(SWOOLE_SOCK_TCP);
                $clients['client']->connect('127.0.0.1', (int) getenv('PEER_ECHO_PORT'));
            }
            $clients['client']->send($payload);
            $reply = '';
            while (strlen($reply) < SIZE && ($chunk = $clients['client']->recv()) !== false && $chunk !== '') {
                $reply .= $chunk;
            }
            $ok = $reply === $payload;
            break;
        case '/socket':
            if (!isset($clients['socket'])) {
                $clients['socket'] = new Swoole\Coroutine\Socket(AF_INET, SOCK_STREAM, 0);
                $clients['socket']->connect('127.0.0.1', (int) getenv('PEER_ECHO_PORT'));
            }
            $clients['socket']->sendAll($payload);
            $ok = $clients['socket']->recvAll(SIZE) === $payload;
            break;
        default:
            $ok = false;
    }
    $response->status($ok ? 200 : 502);
    $response->end($ok ? "ok\n" : "peer failed\n");
});
$server->start();
