<?php
// Actual async Client close/GC/reentry witnesses. No fixed ports or global files.
$mode = $argv[1] ?? 'connect';
if (!in_array($mode, ['connect', 'receive'], true)) { throw new RuntimeException('invalid mode'); }
function check(bool $condition, string $message): void {
    if (!$condition) { throw new RuntimeException($message); }
}
$warnings = [];
set_error_handler(static function (int $severity, string $message) use (&$warnings): bool {
    $warnings[] = ['severity' => $severity, 'message' => $message];
    return true;
});
$process = proc_open([PHP_BINARY, '-n', __DIR__ . '/php_peer.php', $mode],
    [0 => ['pipe', 'r'], 1 => ['pipe', 'w'], 2 => ['pipe', 'w']], $pipes);
check(is_resource($process), 'spawn private peer');
fclose($pipes[0]);
try {
    stream_set_timeout($pipes[1], 15);
    $address = trim((string) fgets($pipes[1]));
    check(preg_match('/^127\.0\.0\.1:([0-9]+)$/D', $address, $matches) === 1, 'peer ready');
    $port = (int) $matches[1];
    $trace = [];
    $received = '';
    $chunks = [];
    $client = new Swoole\Async\Client(SWOOLE_SOCK_TCP);
    $weak = WeakReference::create($client);
    $socketWeak = null;
    $client->on('connect', static function ($connection) use (&$trace, $mode, &$socketWeak): void {
        $trace[] = 'connect';
        check($connection->isConnected(), 'connected in callback');
        if (method_exists($connection, 'getSocket')) {
            // ext/sockets builds: the client holds one Socket, returns it each
            // time, and releases it when it closes.
            $socket = $connection->getSocket();
            check($socket instanceof Socket, 'getSocket returns a Socket');
            check($connection->getSocket() === $socket, 'getSocket returns the socket it holds');
            $socketWeak = WeakReference::create($socket);
            $socket = null;
            check($socketWeak->get() !== null, 'the client keeps its socket');
        }
        if ($mode === 'connect') {
            check($connection->close(), 'close from connect');
            $trace[] = 'connect-return';
        } else {
            check($connection->send('ping') === 4, 'send request');
        }
    });
    $client->on('receive', static function ($connection, string $bytes) use (&$trace, &$received, &$chunks): void {
        $chunks[] = $bytes;
        $received .= $bytes;
        check(strlen($received) <= 4, 'bounded response');
        if (strlen($received) === 4) {
            $trace[] = 'receive';
            check($received === 'pong', 'exact response');
            check($connection->close(), 'close from receive');
            $trace[] = 'receive-return';
        }
    });
    $client->on('error', static function ($connection) use (&$trace): void {
        $trace[] = 'error';
        throw new RuntimeException('unexpected client error ' . $connection->errCode);
    });
    $client->on('close', static function ($connection) use (&$trace, &$client): void {
        $trace[] = 'close';
        check(!$connection->isConnected(), 'inactive before close callback');
        check($connection->close() === false, 'reentrant close rejects');
        $trace[] = 'reenter-false';
        $client = null;
        gc_collect_cycles();
        $trace[] = 'release-owner';
    });
    check($client->connect('127.0.0.1', $port, 3), 'connect scheduled');
    Swoole\Event::wait();
    gc_collect_cycles();
    check($weak->get() === null, 'PHP Client collected after callback/loop');
    check($socketWeak === null || $socketWeak->get() === null, 'closed client released its socket');
    check($warnings === [], 'no warnings: ' . json_encode($warnings));
    $peerOutput = stream_get_contents($pipes[1]);
    $peerError = stream_get_contents($pipes[2]);
    fclose($pipes[1]); fclose($pipes[2]);
    $childExit = proc_close($process);
    $process = null;
    check($childExit === 0 && $peerError === '', 'peer clean exit: ' . $peerError);
    $peer = json_decode(trim($peerOutput), true, 512, JSON_THROW_ON_ERROR);
    check($peer['eof'] === true, 'peer observes close EOF');
    $expected = $mode === 'connect'
        ? ['connect', 'close', 'reenter-false', 'release-owner', 'connect-return']
        : ['connect', 'receive', 'close', 'reenter-false', 'release-owner', 'receive-return'];
    check($trace === $expected, 'exact callback order: ' . json_encode($trace));
    echo json_encode(['mode' => $mode, 'trace' => $trace, 'client_collected' => true,
        'response' => $received, 'receive_chunks' => $chunks, 'peer' => $peer, 'warnings' => $warnings], JSON_THROW_ON_ERROR), "\n";
} finally {
    if (is_resource($process)) {
        proc_terminate($process);
        foreach ($pipes as $pipe) { if (is_resource($pipe)) { fclose($pipe); } }
        proc_close($process);
    }
}
