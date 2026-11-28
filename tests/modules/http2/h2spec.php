<?php
// Protocol testee only: h2spec owns all assertions. TLS when TEST_TLS names a cert and key.
$server = new Swoole\Http\Server('127.0.0.1', (int) getenv('TEST_PORT'), SWOOLE_BASE,
    getenv('TEST_TLS') ? SWOOLE_SOCK_TCP | SWOOLE_SSL : SWOOLE_SOCK_TCP);
// The default limit is UINT_MAX, and 5.1.2 opens one stream more than the limit.
$settings = ['worker_num' => 1, 'open_http2_protocol' => true, 'http2_max_concurrent_streams' => 100,
    'log_file' => '/dev/null'];
if (getenv('TEST_TLS')) {
    $settings += ['ssl_cert_file' => getenv('TEST_TLS') . '.crt', 'ssl_key_file' => getenv('TEST_TLS') . '.key'];
}
$server->set($settings);
// A body, so the flow-control cases have DATA to hold back.
$server->on('request', static fn ($request, $response) => $response->end(str_repeat('h2spec', 200)));
$server->start();
