<?php
// Actual Swoole caller checks, not PHP's separate Base64 aliases.
if ($argv[1] === 'client' || $argv[1] === 'proxy') {
    Swoole\Coroutine\run(function () use ($argv) {
        $count = 0;
        foreach (json_decode(file_get_contents($argv[3]), true, 512, JSON_THROW_ON_ERROR) as [$user, $password]) {
            $client = new Swoole\Coroutine\Http\Client('127.0.0.1', (int) $argv[2]);
            $client->set(['timeout' => 5]);
            if ($argv[1] === 'proxy') {
                $client->set(['http_proxy_host' => '127.0.0.1', 'http_proxy_port' => (int) $argv[2],
                              'http_proxy_username' => hex2bin($user), 'http_proxy_password' => hex2bin($password)]);
            } else {
                $client->setBasicAuth(hex2bin($user), hex2bin($password));
            }
            if (!$client->get('/') || $client->statusCode !== 200) {
                throw new RuntimeException('Basic auth request failed: ' . $client->errCode);
            }
            $client->close();
            ++$count;
        }
        echo json_encode(['basic_auth_requests' => $count], JSON_THROW_ON_ERROR), "\n";
    });
} else {
    $server = new Swoole\WebSocket\Server('127.0.0.1', (int) $argv[2], SWOOLE_BASE);
    $server->set(['worker_num' => 1, 'log_file' => '/dev/null']);
    $server->on('start', function () { echo "READY\n"; flush(); });
    $server->on('message', function ($server, $frame) {});
    $server->start();
}
