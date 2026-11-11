<?php
// Coroutine\Client and Coroutine\Http\Client connect with no descriptor left, and the
// error handler ends the request on "new Socket() failed": upstream stops there, so
// neither client records an error.
if (!getenv('SWR_CLIENT')) {
    // Run this program's command line once per client, under a small descriptor limit.
    $argv = explode("\0", rtrim(file_get_contents('/proc/self/cmdline'), "\0"));
    $argv[0] = PHP_BINARY;
    // Under a sanitizer the child, with no descriptor left, can't open a report file; it
    // writes to its stderr, which is discarded: the program's output is on stdout.
    $sanitizer = [];
    foreach (['ASAN_OPTIONS', 'LSAN_OPTIONS', 'UBSAN_OPTIONS'] as $name) {
        if (getenv($name) !== false) {
            $sanitizer[$name] = getenv($name) . ':log_path=stderr';
        }
    }
    foreach (['client', 'http'] as $kind) {
        $child = proc_open(['/bin/sh', '-c', 'ulimit -n 64 && exec "$@"' . ($sanitizer ? ' 2>/dev/null' : ''), 'sh', ...$argv],
                           [], $pipes, null,
                           $sanitizer + getenv() + ['SWR_CLIENT' => $kind]);
        echo "$kind exit = ", proc_close($child), "\n";
    }
    exit;
}
$client = getenv('SWR_CLIENT') === 'http'
    ? new Swoole\Coroutine\Http\Client('127.0.0.1', 1)
    : new Swoole\Coroutine\Client(SWOOLE_SOCK_TCP);
register_shutdown_function(static fn() => print("errCode = {$client->errCode}, errMsg = " . r($client->errMsg) . "\n"));
Swoole\Coroutine\run(static function () use ($client) {
    $held = [];
    while (($file = @fopen('/dev/null', 'r')) !== false) {
        $held[] = $file;
    }
    set_error_handler(static function (int $severity, string $message): bool {
        echo "  warning($severity): $message\n";
        trigger_error('stop', E_USER_ERROR);
    });
    s('connect', fn() => $client instanceof Swoole\Coroutine\Client ? $client->connect('127.0.0.1', 1) : $client->get('/'));
});
