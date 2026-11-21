<?php
// A response's Set-Cookie overwrites a cookie the user set to an object whose destructor
// ends the request: upstream stops in the header parser, so the request never returns
// and the client's cookie arrays gain nothing after that cookie.
set_error_handler(null);
class Stop
{
    public function __destruct()
    {
        echo "  destruct\n";
        trigger_error('stop', E_USER_ERROR);
    }
}
register_shutdown_function(static function () {
    echo "shutdown\n";
    s('cookies', fn() => $GLOBALS['client']->cookies);
    s('set_cookie_headers', fn() => $GLOBALS['client']->set_cookie_headers);
});
Swoole\Coroutine\run(static function () {
    $server = new Swoole\Coroutine\Socket(AF_INET, SOCK_STREAM, 0);
    $server->bind('127.0.0.1', 0);
    $server->listen();
    $port = $server->getsockname()['port'];
    go(static function () use ($server) {
        $peer = $server->accept();
        $peer->recv();
        $peer->sendAll("HTTP/1.1 200 OK\r\nSet-Cookie: a=1\r\nSet-Cookie: b=2\r\nContent-Length: 2\r\n\r\nok");
        $peer->close();
    });
    $GLOBALS['client'] = $client = new Swoole\Coroutine\Http\Client('127.0.0.1', $port);
    s('setCookies', fn() => $client->setCookies(['a' => new Stop()]));
    s('get', fn() => $client->get('/'));
    echo "after\n";
});
