<?php
// waitPid installs the coroutine SIGCHLD handler. The child exits while a busy loop
// blocks the event loop (Coroutine\run hooks usleep, which would yield instead), so its
// pipe's end and its SIGCHLD reach the same loop pass: exec still reaps it, and the
// later waitPid finds no child (seed 0 program 31).
Swoole\Coroutine\run(function () {
s('sys.waitPid', fn() => Swoole\Coroutine\System::waitPid(1, 0.001));
go(function () { Swoole\Coroutine::sleep(0.02); for ($end = microtime(true) + 0.3; microtime(true) < $end;); });
s('sys.exec', fn() => Swoole\Coroutine\System::exec('sleep 0.05; echo hi'));
s('sys.waitPid', fn() => Swoole\Coroutine\System::waitPid(-1, 0.001));
});
