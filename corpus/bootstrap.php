<?php
function httpServer(): Swoole\Http\Server {
    $server = new Swoole\Http\Server('127.0.0.1', (int) getenv('TEST_PORT'));
    $server->set(['worker_num' => 1]);
    return $server;
}

function jsonResponse(Swoole\Http\Response $response, mixed $data, int $status = 200): void {
    $response->status($status);
    $response->header('Content-Type', 'application/json');
    $response->end(json_encode($data, JSON_THROW_ON_ERROR));
}
