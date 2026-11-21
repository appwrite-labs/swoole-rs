<?php
$mode = $argv[1];
function check(bool $condition, string $message): void { if (!$condition) { throw new RuntimeException($message); } }
check(in_array($mode, ['retry', 'callback'], true), 'valid mode');
$warnings = [];
set_error_handler(static function (int $severity, string $message) use (&$warnings): bool { $warnings[] = [$severity, $message]; return true; });
$process = proc_open([PHP_BINARY, '-n', __DIR__ . '/php_http_peer.php', $mode], [0=>['pipe','r'],1=>['pipe','w'],2=>['pipe','w']], $pipes);
check(is_resource($process), 'spawn peer'); fclose($pipes[0]);
try {
    stream_set_timeout($pipes[1], 15);
    $address = trim((string) fgets($pipes[1]));
    check(preg_match('/^127\.0\.0\.1:([0-9]+)$/D', $address, $matches) === 1, 'peer ready');
    $summary = null;
    Swoole\Coroutine\run(static function () use ($mode, $matches, &$summary): void {
        $client = new Swoole\Coroutine\Http\Client('127.0.0.1', (int) $matches[1]);
        $chunks = [];
        $settings = ['max_retries'=>1, 'timeout'=>3];
        if ($mode === 'callback') {
            $settings['write_func'] = static function ($connection, string $bytes) use (&$chunks): void {
                $chunks[] = $bytes;
                check($connection->set(['max_retries'=>0]), 'mutate retry setting from body callback');
            };
        }
        check($client->set($settings), 'set options');
        check($client->get('/witness'), 'get returns true');
        $cookies = $client->cookies;
        check($cookies['token'] === "one two three\0tail", 'binary cookie decode');
        check($cookies['odd;name'] === 'value', 'native first equals cookie grammar');
        check(in_array('nameless', $cookies, true), 'anonymous cookie');
        check($client->statusCode === ($mode === 'retry' ? 200 : 502), 'status after retry decision');
        check(($mode === 'retry' ? $client->body : implode('', $chunks)) === ($mode === 'retry' ? 'ok' : 'old'), 'exact response bytes');
        $summary = ['mode'=>$mode, 'status'=>$client->statusCode, 'body'=>$client->body,
            'callback_body'=>implode('', $chunks), 'cookies'=>array_map('base64_encode', $cookies), 'close'=>$client->close()];
    });
    $output = stream_get_contents($pipes[1]); $error = stream_get_contents($pipes[2]);
    fclose($pipes[1]); fclose($pipes[2]); $exit = proc_close($process); $process = null;
    check($exit === 0 && $error === '', 'peer clean exit: ' . $error);
    $peer = json_decode(trim($output), true, 512, JSON_THROW_ON_ERROR);
    check($peer['requests'] === array_fill(0, $mode === 'retry' ? 2 : 1, 'GET /witness HTTP/1.1'), 'exact request count and target');
    check($warnings === [], 'no warnings: ' . json_encode($warnings));
    echo json_encode(['result'=>$summary, 'peer'=>$peer, 'warnings'=>$warnings], JSON_THROW_ON_ERROR), "\n";
} finally {
    if (is_resource($process)) { proc_terminate($process); foreach ($pipes as $pipe) { if (is_resource($pipe)) { fclose($pipe); } } proc_close($process); }
}
