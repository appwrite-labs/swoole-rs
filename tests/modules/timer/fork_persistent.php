<?php
declare(strict_types=1);
// Distinct from the unchanged one-shot witness: fork inside a repeating callback.
if (function_exists('posix_setrlimit')) {
    posix_setrlimit(POSIX_RLIMIT_CORE, 0, 0);
}
swoole_async_set(['enable_coroutine' => false]);
$mode = $argv[1] ?? 'persistent';
if (!in_array($mode, ['persistent', 'child-reinit'], true)) {
    throw new RuntimeException('Unknown diagnostic mode');
}
$callbacks = 0;
$health = 0;
$wait = null;
$activeId = null;
$sentinelId = Swoole\Timer::after(60000, function (): void {
    throw new RuntimeException('Inherited sentinel unexpectedly fired');
});
$activeId = Swoole\Timer::tick(1, function (int $id) use (
    &$callbacks, &$health, &$wait, $sentinelId, $mode
): void {
    ++$callbacks;
    if ($callbacks === 1) {
        $process = new Swoole\Process(function (Swoole\Process $child) use ($id, $sentinelId, $mode): void {
            if ($mode === 'persistent') {
                file_put_contents(getenv('TEST_DATA') . '/child.done', 'DONE');
            } else {
                $inheritedMissing = !Swoole\Timer::exists($id) && !Swoole\Timer::exists($sentinelId);
                $initialCount = Swoole\Timer::stats()['num'];
                $cancelledCalls = 0;
                $cancelledId = Swoole\Timer::after(60000, function () use (&$cancelledCalls): void {
                    ++$cancelledCalls;
                });
                $clear = Swoole\Timer::clear($cancelledId);
                $clearedMissing = !Swoole\Timer::exists($cancelledId);
                $childCallbacks = 0;
                $childHealth = 0;
                Swoole\Timer::tick(1, function (int $childId) use (&$childCallbacks): void {
                    if (++$childCallbacks === 2 && !Swoole\Timer::clear($childId)) {
                        throw new RuntimeException('Child active repeating clear failed');
                    }
                });
                Swoole\Timer::after(1, function () use (&$childHealth): void { ++$childHealth; });
                Swoole\Event::wait();
                file_put_contents(getenv('TEST_DATA') . '/child.done', json_encode([
                    'inherited_missing' => $inheritedMissing, 'initial_count' => $initialCount,
                    'clear' => $clear, 'cleared_missing' => $clearedMissing,
                    'cancelled_calls' => $cancelledCalls, 'callbacks' => $childCallbacks,
                    'health' => $childHealth, 'timers' => Swoole\Timer::stats()['num'],
                ], JSON_THROW_ON_ERROR));
            }
            $child->exit(0);
        }, false, 0, false);
        $pid = $process->start();
        if ($pid === false) {
            throw new RuntimeException('Child start failed');
        }
        $wait = Swoole\Process::wait(true);
        if (!is_array($wait) || $wait['pid'] !== $pid) {
            throw new RuntimeException('Wait returned wrong child');
        }
        Swoole\Timer::after(1, function () use (&$health): void { ++$health; });
    }
    if ($callbacks === 3) {
        if (!Swoole\Timer::clear($id) || !Swoole\Timer::clear($sentinelId)) {
            throw new RuntimeException('Parent active/sentinel cleanup failed');
        }
    }
});
Swoole\Event::wait();
$childData = is_file(getenv('TEST_DATA') . '/child.done')
    ? file_get_contents(getenv('TEST_DATA') . '/child.done') : null;
if ($mode === 'child-reinit' && $childData !== null) {
    $childData = json_decode($childData, true, flags: JSON_THROW_ON_ERROR);
}
echo json_encode(['callbacks' => $callbacks, 'health' => $health,
    'child_exit' => $wait['code'] ?? null, 'child_signal' => $wait['signal'] ?? null,
    'child_done' => $childData, 'timers' => Swoole\Timer::stats()['num']], JSON_THROW_ON_ERROR), "\n";
