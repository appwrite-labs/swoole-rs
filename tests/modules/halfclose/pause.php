<?php
$payload = str_repeat(implode('', array_map(chr(...), range(0, 255))), 16384);
$events = [];
$owners = [];
$server = new Swoole\Server('127.0.0.1', (int) getenv('TEST_PORT'),
    getenv('TEST_MODE') === 'process' ? SWOOLE_PROCESS : SWOOLE_BASE,
    SWOOLE_SOCK_TCP | (getenv('TEST_TLS_CERT') ? SWOOLE_SSL : 0));
$server->set([
    'worker_num' => 1, 'reactor_num' => 1, 'enable_coroutine' => getenv('TEST_COROUTINES') === '1',
    'open_eof_check' => true, 'open_eof_split' => true, 'package_eof' => "\n",
    'kernel_socket_send_buffer_size' => 8192, 'socket_buffer_size' => 16 * 1024 * 1024,
    'output_buffer_size' => 16 * 1024 * 1024, 'max_wait_time' => 1,
    'buffer_high_watermark' => 65536, 'buffer_low_watermark' => 1024,
] + (getenv('TEST_TLS_CERT') ? [
    'ssl_cert_file' => getenv('TEST_TLS_CERT'), 'ssl_key_file' => getenv('TEST_TLS_KEY'),
] : []));
$server->on('close', static function ($server, $fd) use (&$events, &$owners): void {
    if (isset($owners[$fd])) ++$events[$owners[$fd]]['closes'];
});
$server->on('bufferFull', static function ($server, $fd) use (&$events, &$owners): void {
    if (isset($owners[$fd])) ++$events[$owners[$fd]]['full'];
});
$server->on('bufferEmpty', static function ($server, $fd) use (&$events, &$owners): void {
    if (isset($owners[$fd])) ++$events[$owners[$fd]]['drained'];
});
$server->on('receive', static function ($server, $fd, $reactorId, $data) use ($payload, &$events, &$owners): void {
    [$command, $id] = array_pad(explode(':', rtrim($data, "\r\n"), 2), 2, '');
    if ($command === 'stats') {
        $server->send($fd, json_encode(['events' => $events, 'connections' => $server->stats()['connection_num']]) . "\n");
        $server->close($fd);
    } elseif (in_array($command, ['pause', 'resume', 'force', 'send', 'finish', 'finish-only', 'queue'], true)) {
        $target = $events[$id]['fd'];
        $ok = match ($command) {
            'pause' => $server->pause($target),
            'queue' => $server->getClientInfo($target),
            'finish-only' => $server->close($target),
            'resume' => $server->resume($target),
            'force' => $server->close($target, true),
            'finish' => $server->close($target) && $server->resume($target),
            'send' => $server->send($target, $payload),
        };
        $server->send($fd, json_encode(['ok' => $ok]) . "\n");
        $server->close($fd);
    } elseif ($command === 'more') {
        ++$events[$id]['more'];
        $server->send($fd, "more\n");
    } else {
        $owners[$fd] = $id;
        $events[$id] ??= ['fd' => $fd, 'starts' => 0, 'more' => 0, 'closes' => 0, 'full' => 0, 'drained' => 0];
        ++$events[$id]['starts'];
        $events[$id]['sent'] = $server->send($fd, $payload);
        $events[$id]['paused'] = $command === 'start-paused' ? $server->pause($fd) : false;
    }
});
$server->start();
