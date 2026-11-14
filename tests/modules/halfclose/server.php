<?php
$payload = str_repeat(implode('', array_map(chr(...), range(0, 255))), 16384);
$file = getenv('TEST_DATA') . '/payload';
file_put_contents($file, $payload);
$events = [];
$owners = [];
$server = new Swoole\Http\Server('127.0.0.1', (int) getenv('TEST_PORT'),
    getenv('TEST_MODE') === 'process' ? SWOOLE_PROCESS : SWOOLE_BASE);
$server->set([
    'worker_num' => 1, 'reactor_num' => 1, 'enable_coroutine' => false,
    'http_compression' => false, 'kernel_socket_send_buffer_size' => 8192,
    'socket_buffer_size' => 16 * 1024 * 1024, 'output_buffer_size' => 16 * 1024 * 1024,
    'max_wait_time' => 1,
]);
$server->on('close', static function ($server, $fd) use (&$events, &$owners): void {
    if (isset($owners[$fd])) ++$events[$owners[$fd]]['closes'];
});
$server->on('request', static function ($request, $response) use ($server, $payload, $file, &$events, &$owners): void {
    unset($owners[$request->fd]);
    $path = $request->server['request_uri'];
    if ($path === '/stats') {
        $response->end(json_encode($events));
        return;
    }
    $id = $request->get['id'];
    $owners[$request->fd] = $id;
    $events[$id] ??= ['requests' => 0, 'closes' => 0];
    ++$events[$id]['requests'];
    if ($path === '/stream') {
        for ($offset = 0; $offset < strlen($payload); $offset += 65536) {
            if (!$response->write(substr($payload, $offset, 65536))) throw new RuntimeException('write failed');
        }
        if (!$response->end()) throw new RuntimeException('end failed');
    } elseif ($path === '/file') {
        if (!$response->sendfile($file)) throw new RuntimeException('sendfile failed');
    } else {
        if (!$response->end($payload)) throw new RuntimeException('end failed');
    }
    $events[$id]['queued'] = $server->getClientInfo($request->fd)['send_queued_bytes'];
    $events[$id]['finished'] = true;
});
$server->start();
