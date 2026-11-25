<?php
use Swoole\Coroutine\Http\Client;
use Swoole\Coroutine\Http\Server;

// Each encoder flushed after 64 'a' bytes and finished after another 65 'b' bytes.
$chunks = [
    'br' => ['8b1f00f825c2e28c00c00c', '0104805f422cce08000e'],
    'zstd' => ['28b52ffd00584400001061610100930016', '0b020062'],
];
Co\run(static function () use ($chunks): void {
    $server = new Server('127.0.0.1', 0);
    $server->handle('/', static function ($request, $response) use ($chunks): void {
        $encoding = $request->get['encoding'];
        $response->header('Content-Encoding', $encoding);
        [$first, $second] = $chunks[$encoding];
        $response->write(hex2bin($first));
        $response->end(hex2bin($second));
    });
    go(static fn () => $server->start());
    $prefix = str_repeat('a', 64);
    $complete = $prefix . str_repeat('b', 65);
    $file = tempnam(sys_get_temp_dir(), 'compressed-limit-');
    try {
        foreach (array_keys($chunks) as $encoding) {
            foreach (['body', 'callback', 'download'] as $sink) {
                foreach ([129, 128] as $limit) {
                    $client = new Client('127.0.0.1', $server->port);
                    $received = '';
                    $settings = ['package_max_length' => $limit];
                    if ($sink === 'callback') {
                        $settings['write_func'] = static function ($owner, $chunk) use (&$received) {
                            $received .= $chunk;
                            return strlen($chunk);
                        };
                    }
                    $client->set($settings);
                    $path = "/?encoding=$encoding";
                    $ok = $sink === 'download' ? $client->download($path, $file) : $client->get($path);
                    $actual = match ($sink) {
                        'body' => $client->body,
                        'callback' => $received,
                        'download' => file_get_contents($file),
                    };
                    $fits = $limit === 129;
                    $expected = $fits ? $complete : ($sink === 'body' ? '' : $prefix);
                    if ($ok !== $fits || $actual !== $expected ||
                        (!$fits && $client->errCode !== SWOOLE_ERROR_PACKAGE_LENGTH_TOO_LARGE)) {
                        throw new RuntimeException("$encoding $sink limit $limit: " .
                            json_encode([$ok, $client->errCode, bin2hex($actual)]));
                    }
                    $client->close();
                }
            }
        }
    } finally {
        unlink($file);
        $server->shutdown();
    }
});
echo "passed\n";
