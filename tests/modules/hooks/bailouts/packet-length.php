<?php
set_error_handler(null);
Swoole\Coroutine\run(static function () {
    $server = new Swoole\Coroutine\Socket(AF_INET, SOCK_STREAM, 0);
    $server->bind('127.0.0.1', 0);
    $server->listen();
    go(static function () use ($server) {
        $peer = $server->accept();
        $peer->sendAll('abc');
        $peer->close();
        $server->close();
    });
    $client = new Swoole\Coroutine\Socket(AF_INET, SOCK_STREAM, 0);
    $client->connect('127.0.0.1', $server->getsockname()['port']);
    $client->setProtocol(['open_length_check' => true, 'package_length_func' => static function ($bytes) {
        trigger_error('length stop', E_USER_ERROR);
        return 0;
    }]);
    register_shutdown_function(static fn() => print("SHUTDOWN\n"));
    $client->recvPacket();
});
