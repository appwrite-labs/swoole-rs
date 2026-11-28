<?php
// An application-shaped workload (#1907): routing, JSON in and out, validation, a
// pooled Redis connection over hooked streams, a file read and three coroutines per
// request. Redis is REDIS_HOST:REDIS_PORT (default 127.0.0.1:6379).
require __DIR__ . '/../../corpus/bootstrap.php';

function redis(Swoole\ConnectionPool $pool, string ...$args): ?string {
    $command = '*' . count($args) . "\r\n";
    foreach ($args as $arg) {
        $command .= '$' . strlen($arg) . "\r\n$arg\r\n";
    }
    $conn = $pool->get();
    try {
        fwrite($conn, $command);
        $line = rtrim(fgets($conn), "\r\n");
        return match ($line[0]) {
            '+', ':' => substr($line, 1),
            '$' => $line === '$-1' ? null : substr(stream_get_contents($conn, (int) substr($line, 1) + 2), 0, -2),
            default => throw new RuntimeException("redis: $line"),
        };
    } finally {
        $pool->put($conn);
    }
}

function validate(mixed $order): array {
    $errors = [];
    if (!is_string($order['sku'] ?? null) || !preg_match('/^[A-Z]-\d{1,4}$/', $order['sku'])) {
        $errors['sku'] = 'expected a SKU like A-1';
    }
    if (!is_int($order['quantity'] ?? null) || $order['quantity'] < 1 || $order['quantity'] > 100) {
        $errors['quantity'] = 'expected an integer from 1 to 100';
    }
    if (isset($order['note']) && (!is_string($order['note']) || strlen($order['note']) > 200)) {
        $errors['note'] = 'expected at most 200 characters';
    }
    return $errors;
}

$routes = [
    ['GET', '#^/health$#', static fn() => [200, ['ok' => true]]],
    ['POST', '#^/users/(\d+)/orders$#', static function (array $params, string $body, Swoole\ConnectionPool $pool): array {
        $order = json_decode($body, true);
        if (!is_array($order) || $errors = validate($order)) {
            return [422, ['errors' => $errors ?? ['body' => 'expected a JSON object']]];
        }
        $user = (int) $params[1];
        $key = "order:$user:{$order['sku']}";
        [$stored, $stock, $catalog] = array_values(Swoole\Coroutine\batch([
            static fn() => redis($pool, 'SET', $key, json_encode($order)) === 'OK'
                && json_decode(redis($pool, 'GET', $key), true) === $order,
            static fn() => (int) redis($pool, 'GET', "stock:{$order['sku']}"),
            static fn() => json_decode(file_get_contents(__DIR__ . '/catalog.json'), true),
        ]));
        $price = $catalog[$order['sku']] ?? null;
        if ($price === null) {
            return [404, ['error' => 'unknown sku']];
        }
        return [201, ['user' => $user, 'sku' => $order['sku'], 'quantity' => $order['quantity'],
            'total' => $price * $order['quantity'], 'in_stock' => $stock >= $order['quantity'], 'stored' => $stored]];
    }],
];

$server = httpServer();
$server->set(['hook_flags' => SWOOLE_HOOK_ALL]);
$pool = null;
$server->on('request', static function ($request, $response) use ($routes, &$pool): void {
    // The first request sets up: with hooks on, I/O in workerStart yields, and the worker
    // serves requests before it returns (#2235).
    if ($pool === null) {
        $address = 'tcp://' . (getenv('REDIS_HOST') ?: '127.0.0.1') . ':' . (getenv('REDIS_PORT') ?: '6379');
        $connections = new Swoole\ConnectionPool(static fn() => stream_socket_client($address, $code, $message, 5)
            ?: throw new RuntimeException("redis: $message"), 4);
        redis($connections, 'SET', 'stock:A-1', '17');
        $pool = $connections;
    }
    $path = $request->server['request_uri'];
    $allowed = false;
    foreach ($routes as [$method, $pattern, $handler]) {
        if (preg_match($pattern, $path, $params)) {
            $allowed = true;
            if ($method === $request->server['request_method']) {
                [$status, $data] = $handler($params, $request->rawContent() ?: '', $pool);
                jsonResponse($response, $data, $status);
                return;
            }
        }
    }
    jsonResponse($response, ['error' => $allowed ? 'method not allowed' : 'not found'], $allowed ? 405 : 404);
});
$server->start();
