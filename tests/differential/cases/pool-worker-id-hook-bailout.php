<?php
// A coroutine Process\Pool subclass hooks workerId (PHP 8.4), so the worker's write of
// its id runs the hook, and the hook ends the worker: upstream stops there, so the
// onWorkerStart coroutine never starts. The restarted worker stops the pool. On PHP
// 8.3 the hook is a parse error on every engine.
class Pool extends Swoole\Process\Pool
{
    public $workerId {
        set($value) {
            if (!file_exists($GLOBALS['marker'])) {
                touch($GLOBALS['marker']);
                echo "  set workerId: stop\n";
                trigger_error('stop', E_USER_ERROR);
            }
            echo "  set workerId: shutdown\n";
            unlink($GLOBALS['marker']);
            Swoole\Process::kill($GLOBALS['master'], SIGTERM);
            exit(0);
        }
    }
}
set_error_handler(null);
$master = getmypid();
$marker = sys_get_temp_dir() . "/pool-worker-id-hook-$master";
$pool = new Pool(1, SWOOLE_IPC_NONE, 0, true);
// Only while the marker exists: the first worker must never start, and the restarted
// one's start races the pool's shutdown, which its hook has already begun (#3597).
$pool->on('workerStart', static function (Pool $pool) {
    if (file_exists($GLOBALS['marker'])) {
        echo "  workerStart\n";
    }
});
s('start', fn() => $pool->start());
echo "after\n";
