<?php
// Reject a task after callback registration, then let its destructor submit a fresh task.
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
    $callback = new class($server) {
        public bool $active = true;
        public function __construct(private Swoole\Server $server) {}
        public function __invoke() {}
        public function __destruct() {
            if ($this->active) {
                echo is_int($this->server->task('nested')) ? "nested accepted\n" : "nested rejected\n";
            }
        }
    };
    $weak = WeakReference::create($callback);
    try {
        if (!rename($socket, $socket . '.held')) {
            throw new RuntimeException('could not suspend task dispatch');
        }
        try {
            $result = @$server->task('reject', -1, $callback);
        } finally {
            rename($socket . '.held', $socket);
        }
        unset($callback);
        echo $result === false && $weak->get() === null ? "callback released\n" : "callback retained or task accepted\n";
    } finally {
        if ($weak->get() !== null) {
            $weak->get()->active = false;
        }
        $server->shutdown();
    }
});
$server->start();
