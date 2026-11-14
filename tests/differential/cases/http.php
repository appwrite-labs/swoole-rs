<?php
$cookie = new Swoole\Http\Cookie();
s('cookie.toString.unnamed', fn() => $cookie->toString());
s('cookie.toString', fn() => $cookie->withName('n')->withValue('a b;c')->toString());
s('cookie.toString.full', fn() => $cookie->withExpires(1700000000)->withPath('/p')->withDomain('example.com')
    ->withSecure(true)->withHttpOnly(true)->withSameSite('Strict')->withPriority('High')->withPartitioned(true)->toString());
s('cookie.toString.empty', fn() => $cookie->withValue('')->toString());
s('cookie.toString.raw', fn() => (new Swoole\Http\Cookie(false))->withName('r')->withValue('a b;c')->toString());
foreach (['', 'a=b', 'a;b', "a\nb", str_repeat('n', 300)] as $name) {
    s('cookie.toString.name', fn() => (new Swoole\Http\Cookie())->withName($name)->withValue('v')->toString());
}
s('cookie.toArray', fn() => $cookie->toArray());
s('cookie.reset', fn() => $cookie->reset());
s('cookie.toString.reset', fn() => $cookie->toString());
$response = new Swoole\Http\Response();
s('response.initHeader', fn() => $response->initHeader());
s('response.header', fn() => $response->header('X-A', 'b'));
s('response.initHeader.again', fn() => $response->initHeader());
s('response.disconnect', fn() => $response->disconnect());
s('response.disconnect.code', fn() => $response->disconnect(1001, 'bye'));
s('response.isWritable', fn() => $response->isWritable());
foreach (["\x81\x05hello", "\x82\x00", "\x88\x02\x03\xe8", "\x89\x00", "\x8a\x00", "\x01\x03abc", "\x81\x85abcd\x09\x07\x0f\x08\x0e",
          "\x81\x7e\x00\x03abc", "\x81", '', "\x81\x05he", "\xf1\x01x"] as $frame) {
    s('websocket.unpack', fn() => ($f = Swoole\WebSocket\Server::unpack($frame)) ? get_object_vars($f) + ['class' => get_class($f)] : $f);
}
