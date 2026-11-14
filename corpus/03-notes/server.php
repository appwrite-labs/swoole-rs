<?php
require __DIR__ . '/../bootstrap.php';
$notes = [];
$nextId = 1;
$server = httpServer();
$server->on('request', static function ($request, $response) use (&$notes, &$nextId): void {
    $method = $request->server['request_method'];
    $path = $request->server['request_uri'];
    if (($request->header['authorization'] ?? '') !== 'Bearer corpus-secret') {
        jsonResponse($response, ['error' => 'unauthorized'], 401);
        return;
    }
    if ($path === '/notes' && $method === 'POST') {
        $input = json_decode($request->rawContent(), true);
        if (!is_array($input) || !is_string($input['title'] ?? null) || trim($input['title']) === '') {
            jsonResponse($response, ['error' => 'title required'], 422);
            return;
        }
        $note = ['id' => $nextId++, 'title' => $input['title']];
        $notes[$note['id']] = $note;
        jsonResponse($response, $note, 201);
    } elseif ($path === '/notes' && $method === 'GET') {
        jsonResponse($response, array_values($notes));
    } elseif (preg_match('#^/notes/(\d+)$#', $path, $match) && isset($notes[(int) $match[1]])) {
        $id = (int) $match[1];
        if ($method === 'GET') {
            jsonResponse($response, $notes[$id]);
        } elseif ($method === 'DELETE') {
            unset($notes[$id]);
            $response->status(204);
            $response->end();
        } else {
            jsonResponse($response, ['error' => 'method not allowed'], 405);
        }
    } else {
        jsonResponse($response, ['error' => 'not found'], 404);
    }
});
$server->start();
