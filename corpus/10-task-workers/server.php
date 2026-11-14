<?php
$server = new Swoole\Http\Server('127.0.0.1', (int) getenv('TEST_PORT'));
$server->set(['worker_num' => 1, 'task_worker_num' => 1, 'enable_coroutine' => false]);
$server->on('task', static function ($server, $taskId, $reactorId, $data): array {
    return ['sum' => array_sum($data), 'count' => count($data), 'task_worker' => $server->taskworker];
});
$server->on('finish', static function ($server, $taskId, $result): void {});
$server->on('request', static function ($request, $response) use ($server): void {
    $data = json_decode($request->rawContent(), true);
    $response->header('Content-Type', 'application/json');
    if (!is_array($data) || count(array_filter($data, 'is_int')) !== count($data)) {
        $response->status(422);
        $response->end(json_encode(['error' => 'expected integer array']));
        return;
    }
    $result = $server->taskwait($data, 2.0);
    if ($result === false) {
        $response->status(503);
        $response->end(json_encode(['error' => 'task failed']));
        return;
    }
    $response->end(json_encode($result));
});
$server->start();
