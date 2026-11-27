<?php
// A raw deflate stream is valid deflate but has no gzip header.
use Swoole\Coroutine\Http\Client;
use Swoole\Coroutine\Socket;

Swoole\Coroutine\run(function () {
    $wire = gzdeflate('test response');
    $listener = new Socket(STREAM_PF_INET, STREAM_SOCK_STREAM, 0);
    $listener->bind('127.0.0.1', 0);
    $listener->listen();
    $path = tempnam(sys_get_temp_dir(), 'swoole-decode-');
    try {
        foreach (['buffer', 'callback', 'download', 'raw'] as $mode) {
            go(static function () use ($listener, $wire) {
                $peer = $listener->accept(3);
                $peer->recv();
                $peer->sendAll("HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Encoding: gzip\r\n"
                    . 'Content-Length: ' . strlen($wire) . "\r\n\r\n" . $wire);
                $peer->close();
            });
            $client = new Client('127.0.0.1', $listener->getsockname()['port']);
            $received = '';
            if ($mode === 'callback') {
                $client->set(['write_func' => static function ($client, $chunk) use (&$received) {
                    $received .= $chunk;
                }]);
            } elseif ($mode === 'raw') {
                $client->set(['body_decompression' => false]);
            }
            $ok = $mode === 'download' ? $client->download('/', $path) : $client->get('/');
            $expected = $mode === 'raw'
                ? $ok && $client->body === $wire
                : !$ok && $client->errCode === SWOOLE_ERROR_HTTP_INVALID_PROTOCOL && !$client->connected
                    && (string) $client->body === '' && $received === '' && file_get_contents($path) === '';
            echo $mode, $expected ? " passed\n" : " failed\n";
            $client->close();
        }
    } finally {
        $listener->close();
        unlink($path);
    }
});
