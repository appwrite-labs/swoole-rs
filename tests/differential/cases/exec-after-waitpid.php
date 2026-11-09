<?php
// waitPid installs the coroutine SIGCHLD handler. The child exits while the loop is
// blocked, so its pipe's end and its SIGCHLD reach the same loop pass: exec still
// reaps it, and the later waitPid finds no child (seed 0 program 31).
Swoole\Coroutine\run(function () {
s('sys.waitPid', fn() => Swoole\Coroutine\System::waitPid(1, 0.001));
go(function () { Swoole\Coroutine::sleep(0.02); usleep(300000); });
s('sys.exec', fn() => Swoole\Coroutine\System::exec('sleep 0.05; echo hi'));
s('sys.waitPid', fn() => Swoole\Coroutine\System::waitPid(-1, 0.001));
});
