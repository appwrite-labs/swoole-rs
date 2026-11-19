<?php
$master = getmypid();
$handled = false;
$server = new Swoole\Server('127.0.0.1', 0, SWOOLE_PROCESS, SWOOLE_SOCK_UDP);
// A native callback needs no PHP coroutine or bundled library.
$server->set(['enable_coroutine' => false, 'worker_num' => 1, 'reactor_num' => 1, 'log_file' => '/dev/null']);
$server->on('start', static function ($server) {
    $peer = stream_socket_client('udp://127.0.0.1:' . $server->port);
    fwrite($peer, 'worker check');
    fclose($peer);
});
$server->on('packet', static function ($server, $data) use (&$handled) {
    $handled = $data === 'worker check';
    $server->shutdown();
});
$server->on('workerStop', static function () use ($master, &$handled) {
    if ($handled && getmypid() !== $master) {
        $lsan = FFI::cdef('int __lsan_do_recoverable_leak_check(void);');
        if ($lsan->__lsan_do_recoverable_leak_check() === 0) {
            echo "worker leak check passed\n";
        }
    }
});
$server->start();
