<?php
// Exercise the sole production swoole_crc32 caller through real UDP6 dispatch.
$server = new Swoole\Server('::1', (int) $argv[1], SWOOLE_PROCESS, SWOOLE_SOCK_UDP6);
$server->set(['worker_num' => 5, 'reactor_num' => 1, 'dispatch_mode' => 2, 'log_file' => '/dev/null']);
$server->on('start', function () { echo "READY\n"; flush(); });
$server->on('packet', function ($server, $data, $client) {
    $server->sendto($client['address'], $client['port'], pack('N', $server->worker_id) . $data);
});
$server->start();
