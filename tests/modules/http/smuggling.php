<?php
// Replies with what the handler received; test_smuggling.py compares it across engines.
$server = new Swoole\Http\Server('127.0.0.1', (int) getenv('TEST_PORT'), SWOOLE_BASE);
$server->set(['worker_num' => 1, 'log_level' => SWOOLE_LOG_NONE, 'package_max_length' => 65536]);
$text = static fn(string $value): string => preg_match('//u', $value) ? $value : 'hex:' . bin2hex($value);
$server->on('request', static function ($request, $response) use ($text): void {
    $header = [];
    foreach ($request->header ?? [] as $name => $value) {
        $header[$text((string) $name)] = is_array($value) ? array_map($text, $value) : $text((string) $value);
    }
    $response->end(json_encode([
        'method' => $text($request->getMethod()),
        'uri' => $text($request->server['request_uri'] ?? ''),
        'query' => $text($request->server['query_string'] ?? ''),
        'protocol' => $request->server['server_protocol'] ?? null,
        'header' => $header,
        'body' => $text((string) $request->getContent()),
    ], JSON_THROW_ON_ERROR));
});
$server->start();
