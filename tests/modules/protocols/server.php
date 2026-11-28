<?php
declare(strict_types=1);
$process = getenv('TEST_SERVER_MODE') === 'process';
$server = new Swoole\Server('127.0.0.1', (int)getenv('TEST_PORT'), $process ? SWOOLE_PROCESS : SWOOLE_BASE);
$server->set([
    'worker_num' => $process ? 2 : 1,
    'log_file' => getenv('TEST_LOG_FILE') ?: '/dev/null',
    'package_max_length' => 4 * 1024 * 1024,
    getenv('TEST_PROTOCOL') === 'mqtt' ? 'open_mqtt_protocol' : 'open_redis_protocol' => true,
]);
$server->on('receive', static function ($server, int $fd, int $reactor, string $data): void {
    $server->send($fd, pack('N', strlen($data)) . hash('sha256', $data, true));
    if (getenv('TEST_SHUTDOWN_SHA256') === hash('sha256', $data)) {
        $server->shutdown();
    }
});
if (getenv('TEST_WORKER_ERRORS')) {
    $server->on('workerError', static function ($server, int $workerId, int $workerPid, int $exitCode, int $signal): void {
        file_put_contents(getenv('TEST_WORKER_ERRORS'), "$workerId:$exitCode:$signal\n", FILE_APPEND);
    });
}
$server->start();
