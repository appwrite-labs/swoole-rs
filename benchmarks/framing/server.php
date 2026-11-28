<?php
// Raw TCP split on an end-of-message marker, and UDP, on TEST_PORT (#2650): each echoes what
// it receives. corpus/07-tcp-framing is the length-prefixed counterpart.
$server = new Swoole\Server('127.0.0.1', (int) getenv('TEST_PORT'), SWOOLE_BASE);
$server->set(['worker_num' => 1, 'open_eof_split' => true, 'package_eof' => "\r\n", 'package_max_length' => 131072]);
$server->on('receive', static function ($server, $fd, $reactorId, $packet): void {
    $server->send($fd, $packet);
});
$server->addListener('127.0.0.1', (int) getenv('TEST_PORT'), SWOOLE_SOCK_UDP)->on('packet',
    static function ($server, $data, $client): void {
        $server->sendto($client['address'], $client['port'], $data);
    });
$server->start();
