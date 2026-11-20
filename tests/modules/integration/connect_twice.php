<?php
// The pinned tests/swoole_http2_client_coro/connect_twice.phpt against a local HTTP/2 server
// in place of nghttp2.org's httpbin, which answers 502 (#1986): the same two coroutines on
// one client, the second connect refused and its recv false (#5358). The PHPT jobs' httpbin
// speaks only HTTP/1.1, so a Swoole server here answers /httpbin/base64/<x> with x decoded.
use Swoole\Coroutine\Channel;
use Swoole\Coroutine\Http2\Client;
use Swoole\Http2\Request;
use function Swoole\Coroutine\run;
use function Swoole\Coroutine\go;

$ready = stream_socket_pair(STREAM_PF_UNIX, STREAM_SOCK_STREAM, STREAM_IPPROTO_IP);
$server = new Swoole\Process(function (Swoole\Process $process) use ($ready) {
    $http = new Swoole\Http\Server('127.0.0.1', 0, SWOOLE_BASE);
    $http->set(['open_http2_protocol' => true, 'worker_num' => 1, 'log_level' => SWOOLE_LOG_ERROR]);
    $http->on('start', function ($http) use ($ready) {
        fwrite($ready[1], $http->port . "\n");
    });
    $http->on('request', function ($request, $response) {
        $response->end(base64_decode(substr($request->server['request_uri'], strlen('/httpbin/base64/'))));
    });
    $http->start();
}, false, 0);
$server->start();
// Stopped however the client ends, so a failed assertion doesn't leave it holding stdout.
register_shutdown_function(function () use ($server) {
    Swoole\Process::kill($server->pid, SIGTERM);
    Swoole\Process::wait();
});
$port = (int) fgets($ready[0]);

run(function () use ($port) {
    $client = new Client('127.0.0.1', $port);
    $chan = new Channel(1);
    go(function () use ($client, $chan) {
        $client->connect();
        $req = new Request();
        $uuid = uniqid();
        $req->method = 'GET';
        $req->path = '/httpbin/base64/' . base64_encode($uuid);
        $client->send($req);
        $chan->push(true);
        $resp = $client->recv();
        assert($resp !== null);
        assert($resp->statusCode === 200);
        assert($resp->data === $uuid);
        $chan->pop();
    });
    go(function () use ($client, $chan) {
        assert($client->connect() === false);
        $uuid = uniqid();
        $req = new Request();
        $req->method = 'GET';
        $req->path = '/httpbin/base64/' . base64_encode($uuid);
        $client->send($req);
        $chan->push(true);
        assert($client->recv() === false);
        $chan->pop();
    });
});
