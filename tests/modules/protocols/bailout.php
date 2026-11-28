<?php
declare(strict_types=1);
// Keep all error and allocator operations native, after Rust events return.
ini_set('display_errors', '0');
ini_set('log_errors', '1');
ini_set('error_log', getenv('TEST_LOG_FILE'));
$scenario = getenv('TEST_SCENARIO');
$process = getenv('TEST_SERVER_MODE') === 'process';
$server = new Swoole\Redis\Server('127.0.0.1', (int)getenv('TEST_PORT'),
    $process ? SWOOLE_PROCESS : SWOOLE_BASE);
$server->set([
    'worker_num' => 1,
    'log_file' => getenv('TEST_LOG_FILE'),
    'package_max_length' => 4 * 1024 * 1024,
    'enable_coroutine' => getenv('TEST_COROUTINES') === '1',
]);
$note = static function (array $value): void {
    file_put_contents(getenv('TEST_EVENTS'), json_encode($value, JSON_THROW_ON_ERROR)."\n", FILE_APPEND);
};
$server->on('workerStart', static function () use ($note): void {
    $note(['start', memory_get_usage(true), ini_get('memory_limit')]);
});
$server->on('workerError', static function ($server, int $id, int $pid, int $code, int $signal) use ($note): void {
    $note(['worker-error', $code, $signal]);
});
$server->setHandler('ECHO', static function (int $fd, array $params) use ($server): void {
    $server->send($fd, "+OK\r\n");
});
$server->setHandler('THROW', static function () use ($note): void {
    $note(['entered', 'throw']);
    throw new RuntimeException('request-boundary-exception');
});
$server->setHandler('EXIT', static function () use ($note): void {
    $note(['entered', 'exit']);
    exit(23);
});
$server->start();
