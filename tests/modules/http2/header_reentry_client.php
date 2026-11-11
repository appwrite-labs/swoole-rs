<?php
// test_header_reentry.py's client: each path of tests/modules/http/header_reentry.php
// over HTTP/2, one JSON line per response. The last request's header value replaces
// the request's own header array while the client sends it.
final class Value
{
    public function __construct(private Closure $effect) {}

    public function __toString(): string
    {
        ($this->effect)();
        $GLOBALS['filler'] = array_map(fn($i) => ['k' . $i => str_repeat('z', 24)], range(1, 64));
        return 'r';
    }
}

Co\run(static function (): void {
    $client = new Swoole\Coroutine\Http2\Client('127.0.0.1', (int) getenv('TEST_PORT'));
    if (!$client->connect()) {
        throw new RuntimeException('connect failed');
    }
    foreach (['/replace-empty', '/replace-new', '/nested', '/trailer', '/', '/client'] as $path) {
        $request = new Swoole\Http2\Request();
        $request->path = $path === '/client' ? '/' : $path;
        if ($path === '/client') {
            $request->headers = ['x-r' => new Value(static function () use ($request) { $request->headers = []; })];
        }
        $client->send($request);
        $response = $client->recv(5);
        $headers = array_filter((array) $response->headers, fn($key) => str_starts_with($key, 'x-'), ARRAY_FILTER_USE_KEY);
        ksort($headers);
        echo json_encode([$path, $response->statusCode, $headers, $response->data]), "\n";
    }
    $client->close();
});
