<?php
$server = new Swoole\Coroutine\Http\Server('127.0.0.1', 0);
foreach (['/', '/named'] as $path) {
    $handler = static function () use ($server) {};
    $registered = WeakReference::create($handler);
    $server->handle($path, $handler);
    unset($handler);
    var_dump($registered->get() !== null);
    $server->handle($path, static function () {});
    var_dump($registered->get() === null);
}
