<?php
declare(strict_types=1);
[$script, $mode, $port, $username, $password] = $argv;
$run = static function () use ($mode, $port, $username, $password): void {
    $client = $mode === 'sync' ? new Swoole\Client(SWOOLE_SOCK_TCP) : new Swoole\Coroutine\Client(SWOOLE_SOCK_TCP);
    $settings = ['socks5_host' => '127.0.0.1', 'socks5_port' => (int)$port, 'timeout' => 2];
    if ($username !== '' || $password !== '') {
        $settings['socks5_username'] = $username;
        $settings['socks5_password'] = $password;
    }
    $client->set($settings);
    $connected = @$client->connect('destination.test', 443, 2);
    $received = null;
    if ($connected) {
        if ($client->send('PING') !== 4) throw new RuntimeException('Payload send failed');
        $received = $client->recv();
        if ($received !== 'PONG') throw new RuntimeException('Payload response mismatch');
        $client->close();
    }
    echo json_encode(['connected' => $connected, 'received' => $received], JSON_THROW_ON_ERROR), "\n";
};
if ($mode === 'sync') $run(); else Swoole\Coroutine\run($run);
