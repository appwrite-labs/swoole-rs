<?php
// Fail during native reply-buffer growth, after payload creation and Rust return.
ini_set('display_errors', 'stderr');
ini_set('log_errors', '0');
$payload = str_repeat('x', 8 * 1024 * 1024);
$limit = memory_get_usage(true) + 4 * 1024 * 1024;
if (ini_set('memory_limit', (string)$limit) === false) {
    throw new RuntimeException('Cannot configure private memory limit');
}
echo json_encode(['checkpoint'=>'payload-ready', 'payload_bytes'=>strlen($payload),
    'payload_sha256'=>hash('sha256', $payload), 'memory_limit'=>$limit]), "\n";
Swoole\Redis\Server::format(Swoole\Redis\Server::STRING, $payload);
throw new RuntimeException('Expected reply allocation to exhaust private limit');
