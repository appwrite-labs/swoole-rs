<?php
function registeredCycle(string $path): array {
    $server = new Swoole\Coroutine\Http\Server('127.0.0.1', 0);
    $handler = static fn() => $server->port;
    $server->handle($path, $handler);
    return [WeakReference::create($server), $handler];
}

foreach (['/', '/named'] as $path) {
    [$weak, $handler] = registeredCycle($path);
    gc_collect_cycles();
    var_dump($weak->get() !== null && $handler() > 0);
    unset($handler);
    gc_collect_cycles();
    var_dump($weak->get() === null);
}

$server = new Swoole\Coroutine\Http\Server('127.0.0.1', 0);
$weak = WeakReference::create($server);
$handler = static fn() => $server->port;
$server->handle('/', $handler);
$server->handle('/same', $handler);
$server->set(['owner' => $server]);
unset($server, $handler);
gc_collect_cycles();
var_dump($weak->get() === null);
