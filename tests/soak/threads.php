<?php
// soak.py --threads on a thread-safe PHP (#5285): what a thread leaves behind.
// SOAK_THREADS=churn starts and joins $argv[1] threads, each adding and deleting an
// event and registering a signal callback, and prints the process's RSS growth per
// thread. SOAK_THREADS=server is a thread-mode server with a second listener (a port
// callback in each worker thread), whose worker thread soak.py restarts with /reload.
use Swoole\Thread;

function rss(): int
{
    foreach (file('/proc/self/status') as $line) {
        if (str_starts_with($line, 'VmRSS:')) {
            return (int) preg_replace('/\D/', '', $line) * 1024;
        }
    }
    return 0;
}

if (getenv('SOAK_THREADS') === 'server') {
    $port = (int) getenv('TEST_PORT');
    $server = new Swoole\Http\Server('127.0.0.1', $port, SWOOLE_THREAD);
    $server->set(['worker_num' => 1, 'log_level' => SWOOLE_LOG_ERROR, 'enable_coroutine' => false]);
    $extra = $server->addListener('127.0.0.1', (int) getenv('SOAK_EXTRA_PORT'), SWOOLE_SOCK_TCP);
    $extra->set([]);
    $extra->on('receive', static fn ($server, $fd, $reactor, $data) => $server->send($fd, $data));
    $server->on('request', static function ($request, $response) use ($server): void {
        $response->end(match ($request->server['request_uri']) {
            '/reload' => $server->reload() ? 'ok' : 'busy',
            '/rss' => (string) rss(),
            default => 'ok',
        });
    });
    $server->start();
    return;
}

if (!Thread::getInfo()['is_main_thread']) {
    [$a, $b] = stream_socket_pair(STREAM_PF_UNIX, STREAM_SOCK_STREAM, 0);
    Swoole\Event::add($a, static function (): void {});
    Swoole\Event::del($a);
    Swoole\Process::signal(SIGUSR1, static function (): void {});
    return;
}
$threads = (int) ($argv[1] ?? 2000);
for ($i = 0; $i < 50; $i++) {
    (new Thread(__FILE__))->join();
}
$start = rss();
for ($i = 0; $i < $threads; $i++) {
    (new Thread(__FILE__))->join();
}
echo json_encode(['per_thread' => (rss() - $start) / $threads]), "\n";
