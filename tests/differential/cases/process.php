<?php
$pool = s('pool.new', fn() => new Swoole\Process\Pool(1));
s('pool.stop', fn() => $pool->stop());
s('pool.getProcess', fn() => $pool->getProcess());
$process = new Swoole\Process(fn() => null);
s('process.exec.missing', fn() => $process->exec('/swr/no/such/program', ['a']));
s('process.exec.relative', fn() => $process->exec('echo', ['a']));
// Last: a successful exec replaces this PHP process with echo.
s('process.exec', fn() => $process->exec('/bin/echo', ['exec', 'ran']));
