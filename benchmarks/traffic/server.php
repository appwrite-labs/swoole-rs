<?php
// Larger and concurrent traffic per request (#2652): HTTP/2 streams carrying bodies, 64 KiB and
// fragmented WebSocket messages, a multipart upload and a file from the static handler. Its own
// server, so the static handler's per-request file check stays out of every other workload.
$server = new Swoole\WebSocket\Server('127.0.0.1', (int) getenv('TEST_PORT'), SWOOLE_BASE);
$server->set([
    'worker_num' => 1,
    'open_http2_protocol' => true,
    'enable_static_handler' => true,
    'document_root' => __DIR__,
    'static_handler_locations' => ['/static.txt'],
    'upload_tmp_dir' => getenv('TEST_DATA'),
    'package_max_length' => 4 * 1024 * 1024,
]);
$server->on('request', static function ($request, $response): void {
    if ($request->server['request_uri'] === '/upload') {
        $file = $request->files['file'] ?? null;
        $response->end($file && $file['error'] === UPLOAD_ERR_OK ? $file['size'] . "\n" : "no file\n");
        return;
    }
    $response->end($request->getContent());  // /echo
});
$server->on('message', static function ($server, $frame): void {
    $server->push($frame->fd, $frame->data);
});
$server->start();
