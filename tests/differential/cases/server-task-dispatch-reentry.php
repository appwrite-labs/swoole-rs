<?php
// Reject a dispatch after registration. Give the callback no caller-owned reference
// so its destructor can submit a replacement as soon as the call releases it.
$server = new Swoole\Server('127.0.0.1', 0, SWOOLE_PROCESS);
$server->set([
    'worker_num' => 1, 'task_worker_num' => 1, 'task_ipc_mode' => 4,
    'log_file' => '/dev/null',
]);
$socket = '/tmp/swoole.task.' . getmypid() . '.sock';
$server->on('receive', static function () {});
$server->on('task', static function () {});
$server->on('workerStart', static function ($server, $id) use ($socket) {
    if ($id !== 0) {
        return;
    }
    $state = (object) ['active' => true, 'nested' => false];
    if (!rename($socket, $socket . '.held')) {
        throw new RuntimeException('could not suspend task dispatch');
    }
    try {
        $result = @$server->task('reject', -1, new class($server, $socket, $state) {
            public function __construct(
                private Swoole\Server $server, private string $socket, private object $state
            ) {}
            public function __invoke() {}
            public function __destruct() {
                if ($this->state->active) {
                    rename($this->socket . '.held', $this->socket);
                    $this->state->nested = is_int($this->server->task('nested'));
                    echo $this->state->nested ? "nested accepted\n" : "nested rejected\n";
                }
            }
        });
        echo $result === false && $state->nested ? "callback released\n" : "callback retained or task accepted\n";
    } finally {
        $state->active = false;
        if (file_exists($socket . '.held')) {
            rename($socket . '.held', $socket);
        }
        $server->shutdown();
    }
});
$server->start();
