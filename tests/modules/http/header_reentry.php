<?php
// A header or trailer value's __toString replaces the array being sent (#3263);
// Response denies unset(). The response carries the array as sending began.
// HTTP/1 and HTTP/2 (tests/modules/http2/test_header_reentry.py) alike.
$server = new Swoole\Http\Server('127.0.0.1', (int) getenv('TEST_PORT'), SWOOLE_BASE);
$server->set(['worker_num' => 1, 'log_level' => SWOOLE_LOG_NONE, 'open_http2_protocol' => true]);

final class Value
{
    public function __construct(private Closure $effect, private string $text) {}

    public function __toString(): string
    {
        ($this->effect)();
        // Reuse what a freed table would have held.
        $GLOBALS['filler'] = array_map(fn($i) => ['k' . $i => str_repeat('z', 24)], range(1, 64));
        return $this->text;
    }
}

$server->on('request', static function ($request, $response): void {
    $effects = [
        '/replace-empty' => static function () use ($response) { $response->header = []; },
        '/replace-new' => static function () use ($response) { $response->header = ['X-New' => 'n']; },
        // Replaces the nested array while its first item converts.
        '/nested' => static function () use ($response) { $response->header['X-List'] = 'flat'; },
    ];
    $path = $request->server['request_uri'];
    if ($path === '/trailer') {
        // trailer() turns sending trailers on.
        $response->trailer('x-t0', null);
        $response->trailer = [
            'x-t1' => new Value(static function () use ($response) { $response->trailer = []; }, 't1'),
            'x-t2' => 't2',
        ];
        $response->write('chunk');
        $response->end();
        return;
    }
    if (isset($effects[$path])) {
        $value = new Value($effects[$path], $path === '/nested' ? 'l1' : 'a');
        $response->header = $path === '/nested'
            ? ['X-A' => 'a', 'X-List' => [$value, 'l2'], 'X-B' => 'b']
            : ['X-A' => $value, 'X-List' => ['l1', 'l2'], 'X-B' => 'b'];
    }
    $response->end('ok');
});
$server->start();
