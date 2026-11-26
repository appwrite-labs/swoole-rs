<?php
// Independent diagnostic: retain the client until both server responses arrive.
// TEST_HANDSHAKE (base or process) adds upstream 6.3's early_frame_before_handshake
// server: a custom handshake that yields, then accepts only `X-Token: good`.
$handshake = getenv('TEST_HANDSHAKE');
$mode = $handshake === 'process' ? SWOOLE_PROCESS : SWOOLE_BASE;
$server = new Swoole\WebSocket\Server('127.0.0.1', (int) getenv('TEST_PORT'), $mode);
$server->set(['worker_num' => 1]);
$messages = 0;
$server->on('open', static function ($server, $request): void {});
$server->on('message', static function ($server, $frame) use (&$messages): void {
    $messages++;
    if ($frame->data === 'aaa') {
        $reply = new Swoole\WebSocket\CloseFrame();
        $reply->code = 1000;
        $reply->reason = 'diagnostic';
    } else {
        $reply = $frame->data;
    }
    if (!$server->push($frame->fd, $reply)) {
        throw new RuntimeException('push failed');
    }
});
if ($handshake) {
    $server->on('handshake', static function ($request, $response): bool {
        Swoole\Coroutine::sleep(0.3);
        if (($request->header['x-token'] ?? '') !== 'good') {
            $response->status(403);
            $response->end();
            return false;
        }
        $key = $request->header['sec-websocket-key'] . '258EAFA5-E914-47DA-95CA-C5AB0DC85B11';
        $response->header('Upgrade', 'websocket');
        $response->header('Connection', 'Upgrade');
        $response->header('Sec-WebSocket-Accept', base64_encode(sha1($key, true)));
        $response->status(101);
        $response->end();
        return true;
    });
}
$server->on('request', static function ($request, $response) use (&$messages): void {
    $response->end($request->server['request_uri'] === '/messages' ? (string) $messages : 'healthy');
});
$server->start();
