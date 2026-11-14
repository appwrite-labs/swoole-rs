<?php
declare(strict_types=1);
// Diagnostic: fork while a one-shot TimerNode is actively dispatching.
if (function_exists('posix_setrlimit')) {
    posix_setrlimit(POSIX_RLIMIT_CORE, 0, 0);
}
swoole_async_set(['enable_coroutine' => false]);
$callbacks = 0;
$health = 0;
$wait = null;
Swoole\Timer::after(1, function () use (&$callbacks, &$health, &$wait): void {
    ++$callbacks;
    $process = new Swoole\Process(function (Swoole\Process $child): void {
        file_put_contents(getenv('TEST_DATA') . '/child.done', 'DONE');
        $child->exit(0);
    }, false, 0, false);
    $pid = $process->start();
    if ($pid === false) {
        throw new RuntimeException('child start failed');
    }
    $wait = Swoole\Process::wait(true);
    if (!is_array($wait) || $wait['pid'] !== $pid) {
        throw new RuntimeException('wait returned wrong child');
    }
    Swoole\Timer::after(1, function () use (&$health): void { ++$health; });
});
Swoole\Event::wait();
echo json_encode(['callbacks' => $callbacks, 'health' => $health,
    'child_exit' => $wait['code'] ?? null, 'child_signal' => $wait['signal'] ?? null,
    'child_done' => is_file(getenv('TEST_DATA') . '/child.done')
        ? file_get_contents(getenv('TEST_DATA') . '/child.done') : null,
    'timers' => Swoole\Timer::stats()['num']], JSON_THROW_ON_ERROR), "\n";
