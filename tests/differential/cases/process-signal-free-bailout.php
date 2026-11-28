<?php
// Process::signal replaces a callable whose destructor ends the request once the event
// loop frees it: upstream stops there, so the coroutine never wakes from its sleep.
set_error_handler(null);
class Stop
{
    public function __destruct()
    {
        echo "  destruct\n";
        trigger_error('stop', E_USER_ERROR);
    }
}
register_shutdown_function(static fn() => print("shutdown\n"));
Swoole\Coroutine\run(static function () {
    $stop = new Stop();
    s('signal', fn() => Swoole\Process::signal(SIGUSR1, function () use ($stop) {}));
    $stop = null;
    s('signal again', fn() => Swoole\Process::signal(SIGUSR1, fn() => null));
    Swoole\Coroutine::sleep(0.01);
    echo "after sleep\n";
});
