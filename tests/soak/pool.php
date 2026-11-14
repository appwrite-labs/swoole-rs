<?php
// A process pool whose workers each run a little coroutine work and exit, so the
// master respawns them for the whole run.
$pool = new Swoole\Process\Pool(2, SWOOLE_IPC_NONE, 0, true);
$pool->on('workerStart', static function (Swoole\Process\Pool $pool, int $id): void {
    $channel = new Swoole\Coroutine\Channel(1);
    go(static fn () => $channel->push(str_repeat('x', 1024)));
    Swoole\Coroutine::sleep(0.2);
    $channel->pop(1.0);
});
$pool->start();
