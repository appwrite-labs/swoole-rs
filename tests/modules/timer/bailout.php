<?php
// Real Zend callback termination; no stand-in longjmp or modified original PHPT.
[$script, $kind, $mode, $coroutine, $clear] = $argv;
swoole_async_set(['enable_coroutine' => (bool)(int)$coroutine]);
$id = 0;
register_shutdown_function(static function () use (&$id) {
    echo 'SHUTDOWN ', error_get_last()['type'] ?? 0, "\n";
    $info = Swoole\Timer::info($id);
    if (is_array($info)) unset($info['exec_msec']); // Native clock offsets vary by process.
    echo 'STATE ', json_encode([Swoole\Timer::stats(), $info]), "\n";
});
if ($mode === 'oom') {
    if (ini_set('memory_limit', (string)(memory_get_usage(true) + 1024 * 1024)) === false) {
        throw new RuntimeException('Cannot set test memory limit');
    }
}
$callback = static function () use (&$id, $mode, $clear) {
    echo "CALL\n";
    if ((int)$clear) {
        if (!Swoole\Timer::clear($id)) throw new RuntimeException('Self-clear failed');
    }
    if ($mode === 'exit') exit(23);
    if ($mode === 'throw') throw new RuntimeException('timer callback failure');
    if ($mode === 'oom') $blob = str_repeat('x', 32 * 1024 * 1024);
};
$id = $kind === 'after' ? Swoole\Timer::after(1, $callback) : Swoole\Timer::tick(1, $callback);
Swoole\Event::wait();
echo "RETURN\n";
