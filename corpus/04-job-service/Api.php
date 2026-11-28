<?php
final class Api {
    public function __construct(private Store $store) {}

    public function __invoke(Swoole\Http\Request $request, Swoole\Http\Response $response): void {
        $path = $request->server['request_uri'];
        $method = $request->server['request_method'];
        if ($path === '/health') {
            jsonResponse($response, ['ok' => true]);
            return;
        }
        if (($request->header['authorization'] ?? '') !== 'Bearer corpus-secret') {
            jsonResponse($response, ['error' => 'unauthorized'], 401);
            return;
        }
        if ($path === '/jobs' && $method === 'POST') {
            $input = json_decode($request->rawContent(), true);
            if (!is_array($input) || !is_array($input['payload'] ?? null)) {
                jsonResponse($response, ['error' => 'payload required'], 422);
                return;
            }
            jsonResponse($response, $this->store->create($input['payload']), 201);
        } elseif ($path === '/metrics' && $method === 'GET') {
            jsonResponse($response, $this->store->counts());
        } elseif (preg_match('#^/jobs/(\d+)(/complete)?$#', $path, $match)) {
            if ($method === 'POST' && isset($match[2])) {
                $job = $this->store->complete((int) $match[1]);
            } elseif ($method === 'GET' && !isset($match[2])) {
                $job = $this->store->find((int) $match[1]);
            } else {
                jsonResponse($response, ['error' => 'method not allowed'], 405);
                return;
            }
            jsonResponse($response, $job ?? ['error' => 'not found'], $job ? 200 : 404);
        } else {
            jsonResponse($response, ['error' => 'not found'], 404);
        }
    }
}
