<?php
require __DIR__ . '/../bootstrap.php';
$server = httpServer();
$server->on('request', static function ($request, $response): void {
    $path = $request->server['request_uri'];
    $method = $request->server['request_method'];
    if ($path === '/echo' && $method === 'POST') {
        $response->header('Content-Type', 'application/octet-stream');
        $response->end($request->rawContent());
    } elseif ($path === '/inspect' && $method === 'GET') {
        parse_str($request->server['query_string'] ?? '', $query);
        jsonResponse($response, ['query' => $query, 'token' => $request->header['x-token'] ?? null]);
    } else {
        jsonResponse($response, ['error' => 'not found'], 404);
    }
});
$server->start();
