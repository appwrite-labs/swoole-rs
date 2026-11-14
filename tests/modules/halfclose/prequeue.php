<?php
$payload = str_repeat(implode('', array_map(chr(...), range(0, 255))), 16384);
$file = getenv('TEST_DATA') . '/payload';
file_put_contents($file, $payload);
$events = [];
$owners = [];
$persist = static function () use (&$events): void {
    $path = getenv('TEST_DATA') . '/events-' . getmypid() . '.json';
    file_put_contents($path . '.tmp', json_encode($events));
    rename($path . '.tmp', $path);
};
$coroutines = getenv('TEST_COROUTINES') === '1';
$server = new Swoole\Http\Server('127.0.0.1', (int) getenv('TEST_PORT'), SWOOLE_PROCESS);
$server->set([
    'worker_num' => (int) (getenv('TEST_WORKERS') ?: 1), 'reactor_num' => 1, 'enable_coroutine' => $coroutines,
    'max_queued_bytes' => (int) getenv('TEST_MAX_QUEUED_BYTES'),
    'dispatch_mode' => getenv('TEST_UNORDERED') === '1' ? 1 : 2,
    'enable_unsafe_event' => getenv('TEST_UNORDERED') === '1',
    'http_compression' => false, 'kernel_socket_send_buffer_size' => 8192,
    'socket_buffer_size' => 16 * 1024 * 1024, 'output_buffer_size' => 16 * 1024 * 1024,
    'max_wait_time' => 1,
]);
$server->on('close', static function ($server, $fd) use (&$events, &$owners, $persist): void {
    if (isset($owners[$fd])) ++$events[$owners[$fd]]['closes'];
    $persist();
});
$server->on('request', static function ($request, $response) use ($server, $payload, $file, &$events, &$owners, $coroutines, $persist): void {
    unset($owners[$request->fd]);
    $path = $request->server['request_uri'];
    if ($path === '/stats') {
        $snapshot = [];
        foreach (glob(getenv('TEST_DATA') . '/events-*.json') as $entry) {
            $snapshot += json_decode(file_get_contents($entry), true, 512, JSON_THROW_ON_ERROR);
        }
        $response->end(json_encode((object) $snapshot));
        return;
    }
    $id = $request->get['id'];
    $owners[$request->fd] = $id;
    $events[$id] ??= ['requests' => 0, 'closes' => 0];
    ++$events[$id]['requests'];
    file_put_contents(getenv('TEST_DATA') . '/' . $id, 'entered');
    $delay = (int) ($request->get['delay'] ?? 0);
    if ($delay > 0) {
        if ($coroutines) Swoole\Coroutine::sleep($delay / 1000000);
        else usleep($delay);
    }
    $accepted = true;
    if ($path === '/force') {
        $events[$id]['force_closed'] = $server->close($request->fd, true);
        $accepted = $response->end($payload);
    } elseif ($path === '/stream') {
        for ($offset = 0; $offset < strlen($payload); $offset += 65536) {
            $accepted = $response->write(substr($payload, $offset, 65536)) && $accepted;
        }
        $accepted = $response->end() && $accepted;
    } elseif ($path === '/file') {
        $accepted = $response->sendfile($file);
    } else {
        $accepted = $response->end($payload);
    }
    $events[$id]['accepted'] = $accepted;
    $events[$id]['finished'] = true;
    $persist();
});
$server->start();
