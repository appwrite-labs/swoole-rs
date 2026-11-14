<?php
// One worker owns the observable request/close ledger; this is not a native leak detector.
$block = str_repeat(implode('', array_map(chr(...), range(0, 255))), 256);
$payload = str_repeat($block, 64);
$file = getenv('TEST_DATA') . '/payload.bin';
file_put_contents($file, $payload);
$events = [];
$owners = [];
$server = new Swoole\Http\Server('127.0.0.1', (int) getenv('TEST_PORT'), SWOOLE_BASE);
$server->set([
    'worker_num' => 1, 'enable_coroutine' => false, 'http_compression' => false,
    'socket_buffer_size' => 16 * 1024 * 1024, 'output_buffer_size' => 16 * 1024 * 1024,
    'kernel_socket_send_buffer_size' => 8192, 'max_wait_time' => 1,
]);
$persist = static function (array $event): void {
    file_put_contents(getenv('TEST_DATA') . '/events.jsonl', json_encode($event) . "\n", FILE_APPEND | LOCK_EX);
};
$server->on('close', static function ($server, $fd) use (&$events, &$owners, $persist): void {
    if (isset($owners[$fd])) {
        $id = $owners[$fd];
        ++$events[$id]['closes'];
        $persist(['event' => 'close', 'id' => $id]);
        // Retain the owner until fd reuse so duplicate close callbacks remain observable.
    }
});
$server->on('workerStop', static function ($server, $workerId) use ($persist): void {
    $persist(['event' => 'worker-stop', 'worker_id' => $workerId]);
});
$server->on('request', static function ($request, $response) use ($server, $payload, $file, &$events, &$owners): void {
    $path = $request->server['request_uri'];
    $id = $request->get['id'] ?? null;
    // Untagged health requests also replace any prior owner when a descriptor is reused.
    unset($owners[$request->fd]);
    if ($id !== null) {
        $owners[$request->fd] = $id;
        $events[$id] ??= ['requests' => 0, 'closes' => 0];
        ++$events[$id]['requests'];
        $events[$id]['session_id'] = (int) $request->fd;
    }
    if ($path === '/stats') {
        $fds = glob((is_dir('/proc/self/fd') ? '/proc/self/fd' : '/dev/fd') . '/*');
        $response->end(json_encode(['events' => $events, 'php_heap_bytes' => memory_get_usage(true),
                                   'process_fds' => $fds === false ? null : count($fds)]));
    } elseif ($path === '/stream' || $path === '/shutdown') {
        for ($i = 0; $i < 64; ++$i) {
            if (!$response->write(substr($payload, $i * 65536, 65536))) throw new RuntimeException('write failed');
        }
        $info = $server->getClientInfo($request->fd);
        $events[$id]['queued_bytes'] = $info['send_queued_bytes'];
        if (!$response->end()) throw new RuntimeException('end failed');
        if ($path === '/shutdown') Swoole\Timer::after(100, static fn() => $server->shutdown());
    } elseif ($path === '/file') {
        if (!$response->sendfile($file, 13, strlen($payload) - 27)) throw new RuntimeException('sendfile failed');
        $events[$id]['sendfile_accepted'] = true;
        $info = $server->getClientInfo($request->fd);
        $events[$id]['queued_bytes'] = $info['send_queued_bytes'];
    } else {
        $response->end('ok');
    }
});
$server->start();
