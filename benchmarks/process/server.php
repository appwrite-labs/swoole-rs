<?php
// plaintext under SWOOLE_PROCESS with two workers (#2654): the master's reactor reads each
// request and dispatches it to a worker over the unix socket, which every other workload
// (SWOOLE_BASE or one worker) skips.
$server = new Swoole\Http\Server('127.0.0.1', (int) getenv('TEST_PORT'), SWOOLE_PROCESS);
$server->set(['worker_num' => 2, 'reactor_num' => 1]);
$server->on('request', static fn($request, $response) => $response->end("Hello, Swoole!\n"));
$server->start();
