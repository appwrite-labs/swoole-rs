<?php
// The build variants' workloads (#2683): a coroutine file read, which the --enable-iouring
// build makes through io_uring, and a Swoole\Thread started and joined, which only the ZTS
// --enable-swoole-thread build has.
require __DIR__ . '/../../corpus/bootstrap.php';

$server = httpServer();
$server->on('request', static function ($request, $response): void {
    switch ($request->server['request_uri']) {
        case '/file':
            $response->end(strlen(Swoole\Coroutine\System::readFile(__FILE__)) . "\n");
            return;
        case '/thread':
            (new Swoole\Thread(__DIR__ . '/thread.php'))->join();
            $response->end("joined\n");
            return;
    }
    $response->end("ok\n");
});
$server->start();
