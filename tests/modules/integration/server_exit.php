<?php
require __DIR__ . '/../../include/bootstrap.php';
use Swoole\Thread;
use Swoole\Http\Server;
const CODE = 235;
$port = get_constant_port(__FILE__);
$master = getmypid();
$driver = Thread::getInfo()['is_main_thread'] ? pcntl_fork() : -1;
if (Thread::getInfo()['is_main_thread']) Assert::greaterThanEq($driver, 0);
if ($driver === 0) {
    $deadline = microtime(true) + 10;
    do {
        $state = @file_get_contents("http://127.0.0.1:$port/ready");
        if ($state === '2') break;
        usleep(1000);
    } while (microtime(true) < $deadline);
    Assert::eq($state, '2');
    echo "begin\n";
    $rs = @file_get_contents("http://127.0.0.1:$port/exit");
    Assert::false($rs);
    do {
        $state = @file_get_contents("http://127.0.0.1:$port/ready");
        if ($state === '3') break;
        usleep(1000);
    } while (microtime(true) < $deadline);
    Assert::eq($state, '3');
    usleep(200_000);
    echo "done\n";
    Assert::true(posix_kill($master, SIGTERM));
    exit(0);
}
$serv = new Server('127.0.0.1', $port, SWOOLE_THREAD);
$serv->set([
    'worker_num' => 2,
    'log_file' => '/dev/null',
    'log_level' => SWOOLE_LOG_ERROR,
    'enable_coroutine' => false,
    'init_arguments' => function () {
        global $atomic;
        $atomic = new Thread\Atomic(0);
        return [$atomic];
    },
]);
$serv->on('WorkerStart', function (Server $serv, $workerId) {
    [$atomic] = Thread::getArguments();
    echo "worker start\n";
    $atomic->add();
});
$serv->on('workerError', function (Server $serv, $workerId, $workerPid, $status, $signal) {
    Assert::eq($status, CODE);
    echo "worker error\n";
});
$serv->on('Request', function ($req, $resp) {
    if ($req->server['request_uri'] === '/exit') {
        swoole_implicit_fn('bailout', CODE);
    }
    [$atomic] = Thread::getArguments();
    $resp->end((string) $atomic->get());
});
$serv->on('shutdown', function () {
    global $atomic;
    echo "shutdown\n";
    Assert::eq($atomic->get(), 3);
});
$serv->start();
if (Thread::getInfo()['is_main_thread']) {
    pcntl_waitpid($driver, $status);
    Assert::true(pcntl_wifexited($status));
    Assert::eq(pcntl_wexitstatus($status), 0);
}
