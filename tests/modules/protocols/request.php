<?php
declare(strict_types=1);
use Swoole\Redis\Server;

$process = getenv('TEST_SERVER_MODE') === 'process';
$server = new Server('127.0.0.1', (int)getenv('TEST_PORT'), $process ? SWOOLE_PROCESS : SWOOLE_BASE);
$server->set([
    'worker_num' => $process ? 2 : 1,
    'log_file' => '/dev/null',
    'package_max_length' => 4 * 1024 * 1024,
    'buffer_output_size' => 8 * 1024 * 1024,
    'enable_coroutine' => getenv('TEST_COROUTINES') !== '0',
]);
foreach (['ECHO', 'MiXeD', str_repeat('l', 63)] as $command) {
    $server->setHandler($command, static function (int $fd, array $params) use ($server, $command): void {
        if (Swoole\Coroutine::getCid() >= 0) {
            Swoole\Coroutine::sleep(0.0001);
        }
        $typed = array_map(static function ($value): array {
            if (is_string($value)) {
                return ['string', base64_encode($value)];
            }
            return [get_debug_type($value), $value];
        }, $params);
        $server->send($fd, Server::format(Server::STRING, json_encode([$command, $typed], JSON_THROW_ON_ERROR)));
    });
}
$server->start();
