<?php
$block = str_repeat(implode('', array_map(chr(...), range(0, 255))), 256);
$payload = '';
for ($i = 0; $i < 64; ++$i) $payload .= pack('N', $i) . substr($block, 4);
$file = getenv('TEST_DATA') . '/payload.bin';
file_put_contents($file, $payload);
$server = new Swoole\Http\Server('127.0.0.1', (int) getenv('TEST_PORT'), SWOOLE_BASE);
$server->set([
    'worker_num' => 1, 'enable_coroutine' => false, 'http_compression' => false,
    'socket_buffer_size' => 16 * 1024 * 1024, 'output_buffer_size' => 16 * 1024 * 1024,
    'kernel_socket_send_buffer_size' => 8192,
]);
$server->on('request', static function ($request, $response) use ($server, $payload, $file): void {
    if ($request->server['request_uri'] === '/stream') {
        $response->header('Content-Type', 'application/octet-stream');
        for ($i = 0; $i < 64; ++$i) {
            if (!$response->write(substr($payload, $i * 65536, 65536))) throw new RuntimeException('write failed');
        }
        $info = $server->getClientInfo($request->fd);
        file_put_contents(getenv('TEST_DATA') . '/queued.json', json_encode($info));
        if (!$response->end()) throw new RuntimeException('end failed');
    } elseif ($request->server['request_uri'] === '/queue') {
        $response->end(file_get_contents(getenv('TEST_DATA') . '/queued.json'));
    } elseif ($request->server['request_uri'] === '/file') {
        if (!$response->sendfile($file, 13, strlen($payload) - 27)) throw new RuntimeException('sendfile failed');
    } else {
        $response->end('ok');
    }
});
$server->start();
