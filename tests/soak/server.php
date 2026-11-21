<?php
// The soak target: HTTP/1, HTTP/2 and WebSocket on one port, task workers, and
// coroutine redis and mysql clients when SOAK_REDIS / SOAK_MYSQL name a host.
// Each worker publishes memory_get_usage() to a shared table every second.
// SOAK_LEAK=1 keeps about 60 bytes per request alive: soak.py's self-test.
$server = new Swoole\WebSocket\Server('127.0.0.1', (int) getenv('TEST_PORT'), SWOOLE_PROCESS);
$server->set(['worker_num' => 2, 'task_worker_num' => 2, 'open_http2_protocol' => true,
    'hook_flags' => SWOOLE_HOOK_TCP, 'log_level' => SWOOLE_LOG_WARNING]);
$memory = new Swoole\Table(64);
$memory->column('pid', Swoole\Table::TYPE_INT);
$memory->column('task', Swoole\Table::TYPE_INT);
$memory->column('bytes', Swoole\Table::TYPE_INT);
$memory->create();
$workerErrors = new Swoole\Atomic();
$redis = getenv('SOAK_REDIS');
$mysql = getenv('SOAK_MYSQL');
$leak = getenv('SOAK_LEAK') === '1';
if ($leak) {
    ini_set('memory_limit', '-1');
}

$server->on('workerStart', static function ($server, int $id) use ($memory): void {
    $publish = static fn () => $memory->set("w$id",
        ['pid' => getmypid(), 'task' => (int) $server->taskworker, 'bytes' => memory_get_usage()]);
    $publish();
    Swoole\Timer::tick(1000, $publish);
});
$server->on('workerError', static fn () => $workerErrors->add());
$server->on('task', static fn ($server, $task, $worker, $data) => array_sum($data));
$server->on('finish', static function (): void {});
$server->on('message', static fn ($server, $frame) => $server->push($frame->fd, $frame->data));
$server->on('request', static function ($request, $response) use ($server, $memory, $workerErrors, $redis, $mysql, $leak): void {
    static $kept = [];
    if ($leak) {
        $kept[] = str_repeat('x', 16);
    }
    switch ($request->server['request_uri']) {
        case '/task':
            $body = (string) $server->taskwait([1, 2, 3], 5.0);
            break;
        case '/redis':
            $client = new Swoole\Coroutine\Client(SWOOLE_SOCK_TCP);
            $client->connect($redis, 6379, 5.0);
            $client->send("INCR soak\r\n");
            $body = (string) $client->recv(5.0);
            $client->close();
            break;
        case '/mysql':
            $pdo = new PDO("mysql:host=$mysql;dbname=test", 'root', 'root');
            $body = (string) $pdo->query('SELECT 1 + 1')->fetchColumn();
            break;
        case '/reload':
            // Task workers only: event workers must live the whole run, or a
            // per-request leak in them would be reset before it shows.
            $body = $server->reload(true) ? 'reloading' : 'busy';
            break;
        case '/stats':
            $body = json_encode(['worker_errors' => $workerErrors->get(),
                'memory' => iterator_to_array($memory), 'stats' => $server->stats()]);
            break;
        default:
            $body = 'ok';
    }
    $response->end($body);
});
$server->start();
