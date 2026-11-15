<?php
error_reporting(0);
register_shutdown_function(fn() => print("SHUTDOWN\n"));
$close = function () use ($argv) {
    $channel = new Swoole\Coroutine\Channel(1);
    go(function () use ($channel, $argv) {
        if ($channel->pop() !== false || $channel->errCode !== SWOOLE_CHANNEL_CLOSED) {
            throw new LogicException('the waiter did not observe channel close');
        }
        echo "RESUMED\n";
        if ($argv[1] === 'fatal') {
            trigger_error('resumed close waiter', E_USER_ERROR);
        }
    });
    echo "CLOSE\n";
    var_dump($channel->close());
    echo "AFTER\n";
};
if ($argv[2] === 'coroutine') {
    Swoole\Coroutine\run($close);
} else {
    $close();
}
