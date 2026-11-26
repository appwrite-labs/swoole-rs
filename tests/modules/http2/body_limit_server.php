<?php
// test_server.py's h2c server, whose request bodies may not exceed 64 KiB and
// whose sends wait half a second for the peer.
$server = new Swoole\Http\Server('127.0.0.1', (int) getenv('TEST_PORT'), SWOOLE_BASE);
$server->set(['worker_num' => 1, 'open_http2_protocol' => true, 'package_max_length' => 65536,
    'send_timeout' => 0.5, 'log_file' => '/dev/null']);
$server->on('request', static fn ($request, $response) => $response->end('ok'));
$server->start();
