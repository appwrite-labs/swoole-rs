<?php
// Process\Pool callbacks call their own pool's methods while the pool runs them:
// set() and getProcess() from onWorkerStart, set() and on() (refused once
// started) from onShutdown, set() from onMessage. The worker stops each pool
// once it has printed.
set_error_handler(null);
$master = getmypid();
$pool = new Swoole\Process\Pool(1);
$pool->on('workerStart', static function (Swoole\Process\Pool $pool, int $id) use ($master) {
    s('  set', fn() => $pool->set(['enable_coroutine' => false, 'max_package_size' => 65536]));
    s('  getProcess', fn() => $pool->getProcess() instanceof Swoole\Process);
    s('  workerId', fn() => $pool->workerId);
    Swoole\Process::kill($master, SIGTERM);
});
$pool->on('shutdown', static function (Swoole\Process\Pool $pool) {
    s('  set', fn() => $pool->set(['max_wait_time' => 1]));
    s('  on', fn() => $pool->on('workerStart', fn() => null));
    echo "  shutdown\n";
});
s('start', fn() => $pool->start());
echo "after\n";
// onMessage, from a pool whose onStart sent the message: the handler reconfigures
// the pool before it stops it.
$pool = new Swoole\Process\Pool(1, SWOOLE_IPC_UNIXSOCK);
$pool->on('start', static function (Swoole\Process\Pool $pool) {
    $pool->sendMessage('hello', 0);
});
$pool->on('message', static function (Swoole\Process\Pool $pool, string $data) use ($master) {
    s('  set', fn() => $pool->set(['enable_coroutine' => false, 'max_wait_time' => 1]));
    echo "  message $data\n";
    Swoole\Process::kill($master, SIGTERM);
});
s('start', fn() => $pool->start());
echo "after\n";
