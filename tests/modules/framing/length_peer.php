<?php
$server = new Swoole\Server('127.0.0.1', (int)getenv('TEST_PORT'), SWOOLE_BASE);
$options = [
    'worker_num' => 1, 'reactor_num' => 1, 'enable_coroutine' => false,
    'open_length_check' => true, 'package_length_type' => 'N',
    'package_body_offset' => 4, 'package_max_length' => 64,
];
$trace = getenv('TEST_TRACE');
if (getenv('TEST_CASE') === 'callback') {
    $options['package_length_func'] = static function (string $bytes) use ($trace): int {
        file_put_contents($trace, strlen($bytes) . "\n", FILE_APPEND);
        return -1;
    };
}
$server->set($options);
if (getenv('TEST_CASE') === 'callback') {
    // Installing a PHP callback changes its requested header width. Set it back
    // to four so both engines receive the same five-byte message at once.
    $server->set(['package_length_offset' => 4]);
}
$server->on('receive', static function ($server, $fd, $rid, $packet) use ($trace) {
    file_put_contents($trace, bin2hex($packet) . "\n", FILE_APPEND);
    $server->close($fd);
});
$server->start();
