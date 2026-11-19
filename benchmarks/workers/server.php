<?php
// plaintext on four workers, for requests/s and CPU per request across cores (wall.py; every
// other HTTP fixture has one worker, or process-mode's two). SWOOLE_BASE here: each worker
// accepts and serves its own connections. process/server.php is the same server under
// SWOOLE_PROCESS, where two reactor threads in the master read each request and pass it to
// a worker. The counts are fixed so results compare across machines; the load generator
// needs cores of its own (`--wrk-threads`).
$server = new Swoole\Http\Server('127.0.0.1', (int) getenv('TEST_PORT'), defined('MODE') ? MODE : SWOOLE_BASE);
$server->set(['worker_num' => 4, 'reactor_num' => 2]);
$server->on('request', static fn($request, $response) => $response->end("Hello, Swoole!\n"));
$server->start();
