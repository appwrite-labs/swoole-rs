<?php
// A thread's script ends; during its request shutdown a __destruct leaves the thread
// while an object of the mode's class is still live, so Zend frees it later in that
// shutdown (with no zend_try around the object store). The main thread prints the
// thread's exit status.
use Swoole\Thread;
class Leave {
    public function __destruct() { swoole_implicit_fn('bailout', 235); }
}
function make(string $mode) {
    return match ($mode) {
        'none', 'arguments' => null,
        'cookie' => new Swoole\Http\Cookie(),
        'client' => new Swoole\Client(SWOOLE_SOCK_TCP),
        'http_client' => new Swoole\Coroutine\Http\Client('127.0.0.1', 80),
        'socket' => new Swoole\Coroutine\Socket(AF_INET, SOCK_STREAM, 0),
        'scheduler' => new Swoole\Coroutine\Scheduler(),
    };
}
if (Thread::getInfo()['is_main_thread']) {
    $mode = $argv[1];
    // The thread reads its mode from $argv; only `arguments` gives it arguments of its
    // own, which its RSHUTDOWN releases.
    $worker = $mode === 'arguments' ? new Thread(__FILE__, $mode) : new Thread(__FILE__);
    $worker->join();
    echo json_encode([$mode, $worker->getExitStatus()]), "\n";
    exit;
}
$object = make($argv[1]);
$keep = $object;
$leave = new Leave();
