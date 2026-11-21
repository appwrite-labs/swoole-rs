<?php
// A Coroutine\Scheduler is freed with a queued task whose closure's destructor ends the
// request: upstream stops there, so the code after the free never runs.
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
$scheduler = new Swoole\Coroutine\Scheduler();
$stop = new Stop();
s('add', fn() => $scheduler->add(function () use ($stop) {}));
$stop = null;
s('free', function () use (&$scheduler) {
    $scheduler = null;
    return 'freed';
});
echo "after\n";
