<?php
// The pinned tests/swoole_process/process_exec.phpt with /bin/sh in place of its echo.py,
// which is Python 2: the same child, pipe, line and expected output (#5358).
$process = new Swoole\Process('shell_process', true);
$pid = $process->start();

function shell_process(Swoole\Process $worker)
{
    $worker->exec('/bin/sh', ['-c', 'read line; echo "Python: $line"']);
}

$process->write("Hello World\n");
echo $process->read();
?>
Done
