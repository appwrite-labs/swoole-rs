<?php
[$script, $kind, $co, $mode] = $argv;
swoole_async_set(['enable_coroutine' => (bool)(int)$co]);
class TerminalGuard {
    public function __construct(private string $mode) {}
    public function __destruct() {
        echo "DTOR\n";
        if ($this->mode === 'exit') exit(31);
        if ($this->mode === 'oom') $blob = str_repeat('x', 32 * 1024 * 1024);
        throw new RuntimeException('destructor failure');
    }
}
register_shutdown_function(static function() { echo "SHUTDOWN\n"; });
ini_set('memory_limit', (string)(memory_get_usage(true) + 1024 * 1024));
$g = new TerminalGuard($mode);
$cb = static function() use ($g) { echo "CALL\n"; };
$id = $kind === 'event-exit-after' ? Swoole\Timer::after(60000, $cb) : Swoole\Timer::tick(60000, $cb);
unset($g, $cb);
if (str_starts_with($kind, 'event-exit-')) {
    Swoole\Timer::after(1, static function() { Swoole\Event::exit(); });
    Swoole\Event::wait();
} elseif ($kind === 'clear') Swoole\Timer::clear($id);
else Swoole\Timer::clearAll();
echo "RETURN\n";
