<?php
// Process\Pool callbacks call their own pool's methods while the pool runs them:
// set() and getProcess() from onWorkerStart, set() and on() (refused once
// started) from onShutdown, set() from onMessage. The worker stops each pool
// once it has printed.
set_error_handler(null);
$master = getmypid();
// The master acts on a signal when its wait for a worker returns. A SIGTERM sent
// before it waits, or a worker that exits before the wait returns, makes it reap
// that worker before it stops, so a worker signals once the master waits (its
// /proc state is S) and stays until the master stops it.
function stop_master(int $master): void {
    while (is_dir("/proc/$master") && !preg_match('/\) S /', (string) @file_get_contents("/proc/$master/stat"))) {
        usleep(1000);
    }
    Swoole\Process::kill($master, SIGTERM);
}
$pool = new Swoole\Process\Pool(1);
$pool->on('workerStart', static function (Swoole\Process\Pool $pool, int $id) use ($master) {
    s('  set', fn() => $pool->set(['enable_coroutine' => false, 'max_package_size' => 65536]));
    s('  getProcess', fn() => $pool->getProcess() instanceof Swoole\Process);
    s('  workerId', fn() => $pool->workerId);
    stop_master($master);
    sleep(5);
});
$pool->on('shutdown', static function (Swoole\Process\Pool $pool) {
    // A deadline for the workers' exit beyond the program's run.
    s('  set', fn() => $pool->set(['max_wait_time' => 30]));
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
    stop_master($master);
});
s('start', fn() => $pool->start());
echo "after\n";
