<?php
// Isolated ordinary Process callbacks exercise the real native shared helper.
// pipe_type0: no IPC socket, file, listener, fixed fixture or network.
function check(bool $ok, string $message): void {
    if (!$ok) throw new RuntimeException($message);
}
check(class_exists(Swoole\Process::class), 'UNSUPPORTED: Process required');
$coroutine = $argv[1] === 'coroutine';
$exception = $argv[2] === 'exception';
$process = new Swoole\Process(function () use ($exception): void {
    echo "M26 callback enter\n";
    $nested = swoole_typed_array('<int>', [3, 5]);
    check($nested->sum() === 8, 'child callback reenters helpers');
    if ($exception) throw new RuntimeException('M26 ordinary child exception');
    echo "M26 callback return\n";
}, false, 0, $coroutine);
$pid = $process->start();
check(is_int($pid) && $pid > 0, 'own child started');
$status = Swoole\Process::wait(true);
check(is_array($status) && $status['pid'] === $pid, 'own child reaped');
check($status['code'] === ($exception ? 255 : 0) && $status['signal'] === 0, 'exact native child exit status');
echo json_encode(['coroutine' => $coroutine, 'exception' => $exception,
    'code' => $status['code'], 'signal' => $status['signal']], JSON_THROW_ON_ERROR), "\n";
