<?php
// Actual async Client close/GC/reentry witnesses. No fixed ports or global files.
$mode = $argv[1] ?? 'prefix';
if (!in_array($mode, ['prefix', 'empty', 'nul', 'replace', 'refused', 'bailout'], true)) { throw new RuntimeException('invalid mode'); }
function check(bool $condition, string $message): void {
    if (!$condition) { throw new RuntimeException($message); }
}
$warnings = [];
set_error_handler(static function (int $severity, string $message) use (&$warnings): bool {
    $warnings[] = ['severity' => $severity, 'message' => $message];
    return true;
});
if ($mode === 'refused' || $mode === 'bailout') {
    $listener = stream_socket_server('tcp://127.0.0.1:0', $errno, $error);
    check(is_resource($listener), 'reserve refused port');
    $endpoint = stream_socket_get_name($listener, false);
    $port = (int) substr($endpoint, strrpos($endpoint, ':') + 1);
    fclose($listener);
    $trace = [];
    $client = new Swoole\Async\Client(SWOOLE_SOCK_TCP);
    $weak = WeakReference::create($client);
    $client->on('c', static function (): void { throw new RuntimeException('unexpected connect'); });
    $client->on('r', static function (): void { throw new RuntimeException('unexpected receive'); });
    $client->on('cl', static function (): void { throw new RuntimeException('unexpected close'); });
    $client->on('e', static function ($connection) use (&$client, &$trace, $mode): void {
        check($connection->errCode === SOCKET_ECONNREFUSED, 'refused error');
        check(!$connection->isConnected(), 'inactive error callback');
        if ($mode === 'bailout') { $trace[] = 'error-before-exit'; echo "async-native-effect-bailout\n"; exit(43); }
        $trace[] = 'error';
        $client = null;
        gc_collect_cycles();
    });
    check($client->connect('127.0.0.1', $port, 3), 'refused connect scheduled');
    Swoole\Event::wait();
    if ($mode === 'bailout') {
        $trace[] = 'wait-return';
        check($trace === ['error-before-exit', 'wait-return'], 'native callback exit returns from Event::wait');
        check(!$client->isConnected(), 'inactive after callback exit');
        $client = null;
    }
    gc_collect_cycles();
    check($trace === ($mode === 'bailout' ? ['error-before-exit', 'wait-return'] : ['error']) && $weak->get() === null, 'error cleanup/GC');
    check($warnings === [], 'no warnings');
    echo json_encode(['mode' => $mode, 'trace' => $trace, 'collected' => true, 'warnings' => $warnings]), "\n";
    exit;
}
$process = proc_open([PHP_BINARY, '-n', __DIR__ . '/php_peer.php', 'receive'],
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
    $connectName = match ($mode) { 'empty' => '', 'nul' => "connect\0tail", default => 'c' };
    $released = [];
    if ($mode === 'replace') {
        $capture = new class($released) {
            private $events;
            public function __construct(array &$events) { $this->events =& $events; }
            public function __destruct() { $this->events[] = 'released'; }
        };
        $client->on('c', static function () use ($capture): void { throw new RuntimeException('replaced callback called'); });
        unset($capture);
        check($released === [], 'capture retained by original callback');
    }
    $client->on($connectName, static function ($connection) use (&$trace, $mode): void {
        $trace[] = 'connect';
        check($connection->isConnected(), 'connected in callback');
        check($connection->send('ping') === 4, 'send request');
    });
    check($released === ($mode === 'replace' ? ['released'] : []), 'old callback capture release');
    $client->on('r', static function ($connection, string $bytes) use (&$trace, &$received, &$chunks): void {
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
    $client->on('e', static function ($connection) use (&$trace): void {
        $trace[] = 'error';
        throw new RuntimeException('unexpected client error ' . $connection->errCode);
    });
    $client->on('cl', static function ($connection) use (&$trace, &$client): void {
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
    check($warnings === [], 'no warnings: ' . json_encode($warnings));
    $peerOutput = stream_get_contents($pipes[1]);
    $peerError = stream_get_contents($pipes[2]);
    fclose($pipes[1]); fclose($pipes[2]);
    $childExit = proc_close($process);
    $process = null;
    check($childExit === 0 && $peerError === '', 'peer clean exit: ' . $peerError);
    $peer = json_decode(trim($peerOutput), true, 512, JSON_THROW_ON_ERROR);
    check($peer['eof'] === true, 'peer observes close EOF');
    $expected = ['connect', 'receive', 'close', 'reenter-false', 'release-owner', 'receive-return'];
    check($trace === $expected, 'exact callback order: ' . json_encode($trace));
    echo json_encode(['mode' => $mode, 'trace' => $trace, 'client_collected' => true,
        'capture_releases' => $released, 'response' => $received, 'receive_chunks' => $chunks, 'peer' => $peer, 'warnings' => $warnings], JSON_THROW_ON_ERROR), "\n";
} finally {
    if (is_resource($process)) {
        proc_terminate($process);
        foreach ($pipes as $pipe) { if (is_resource($pipe)) { fclose($pipe); } }
        proc_close($process);
    }
}
