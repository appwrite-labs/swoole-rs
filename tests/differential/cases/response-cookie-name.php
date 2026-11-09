<?php
$res = new Swoole\Http\Response();
foreach (['', 'a=b', 'a;b', 'a,b', "a\tb", 'a b', "a\rb", "a\nb"] as $name) {
    s('response.cookie', fn() => $res->cookie($name, 'v'));
    s('response.rawcookie', fn() => $res->rawcookie($name, 'v', 60));
    s('response.setCookie', fn() => $res->setCookie($name));
}
